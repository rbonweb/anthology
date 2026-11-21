//! Import validation (MX1): `memory_portable._load_archive` and its helpers. The archive is
//! read into memory once and parsed, never extracted; nothing here writes anywhere.
//!
//! Python reads the archive with `gzip.GzipFile` and CPython 3.12's `tarfile`, and what they
//! accept, refuse or silently stop at decides the outcome. So the readers here are ports of
//! the parts `_load_archive` drives, not the `tar` crate (which is strict where `tarfile` is
//! lenient, and the reverse):
//!
//! - [`Gz`] is `GzipFile` in read mode, its `BufferedReader` and `_GzipReader` read for
//!   read: concatenated members; zero padding after a member; a member's CRC and length, and
//!   whatever follows it, checked only when a read goes past its end (so trailing garbage or a
//!   bad CRC after the last byte `tarfile` needs is never seen, as in Python); `seek` reads
//!   forward, or rewinds and reads again. Corrupt deflate data fails the read whose zlib
//!   `decompress(data, max_length)` call meets it, which depends on how much each call is
//!   asked for, so the call sizes are Python's; and zlib's "invalid distance too far back",
//!   which miniz_oxide does not check, is found by a second decoder.
//! - [`CappedReader`] is `_CappedReader`: no read may end past [`STREAM_LIMIT`]. A read that
//!   would is first measured without keeping the bytes, so a bomb is refused unbuffered.
//! - [`Tar`] is `TarFile.next`, `TarInfo._frombuf`, the `_proc_*` methods and `ExFileObject`
//!   of 3.12.16: the checksum and number fields (`nti`, octal and base-256), the end at a zero
//!   or bad header after the first, GNU longname/longlink, PAX records (3.12.16's framing
//!   checks) and global headers, old and PAX GNU sparse members, and `TarFile.offset`, which
//!   decides where the next header is read.
//!
//! Bounds Python has implicitly are explicit here: a header chain nested deeper than Python's
//! recursion limit allows ([`MAX_DEPTH`], measured on the CLI path) is refused, as Python's
//! `RecursionError` refuses it; and a GNU sparse map with more than [`MAX_SPARSE`] entries, or a
//! value outside `i64`, is refused (Python would build the list; no exporter writes one).
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "positions are u64 and sizes i128, converted after range checks"
)]

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::fs::OpenOptions;
use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::rc::Rc;

use anthology_pycompat::errno_tables;
use anthology_pycompat::json::{self, Int, Value};
use anthology_pycompat::pyerr::PyExc;
use anthology_pycompat::str::{py_int, repr_surrogateescape};
use flate2::{Crc, Decompress, FlushDecompress, Status};
use sha2::{Digest, Sha256};

/// The archive file.
pub const MAX_ARCHIVE: u64 = 50 * 1024 * 1024;
/// All member data together.
pub const MAX_TOTAL: u64 = 50 * 1024 * 1024;
pub const MAX_MEMBERS: usize = 10000;
pub const MAX_MANIFEST: u64 = 1024 * 1024;
/// What the gzip stream may expand to while it is read (GC-12).
pub const STREAM_LIMIT: u64 = MAX_TOTAL + MAX_MEMBERS as u64 * 1024 + 1024 * 1024;
pub const FORMAT: i64 = 1;

/// `ImportRejected`: the archive (or a plan) is unsafe or malformed; nothing was written. The
/// text is Python's `str(exc)`, verbatim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rejected(pub String);

/// What `_check_name` classifies a member name as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Manifest,
    Note,
    Dir,
}

/// What `_load_archive` returns: the manifest object, `{member name: bytes}` of every regular
/// member (the manifest included), and the sha256 hex of the archive's bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct Loaded {
    pub manifest: json::Map,
    pub contents: BTreeMap<String, Vec<u8>>,
    pub sha256: String,
}

/// `BLOCKSIZE`.
const BLOCK: u64 = 512;
/// The most `_fromtarfile` calls one `TarFile.next()` makes before Python's default recursion
/// limit (1000 frames) raises `RecursionError`: a member behind 328 longname or PAX headers
/// loads through `memory_portable.py import`, one behind 329 is refused (measured on the 3.12
/// image). A deeper Python call stack (a test harness) refuses one header earlier.
pub const MAX_DEPTH: usize = 329;
/// GNU sparse `(offset, numbytes)` pairs allowed in one archive (module docs).
pub const MAX_SPARSE: usize = 4096;
/// `tarfile.data_filter` resolves each name under a fresh `tempfile.mkdtemp()`; on the oracle
/// that is `/tmp/tmpXXXXXXXX`, and `os.lstat` of a path of `PATH_MAX` bytes or more fails.
const FILTER_DEST_LEN: usize = "/tmp/tmpXXXXXXXX/".len();
const PATH_MAX: usize = 4096;
#[cfg(target_os = "macos")]
const ENAMETOOLONG: i32 = 63;
#[cfg(not(target_os = "macos"))]
const ENAMETOOLONG: i32 = 36;
const EISDIR: i32 = 21;
#[cfg(target_os = "macos")]
const O_NONBLOCK: i32 = 0x0004;
#[cfg(not(target_os = "macos"))]
const O_NONBLOCK: i32 = 0o4000;

const CAP: &str = "the archive expands beyond the size limit";

fn cap() -> Rejected {
    Rejected(CAP.to_string())
}

/// `ImportRejected("not a readable memory archive: %s" % (exc,))`.
fn not_readable(text: impl std::fmt::Display) -> Rejected {
    Rejected(format!("not a readable memory archive: {text}"))
}

// ---------------------------------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------------------------------

/// `NAME.match(s)`: `^[A-Za-z0-9][A-Za-z0-9._-]*\Z`.
pub fn name_ok(s: &str) -> bool {
    name_ok_b(s.as_bytes())
}

fn name_ok_b(s: &[u8]) -> bool {
    s.first().is_some_and(u8::is_ascii_alphanumeric)
        && s.iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
}

/// `_ok_note_name(filename)`: `<NAME>.md`, and not `memory.md` in any case.
pub fn ok_note_name(filename: &str) -> bool {
    ok_note_b(filename.as_bytes())
}

fn ok_note_b(f: &[u8]) -> bool {
    // A stem that passes NAME is ASCII, so `str.lower()` is ASCII lowercasing here.
    f.strip_suffix(b".md")
        .is_some_and(|stem| !stem.is_empty() && name_ok_b(stem))
        && !f.eq_ignore_ascii_case(b"memory.md")
}

/// `_check_name(name)`.
pub fn check_name(name: &str) -> Result<Kind, Rejected> {
    check_name_b(name.as_bytes())
}

/// `_check_name` of a member name as `tarfile` decodes it: UTF-8 with `surrogateescape`, so
/// any bytes; `%r` prints a byte that is not UTF-8 as `\udcXX`, as Python does.
fn check_name_b(name: &[u8]) -> Result<Kind, Rejected> {
    let unsafe_name = || {
        Rejected(format!(
            "unsafe member name: {}",
            repr_surrogateescape(name)
        ))
    };
    if name.contains(&b'\\') || name.contains(&0) || name.starts_with(b"/") {
        return Err(unsafe_name());
    }
    let parts: Vec<&[u8]> = name.split(|&c| c == b'/').collect();
    if parts.iter().any(|p| matches!(*p, b"" | b"." | b"..")) {
        return Err(unsafe_name());
    }
    match parts.as_slice() {
        [b"manifest.json"] => Ok(Kind::Manifest),
        [b"personal"] | [b"repo"] | [b"repo", b"memory"] | [b"repo", b"agent-memory"] => {
            Ok(Kind::Dir)
        }
        [b"repo", b"agent-memory", agent] if name_ok_b(agent) => Ok(Kind::Dir),
        [b"personal", note] if ok_note_b(note) => Ok(Kind::Note),
        [b"repo", b"memory", note] if ok_note_b(note) => Ok(Kind::Note),
        [b"repo", b"agent-memory", agent, note] if name_ok_b(agent) && ok_note_b(note) => {
            Ok(Kind::Note)
        }
        _ => Err(Rejected(format!(
            "member outside the memory locations: {}",
            repr_surrogateescape(name)
        ))),
    }
}

// ---------------------------------------------------------------------------------------------
// The capped stream
// ---------------------------------------------------------------------------------------------

/// What a [`CappedReader`] reads: Python's `read(n)` (at most `n` bytes, fewer only at the
/// end; kept, or only counted), `seek(pos)` and `tell()`. Any `Read + Seek` is one.
pub trait Stream {
    /// `read(n)`: the bytes if `keep`, and how many there were.
    fn read_n(&mut self, n: u64, keep: bool) -> io::Result<(Vec<u8>, u64)>;
    /// `seek(pos)`: the position reached.
    fn seek_to(&mut self, pos: u64) -> io::Result<u64>;
    /// `tell()`.
    fn tell(&mut self) -> io::Result<u64>;
}

impl<R: Read + Seek> Stream for R {
    fn read_n(&mut self, n: u64, keep: bool) -> io::Result<(Vec<u8>, u64)> {
        let (mut out, mut count) = (Vec::new(), 0u64);
        let mut chunk = [0u8; 8192];
        while count < n {
            let want = (n - count).min(chunk.len() as u64) as usize;
            let got = self.read(&mut chunk[..want])?;
            if got == 0 {
                break;
            }
            if keep {
                out.extend_from_slice(&chunk[..got]);
            }
            count += got as u64;
        }
        Ok((out, count))
    }

    fn seek_to(&mut self, pos: u64) -> io::Result<u64> {
        self.seek(SeekFrom::Start(pos))
    }

    fn tell(&mut self) -> io::Result<u64> {
        self.stream_position()
    }
}

/// `_CappedReader(gz, limit)`: a view of a stream that refuses to go past `limit` bytes. A
/// read error of the stream is `not a readable memory archive: <error>`.
pub struct CappedReader<R> {
    inner: R,
    limit: u64,
}

impl<R: Stream> CappedReader<R> {
    pub fn new(inner: R, limit: u64) -> CappedReader<R> {
        CappedReader { inner, limit }
    }

    /// The stream (`source` in the Python test).
    pub fn get_ref(&self) -> &R {
        &self.inner
    }

    /// `read(n)`; `None` reads everything. As in Python, at most one byte past the limit is
    /// asked for, and reaching past the limit is the size-limit rejection; unlike Python, such
    /// a read is first made without keeping the bytes.
    pub fn read(&mut self, n: Option<u64>) -> Result<Vec<u8>, Rejected> {
        let tell = self.tell()?;
        let Some(room) = self.limit.checked_sub(tell) else {
            return Err(cap());
        };
        let reach = room + 1;
        let n = match n {
            Some(n) if n < reach => n,
            _ => {
                // If all `reach` bytes are there, Python's read ends past the limit.
                let (_, got) = self.inner.read_n(reach, false).map_err(not_readable)?;
                if got == reach {
                    return Err(cap());
                }
                self.inner.seek_to(tell).map_err(not_readable)?;
                reach
            }
        };
        let (data, _) = self.inner.read_n(n, true).map_err(not_readable)?;
        if self.tell()? > self.limit {
            return Err(cap());
        }
        Ok(data)
    }

    /// `seek(pos)`. Past the limit is the rejection; a negative position is 0, as
    /// `GzipFile.seek` makes it.
    pub fn seek(&mut self, pos: i128) -> Result<u64, Rejected> {
        if pos > i128::from(self.limit) {
            return Err(cap());
        }
        self.inner.seek_to(pos.max(0) as u64).map_err(not_readable)
    }

    /// `tell()`.
    pub fn tell(&mut self) -> Result<u64, Rejected> {
        self.inner.tell().map_err(not_readable)
    }
}

// ---------------------------------------------------------------------------------------------
// gzip
// ---------------------------------------------------------------------------------------------

/// `io.DEFAULT_BUFFER_SIZE`: `GzipFile`'s `BufferedReader` and `DecompressReader.seek`.
const BUFFER: usize = 8192;
/// `gzip.READ_BUFFER_SIZE`: the compressed bytes `_GzipReader` hands zlib at a time.
const READ_BUFFER_SIZE: usize = 128 * 1024;
/// The deflate window.
const WINDOW: usize = 32768;
/// How `zlib.error` starts.
const ZLIB: &str = "Error -3 while decompressing data: ";
const GZ_EOF: &str = "Compressed file ended before the end-of-stream marker was reached";

fn gz_error(text: String) -> io::Error {
    io::Error::other(text)
}

/// `gzip.GzipFile(fileobj=io.BytesIO(blob), mode="rb")`: its `BufferedReader` over a
/// `_GzipReader`, call for call, because when zlib meets corrupt data is decided by how much
/// each `decompress(data, max_length)` call is asked for (module docs). The deflate decoder is
/// miniz_oxide, run ahead into a small buffer so a failure's output position is exact; a
/// call fails as zlib's would: when the failure lies within what the call may produce (or right
/// at its end, for a failure that writes nothing), and within the input zlib has been given.
struct Gz<'a> {
    blob: &'a [u8],
    /// `_PaddedFile`'s position: the next compressed byte for a header, a trailer or a new
    /// `READ_BUFFER_SIZE` chunk for zlib.
    cursor: usize,
    /// `_GzipReader._pos`: the decompressed bytes it has returned.
    raw_pos: u64,
    state: GzState,
    dec: Decompress,
    /// Where the decoder's next compressed byte is (it is given bytes up to `cursor` only).
    dec_in: usize,
    /// Decoded bytes not returned yet (at most one decoder call's worth).
    ahead: VecDeque<u8>,
    /// The decoder reached the end of the member's deflate data.
    ended: bool,
    /// Where decoding fails: the raw position, whether the failure is a match that writes
    /// (zlib meets it only with room to write), and zlib's text.
    failure: Option<(u64, bool, String)>,
    crc: Crc,
    shadow: Shadow,
    /// The decoder's output buffer, kept between calls.
    scratch: Vec<u8>,
    /// `BufferedReader`'s buffer (the bytes just before `raw_pos`) and read position.
    buf: Vec<u8>,
    bpos: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GzState {
    /// `_new_member`: a member header is next (or the end).
    Header,
    Body,
    /// `decompressor.eof`: the trailer is checked by the next read.
    Trailer,
    Done,
}

/// A second decoder of the member whose window starts as [`WINDOW`] bytes of `0xff`, not
/// zeros: zlib refuses a match that reaches back before the member's first byte ("invalid
/// distance too far back"), miniz_oxide copies from its window instead. Within the first
/// [`WINDOW`] bytes, the only place such a match can be, the two decoders differ exactly where
/// one is.
struct Shadow {
    dec: Decompress,
    /// Where it reads the member's compressed bytes (it consumes them at its own pace).
    at: usize,
    /// Member bytes it has compared.
    made: usize,
    /// Whether it is checking the current member, and where that member's data starts while
    /// it is not primed yet.
    active: bool,
    start: Option<usize>,
    /// Its output buffer, kept between calls.
    theirs: Vec<u8>,
}

/// A non-final stored block of [`WINDOW`] `0xff` bytes.
static PRIMER: [u8; 5 + WINDOW] = {
    let mut b = [0xff; 5 + WINDOW];
    b[0] = 0x00;
    b[1] = 0x00;
    b[2] = 0x80;
    b[3] = 0xff;
    b[4] = 0x7f;
    b
};

impl Shadow {
    fn new() -> Shadow {
        Shadow {
            dec: Decompress::new(false),
            at: 0,
            made: 0,
            active: false,
            start: None,
            theirs: Vec::new(),
        }
    }

    /// Checks a new member, whose deflate data starts at `at`.
    fn start(&mut self, at: usize) {
        self.active = false;
        self.start = Some(at);
    }

    /// How many of `out`, the member's next decoded bytes, are not copied from before its
    /// start (all of them past the first [`WINDOW`]).
    fn good(&mut self, blob: &[u8], out: &[u8]) -> usize {
        if out.is_empty() {
            return 0;
        }
        if let Some(at) = self.start.take() {
            self.active = self.prime(at);
        }
        if !self.active {
            return out.len();
        }
        let n = out.len().min(WINDOW - self.made);
        let mut theirs = std::mem::take(&mut self.theirs);
        theirs.resize(n, 0);
        let mut filled = 0;
        while filled < n {
            let (i, o) = (self.dec.total_in(), self.dec.total_out());
            let Ok(status) = self.dec.decompress(
                &blob[self.at..],
                &mut theirs[filled..],
                FlushDecompress::None,
            ) else {
                break;
            };
            self.at += (self.dec.total_in() - i) as usize;
            let made = (self.dec.total_out() - o) as usize;
            filled += made;
            if made == 0 && (status == Status::StreamEnd || self.dec.total_in() == i) {
                break;
            }
        }
        self.made += filled;
        let good = out[..filled]
            .iter()
            .zip(&theirs[..filled])
            .take_while(|(a, b)| a == b)
            .count();
        self.theirs = theirs;
        if good < filled {
            self.active = false;
            return good;
        }
        if filled < n || self.made >= WINDOW {
            self.active = false;
        }
        out.len()
    }

    /// The decoder after the primer, ready for the member's data at `at`.
    fn prime(&mut self, at: usize) -> bool {
        self.dec.reset(false);
        let mut sink = [0u8; 8192];
        while self.dec.total_out() < WINDOW as u64 {
            let (i, o) = (self.dec.total_in(), self.dec.total_out());
            let ok = self
                .dec
                .decompress(&PRIMER[i as usize..], &mut sink, FlushDecompress::None)
                .is_ok();
            if !ok || (self.dec.total_in(), self.dec.total_out()) == (i, o) {
                return false;
            }
        }
        self.at = at;
        self.made = 0;
        true
    }
}

/// Where a `_GzipReader.read` call puts its bytes.
enum Dest<'b> {
    Keep(&'b mut Vec<u8>),
    Discard,
}

impl Dest<'_> {
    fn put(&mut self, b: &[u8]) {
        if let Dest::Keep(v) = self {
            v.extend_from_slice(b);
        }
    }
}

impl<'a> Gz<'a> {
    fn new(blob: &'a [u8]) -> Gz<'a> {
        Gz {
            blob,
            cursor: 0,
            raw_pos: 0,
            state: GzState::Header,
            dec: Decompress::new(false),
            dec_in: 0,
            ahead: VecDeque::new(),
            ended: false,
            failure: None,
            crc: Crc::new(),
            shadow: Shadow::new(),
            scratch: Vec::new(),
            buf: Vec::new(),
            bpos: 0,
        }
    }

    /// `_read_exact(fp, n)`.
    fn exact(&mut self, n: usize) -> io::Result<&'a [u8]> {
        if self.blob.len() - self.cursor < n {
            self.cursor = self.blob.len();
            return Err(gz_error(GZ_EOF.to_string()));
        }
        let out = &self.blob[self.cursor..self.cursor + n];
        self.cursor += n;
        Ok(out)
    }

    /// Up to and past the next NUL (or to the end).
    fn skip_string(&mut self) {
        match self.blob[self.cursor..].iter().position(|&b| b == 0) {
            Some(i) => self.cursor += i + 1,
            None => self.cursor = self.blob.len(),
        }
    }

    /// `_read_gzip_header(fp)`: false at the end of the input.
    fn header(&mut self) -> io::Result<bool> {
        let rest = &self.blob[self.cursor..];
        if rest.is_empty() {
            return Ok(false);
        }
        let magic = &rest[..rest.len().min(2)];
        if magic != b"\x1f\x8b" {
            return Err(gz_error(format!(
                "Not a gzipped file ({})",
                bytes_repr(magic)
            )));
        }
        self.cursor += 2;
        let head = self.exact(8)?;
        let (method, flag) = (head[0], head[1]);
        if method != 8 {
            return Err(gz_error("Unknown compression method".to_string()));
        }
        if flag & 4 != 0 {
            let len = self.exact(2)?;
            let len = usize::from(u16::from_le_bytes([len[0], len[1]]));
            self.exact(len)?;
        }
        if flag & 8 != 0 {
            self.skip_string();
        }
        if flag & 16 != 0 {
            self.skip_string();
        }
        if flag & 2 != 0 {
            self.exact(2)?;
        }
        Ok(true)
    }

    /// `_read_eof()`: the trailer, then any zero padding.
    fn trailer(&mut self) -> io::Result<()> {
        let t = self.exact(8)?;
        let crc = u32::from_le_bytes([t[0], t[1], t[2], t[3]]);
        let isize = u32::from_le_bytes([t[4], t[5], t[6], t[7]]);
        if crc != self.crc.sum() {
            return Err(gz_error(format!(
                "CRC check failed {crc:#x} != {:#x}",
                self.crc.sum()
            )));
        }
        if isize != self.crc.amount() {
            return Err(gz_error("Incorrect length of data produced".to_string()));
        }
        while self.blob.get(self.cursor) == Some(&0) {
            self.cursor += 1;
        }
        Ok(())
    }

    /// The decoder's next step on the input it has been given: false when it can do nothing
    /// more without more input (or ever).
    fn decode_more(&mut self) -> bool {
        if self.ended || self.failure.is_some() {
            return false;
        }
        // Wide enough for all the decoder holds, so a failure's position is exact.
        let mut scratch = std::mem::take(&mut self.scratch);
        scratch.resize(2 * WINDOW, 0);
        let (in0, out0) = (self.dec.total_in(), self.dec.total_out());
        let res = self.dec.decompress(
            &self.blob[self.dec_in..self.cursor],
            &mut scratch,
            FlushDecompress::None,
        );
        let used = (self.dec.total_in() - in0) as usize;
        let made = (self.dec.total_out() - out0) as usize;
        self.dec_in += used;
        let good = self.shadow.good(self.blob, &scratch[..made]);
        self.ahead.extend(&scratch[..good]);
        self.scratch = scratch;
        let at = self.raw_pos + self.ahead.len() as u64;
        if good < made {
            self.failure = Some((at, true, format!("{ZLIB}invalid distance too far back")));
        } else {
            match res {
                Err(e) => self.failure = Some((at, false, format!("{ZLIB}{e}"))),
                Ok(Status::StreamEnd) => self.ended = true,
                Ok(_) => {}
            }
        }
        used > 0 || good > 0 || self.ended || self.failure.is_some()
    }

    /// The failure, if a `decompress` call that may produce bytes up to `end` meets it.
    fn failed(&self, end: u64) -> io::Result<()> {
        if !self.ahead.is_empty() {
            return Ok(());
        }
        match &self.failure {
            Some((at, writes, text)) if *at < end || (*at == end && !writes) => {
                Err(gz_error(text.clone()))
            }
            _ => Ok(()),
        }
    }

    /// `_GzipReader.read(size)`: one `decompress` call's bytes (at least one), none at the end
    /// of the stream, or the error.
    fn raw_read(&mut self, size: usize, dest: &mut Dest<'_>) -> io::Result<usize> {
        if size == 0 {
            return Ok(0);
        }
        loop {
            match self.state {
                GzState::Done => return Ok(0),
                GzState::Trailer => {
                    self.trailer()?;
                    self.state = GzState::Header;
                    continue;
                }
                GzState::Header => {
                    if !self.header()? {
                        self.state = GzState::Done;
                        return Ok(0);
                    }
                    self.dec.reset(false);
                    self.crc.reset();
                    self.dec_in = self.cursor;
                    self.ahead.clear();
                    self.ended = false;
                    self.failure = None;
                    self.shadow.start(self.cursor);
                    self.state = GzState::Body;
                }
                GzState::Body => {}
            }
            // `needs_input`: nothing to return from the input zlib has; then the next chunk.
            let mut chunk = None;
            if self.ahead.is_empty() && !self.decode_more() && !self.ended {
                let next = (self.cursor + READ_BUFFER_SIZE).min(self.blob.len());
                chunk = Some(next - self.cursor);
                self.cursor = next;
            }
            let end = self.raw_pos + size as u64;
            let mut made = 0;
            while self.raw_pos < end {
                if self.ahead.is_empty() && !self.decode_more() {
                    break;
                }
                self.failed(end)?;
                let n = self.ahead.len().min((end - self.raw_pos) as usize);
                let (a, b) = self.ahead.as_slices();
                let (a, b) = (&a[..n.min(a.len())], &b[..n - n.min(a.len())]);
                for part in [a, b] {
                    self.crc.update(part);
                    dest.put(part);
                }
                self.ahead.drain(..n);
                self.raw_pos += n as u64;
                made += n;
            }
            // zlib goes on past a full output to what writes nothing (the end of the data, or a
            // bad code right there).
            if self.ahead.is_empty() {
                self.decode_more();
            }
            self.failed(end)?;
            if self.ended && self.ahead.is_empty() {
                // `unused_data` goes back to the file.
                self.cursor = self.dec_in;
                self.state = GzState::Trailer;
            }
            if made > 0 {
                return Ok(made);
            }
            if chunk == Some(0) {
                return Err(gz_error(GZ_EOF.to_string()));
            }
        }
    }

    /// `_GzipReader._rewind()`.
    fn rewind(&mut self) {
        self.cursor = 0;
        self.raw_pos = 0;
        self.state = GzState::Header;
    }

    /// `BufferedReader.read(n)`: the bytes, fewer only at the end of the stream.
    fn buffered_read(&mut self, n: usize, dest: &mut Dest<'_>) -> io::Result<u64> {
        let avail = self.buf.len() - self.bpos;
        if n <= avail {
            dest.put(&self.buf[self.bpos..self.bpos + n]);
            self.bpos += n;
            return Ok(n as u64);
        }
        dest.put(&self.buf[self.bpos..]);
        let mut remaining = n - avail;
        self.buf.clear();
        self.bpos = 0;
        while remaining >= BUFFER {
            let got = self.raw_read(remaining - remaining % BUFFER, dest)?;
            if got == 0 {
                return Ok((n - remaining) as u64);
            }
            remaining -= got;
        }
        while remaining > 0 && self.buf.len() < BUFFER {
            let mut fill = Vec::new();
            let got = self.raw_read(BUFFER - self.buf.len(), &mut Dest::Keep(&mut fill))?;
            if got == 0 {
                break;
            }
            self.buf.extend_from_slice(&fill);
            let take = got.min(remaining);
            dest.put(&self.buf[self.bpos..self.bpos + take]);
            self.bpos += take;
            remaining -= take;
        }
        Ok((n - remaining) as u64)
    }
}

impl Stream for Gz<'_> {
    fn read_n(&mut self, n: u64, keep: bool) -> io::Result<(Vec<u8>, u64)> {
        let mut out = Vec::new();
        let mut dest = if keep {
            Dest::Keep(&mut out)
        } else {
            Dest::Discard
        };
        let got = self.buffered_read(n as usize, &mut dest)?;
        Ok((out, got))
    }

    /// `BufferedReader.seek(pos)`: within the buffer, or `DecompressReader.seek` (forward by
    /// `read(min(8192, left))` calls, back by rewinding first).
    fn seek_to(&mut self, pos: u64) -> io::Result<u64> {
        let start = self.raw_pos - self.buf.len() as u64;
        if self.bpos < self.buf.len() && start <= pos && pos <= self.raw_pos {
            self.bpos = (pos - start) as usize;
            return Ok(pos);
        }
        if pos < self.raw_pos {
            self.rewind();
        }
        while self.raw_pos < pos {
            let want = (pos - self.raw_pos).min(BUFFER as u64) as usize;
            if self.raw_read(want, &mut Dest::Discard)? == 0 {
                break;
            }
        }
        self.buf.clear();
        self.bpos = 0;
        Ok(self.raw_pos)
    }

    fn tell(&mut self) -> io::Result<u64> {
        Ok(self.raw_pos - (self.buf.len() - self.bpos) as u64)
    }
}

/// `repr(b)` of a bytes object.
fn bytes_repr(b: &[u8]) -> String {
    let quote = if b.contains(&b'\'') && !b.contains(&b'"') {
        '"'
    } else {
        '\''
    };
    let mut out = format!("b{quote}");
    for &c in b {
        match c {
            b'\\' => out.push_str("\\\\"),
            b'\t' => out.push_str("\\t"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            _ if char::from(c) == quote => {
                out.push('\\');
                out.push(quote);
            }
            b' '..=b'~' => out.push(char::from(c)),
            _ => out.push_str(&format!("\\x{c:02x}")),
        }
    }
    out.push(quote);
    out
}

// ---------------------------------------------------------------------------------------------
// tar
// ---------------------------------------------------------------------------------------------

/// What `tarfile` raises, as `TarFile.next()` sorts it: the `HeaderError` kinds, or anything
/// else (which rejects the archive with its text).
#[derive(Debug)]
enum Exc {
    Empty,
    Truncated,
    Eof,
    Invalid(&'static str),
    Subsequent(String),
    Fatal(Rejected),
}

impl Exc {
    /// `str(exc)`.
    fn text(&self) -> String {
        match self {
            Exc::Empty => "empty header".to_string(),
            Exc::Truncated => "truncated header".to_string(),
            Exc::Eof => "end of file header".to_string(),
            Exc::Invalid(m) => (*m).to_string(),
            Exc::Subsequent(m) => m.clone(),
            Exc::Fatal(r) => r.0.clone(),
        }
    }
}

impl From<Rejected> for Exc {
    fn from(r: Rejected) -> Exc {
        Exc::Fatal(r)
    }
}

/// `except HeaderError as e: raise SubsequentHeaderError(str(e))`.
fn subsequent(e: Exc) -> Exc {
    match e {
        Exc::Fatal(r) => Exc::Fatal(r),
        e => Exc::Subsequent(e.text()),
    }
}

const INVALID: Exc = Exc::Invalid("invalid header");

fn fatal(text: impl std::fmt::Display) -> Exc {
    Exc::Fatal(not_readable(text))
}

/// `TarInfo`, the fields `_load_archive` depends on.
#[derive(Clone, Debug)]
struct Info {
    name: Rc<[u8]>,
    typ: u8,
    size: i128,
    offset_data: u64,
    /// `sparse`: `(offset, numbytes)` pairs, or `None` for the whole data.
    sparse: Option<Vec<(i128, i128)>>,
    /// Old GNU sparse (`S`): the header's `isextended` and `origsize`.
    sparse_ext: bool,
    sparse_origsize: i128,
}

const DIRTYPE: u8 = b'5';

/// `REGULAR_TYPES`.
fn is_reg(t: u8) -> bool {
    matches!(t, b'0' | b'\0' | b'7' | b'S')
}

/// `isreg() or type not in SUPPORTED_TYPES`: the member's data follows its header.
fn has_data(t: u8) -> bool {
    is_reg(t) || !matches!(t, b'1' | b'2' | b'5' | b'6' | b'3' | b'4' | b'L' | b'K')
}

/// `nts(s)`: up to the first NUL.
fn nts(s: &[u8]) -> &[u8] {
    s.iter().position(|&b| b == 0).map_or(s, |i| &s[..i])
}

fn rstrip_slash(s: &Rc<[u8]>) -> Rc<[u8]> {
    let n = s.len() - s.iter().rev().take_while(|&&b| b == b'/').count();
    if n == s.len() {
        s.clone()
    } else {
        Rc::from(&s[..n])
    }
}

/// `s[a:b]` with Python's clamping.
fn sub(s: &[u8], a: usize, b: usize) -> &[u8] {
    &s[a.min(s.len())..b.min(s.len())]
}

/// `nti(s)`: an octal or base-256 number field; `InvalidHeaderError` otherwise (and
/// `IndexError` for an empty field).
fn nti(s: &[u8]) -> Result<i128, Exc> {
    let Some(&first) = s.first() else {
        return Err(fatal("index out of range"));
    };
    if first == 0o200 || first == 0o377 {
        let mut n: i128 = 0;
        for &b in &s[1..] {
            n = (n << 8) + i128::from(b);
        }
        if first == 0o377 {
            n -= 1i128 << (8 * (s.len() - 1));
        }
        return Ok(n);
    }
    octal(nts(s)).ok_or(INVALID)
}

/// `int(s.strip() or "0", 8)` of ASCII text: `str.strip()`, then `PyLong_FromString` in base
/// 8 (a sign, an optional `0o` and one underscore after it, digits with single underscores).
fn octal(s: &[u8]) -> Option<i128> {
    if !s.is_ascii() {
        return None;
    }
    let ws = |c: &u8| matches!(c, b'\t'..=b'\r' | 0x1c..=b' ');
    let start = s.iter().position(|c| !ws(c)).unwrap_or(s.len());
    let end = s.iter().rposition(|c| !ws(c)).map_or(start, |i| i + 1);
    let mut t = &s[start..end];
    if t.is_empty() {
        return Some(0);
    }
    let neg = t[0] == b'-';
    if matches!(t[0], b'+' | b'-') {
        t = &t[1..];
    }
    if t.len() >= 2 && t[0] == b'0' && matches!(t[1], b'o' | b'O') {
        t = &t[2..];
        if t.first() == Some(&b'_') {
            t = &t[1..];
        }
    }
    if t.is_empty() || t[0] == b'_' || t[t.len() - 1] == b'_' {
        return None;
    }
    let mut n: i128 = 0;
    let mut prev_underscore = false;
    for &c in t {
        match c {
            b'0'..=b'7' => {
                n = n * 8 + i128::from(c - b'0');
                prev_underscore = false;
            }
            b'_' if !prev_underscore => prev_underscore = true,
            _ => return None,
        }
    }
    Some(if neg { -n } else { n })
}

/// `calc_chksums(buf)`: the unsigned and signed sums, the checksum field counted as spaces.
fn chksums(buf: &[u8]) -> (i128, i128) {
    let bytes = buf[..148].iter().chain(&buf[156..]);
    let unsigned: i128 = bytes.clone().map(|&b| i128::from(b)).sum();
    let signed: i128 = bytes.map(|&b| i128::from(b as i8)).sum();
    (256 + unsigned, 256 + signed)
}

/// `_block(count)`.
fn block(n: i128) -> Result<i128, Exc> {
    if n < 0 {
        return Err(Exc::Invalid("invalid offset"));
    }
    let blocks = n / BLOCK as i128 + i128::from(n % BLOCK as i128 != 0);
    Ok(blocks.saturating_mul(BLOCK as i128))
}

fn to_u64(n: i128) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

/// `TarInfo._frombuf(buf, encoding, errors, dircheck=...)`.
fn frombuf(buf: &[u8], dircheck: bool) -> Result<Info, Exc> {
    if buf.is_empty() {
        return Err(Exc::Empty);
    }
    if buf.len() != BLOCK as usize {
        return Err(Exc::Truncated);
    }
    if buf.iter().all(|&b| b == 0) {
        return Err(Exc::Eof);
    }
    let chksum = nti(&buf[148..156])?;
    let (unsigned, signed) = chksums(buf);
    if chksum != unsigned && chksum != signed {
        return Err(Exc::Invalid("bad checksum"));
    }
    let mut name: Rc<[u8]> = Rc::from(nts(&buf[..100]));
    nti(&buf[100..108])?;
    nti(&buf[108..116])?;
    nti(&buf[116..124])?;
    let size = nti(&buf[124..136])?;
    nti(&buf[136..148])?;
    let mut typ = buf[156];
    nti(&buf[329..337])?;
    nti(&buf[337..345])?;
    let prefix = nts(&buf[345..500]);
    if dircheck && typ == b'\0' && name.ends_with(b"/") {
        typ = DIRTYPE;
    }
    let (mut sparse_ext, mut sparse_origsize) = (false, 0);
    if typ == b'S' {
        let mut pos = 386;
        for _ in 0..4 {
            nti(&buf[pos..pos + 12])?;
            nti(&buf[pos + 12..pos + 24])?;
            pos += 24;
        }
        sparse_ext = buf[482] != 0;
        sparse_origsize = nti(&buf[483..495])?;
    }
    if typ == DIRTYPE {
        name = rstrip_slash(&name);
    }
    if !prefix.is_empty() && !matches!(typ, b'L' | b'K' | b'S') {
        name = Rc::from([prefix, b"/", &name].concat());
    }
    Ok(Info {
        name,
        typ,
        size,
        offset_data: 0,
        sparse: None,
        sparse_ext,
        sparse_origsize,
    })
}

/// The PAX keywords that change what `_load_archive` sees (every other one only sets a field it
/// never reads): `path`, `size` and the GNU sparse ones.
const PAX_KEYS: [&[u8]; 8] = [
    b"path",
    b"size",
    b"GNU.sparse.name",
    b"GNU.sparse.size",
    b"GNU.sparse.realsize",
    b"GNU.sparse.map",
    b"GNU.sparse.major",
    b"GNU.sparse.minor",
];

#[derive(Clone, Debug)]
struct PaxVal {
    raw: Rc<[u8]>,
    /// `int(value)` (`None` is `ValueError`), saturated, for the size keywords.
    int: Option<i128>,
}

/// A `pax_headers` dict: its [`PAX_KEYS`], in dict order.
#[derive(Clone, Debug, Default)]
struct Pax(Vec<(&'static [u8], PaxVal)>);

impl Pax {
    fn set(&mut self, k: &[u8], v: &[u8]) {
        let Some(&key) = PAX_KEYS.iter().find(|&&x| x == k) else {
            return;
        };
        let int = if key.ends_with(b"size") {
            py_int(&String::from_utf8_lossy(v)).map(saturate)
        } else {
            None
        };
        let val = PaxVal { raw: v.into(), int };
        match self.0.iter_mut().find(|(x, _)| *x == key) {
            Some(e) => e.1 = val,
            None => self.0.push((key, val)),
        }
    }

    fn get(&self, k: &[u8]) -> Option<&PaxVal> {
        self.0.iter().find(|(x, _)| *x == k).map(|(_, v)| v)
    }

    fn raw(&self, k: &[u8]) -> Option<&[u8]> {
        self.get(k).map(|v| &*v.raw)
    }
}

fn saturate(i: Int) -> i128 {
    match i {
        Int::Small(n) => i128::from(n),
        Int::Big(s) if s.starts_with('-') => i128::MIN,
        Int::Big(_) => i128::MAX,
    }
}

/// `ValueError: invalid literal for int() with base 10: ...`.
fn bad_int(raw: &[u8]) -> Exc {
    fatal(format!(
        "invalid literal for int() with base 10: {}",
        repr_surrogateescape(raw)
    ))
}

fn out_of_range() -> Exc {
    fatal("sparse value out of range")
}

fn too_many_sparse() -> Exc {
    fatal("too many sparse entries")
}

/// A GNU sparse number from a str: `int(...)`, within `i64` (module docs).
fn sparse_int(raw: &[u8], invalid: impl Fn() -> Exc) -> Result<i128, Exc> {
    let text = std::str::from_utf8(raw).map_err(|_| invalid())?;
    match py_int(text) {
        None => Err(invalid()),
        Some(Int::Small(n)) => Ok(i128::from(n)),
        Some(Int::Big(_)) => Err(out_of_range()),
    }
}

/// `int(b)` of a bytes object (ASCII only), saturated.
fn ascii_int(b: &[u8]) -> Option<i128> {
    let text = std::str::from_utf8(b).ok().filter(|t| t.is_ascii())?;
    py_int(text).map(saturate)
}

/// `TarInfo._apply_pax_info(pax_headers, ...)`.
fn apply_pax_info(info: &mut Info, pax: &Pax) -> Result<(), Exc> {
    for (key, v) in &pax.0 {
        match *key {
            b"GNU.sparse.name" => info.name = v.raw.clone(),
            b"GNU.sparse.size" | b"GNU.sparse.realsize" => {
                info.size = v.int.ok_or_else(|| bad_int(&v.raw))?;
            }
            b"path" => info.name = rstrip_slash(&v.raw),
            b"size" => info.size = v.int.unwrap_or(0),
            _ => {}
        }
    }
    Ok(())
}

/// The `TarFile` of `tarfile.open(fileobj=_CappedReader(gz, STREAM_LIMIT), mode="r:")`.
struct Tar<'a> {
    f: CappedReader<Gz<'a>>,
    /// `TarFile.offset`: where the next header starts.
    offset: i128,
    /// `TarFile.pax_headers`: the global PAX headers.
    pax: Pax,
    firstmember: Option<Info>,
    /// `_fromtarfile` calls in this `next()`.
    depth: usize,
    /// Sparse pairs still allowed ([`MAX_SPARSE`]).
    sparse_left: usize,
}

impl<'a> Tar<'a> {
    /// `TarFile.__init__` in read mode: reads the first member.
    fn open(blob: &'a [u8]) -> Result<Tar<'a>, Rejected> {
        let mut t = Tar {
            f: CappedReader::new(Gz::new(blob), STREAM_LIMIT),
            offset: 0,
            pax: Pax::default(),
            firstmember: None,
            depth: 0,
            sparse_left: MAX_SPARSE,
        };
        t.firstmember = t.next_member()?;
        Ok(t)
    }

    /// `TarFile.next()`.
    fn next(&mut self) -> Result<Option<Info>, Rejected> {
        if let Some(m) = self.firstmember.take() {
            return Ok(Some(m));
        }
        self.next_member()
    }

    /// `TarFile.next()` once `firstmember` is used.
    fn next_member(&mut self) -> Result<Option<Info>, Rejected> {
        let tell = self.f.tell()?;
        if self.offset != i128::from(tell) {
            if self.offset == 0 {
                return Ok(None);
            }
            self.f.seek(self.offset - 1)?;
            if self.f.read(Some(1))?.is_empty() {
                return Err(not_readable("unexpected end of data"));
            }
        }
        self.depth = 0;
        // `self.offset` as the call left it: a nested `_proc_builtin` may have moved it.
        match self.fromtarfile(true) {
            Ok(info) => Ok(Some(info)),
            Err(Exc::Eof) => Ok(None),
            Err(Exc::Empty) if self.offset == 0 => Err(not_readable("empty file")),
            Err(e @ (Exc::Invalid(_) | Exc::Truncated)) if self.offset == 0 => {
                Err(not_readable(e.text()))
            }
            Err(Exc::Invalid(_) | Exc::Truncated | Exc::Empty) => Ok(None),
            Err(Exc::Subsequent(m)) => Err(not_readable(m)),
            // `raise ReadError(f'zlib error: {e}')` for a zlib.error.
            Err(Exc::Fatal(r)) => match r.0.strip_prefix(&not_readable(ZLIB).0) {
                Some(rest) => Err(not_readable(format!("zlib error: {ZLIB}{rest}"))),
                None => Err(r),
            },
        }
    }

    /// `TarInfo._fromtarfile(tarfile, dircheck=...)` and `_proc_member`.
    fn fromtarfile(&mut self, dircheck: bool) -> Result<Info, Exc> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(fatal("maximum recursion depth exceeded"));
        }
        let buf = self.f.read(Some(BLOCK))?;
        let info = frombuf(&buf, dircheck)?;
        match info.typ {
            b'L' | b'K' => self.proc_gnulong(info),
            b'S' => self.proc_sparse(info),
            b'x' | b'g' | b'X' => self.proc_pax(info),
            _ => self.proc_builtin(info),
        }
    }

    /// `_proc_builtin`.
    fn proc_builtin(&mut self, mut info: Info) -> Result<Info, Exc> {
        info.offset_data = self.f.tell()?;
        let mut offset = i128::from(info.offset_data);
        if has_data(info.typ) {
            offset = offset.saturating_add(block(info.size)?);
        }
        self.offset = offset;
        apply_pax_info(&mut info, &self.pax)?;
        if info.typ == DIRTYPE {
            info.name = rstrip_slash(&info.name);
        }
        Ok(info)
    }

    /// `_proc_gnulong`.
    fn proc_gnulong(&mut self, info: Info) -> Result<Info, Exc> {
        let buf = self.f.read(Some(to_u64(block(info.size)?)))?;
        let mut next = self.fromtarfile(false).map_err(subsequent)?;
        if info.typ == b'L' {
            next.name = Rc::from(nts(&buf));
        }
        if next.typ == DIRTYPE
            && let Some(n) = next.name.strip_suffix(b"/")
        {
            next.name = Rc::from(n);
        }
        Ok(next)
    }

    /// `_proc_sparse`: the extension blocks are parsed for their errors only; an `S` member is
    /// always refused afterwards, so its map is never kept.
    fn proc_sparse(&mut self, mut info: Info) -> Result<Info, Exc> {
        let mut extended = info.sparse_ext;
        while extended {
            let buf = self.f.read(Some(BLOCK))?;
            let mut pos = 0;
            for _ in 0..21 {
                nti(sub(&buf, pos, pos + 12))?;
                nti(sub(&buf, pos + 12, pos + 24))?;
                pos += 24;
            }
            extended = *buf.get(504).ok_or_else(|| fatal("index out of range"))? != 0;
        }
        info.offset_data = self.f.tell()?;
        self.offset = i128::from(info.offset_data).saturating_add(block(info.size)?);
        info.size = info.sparse_origsize;
        Ok(info)
    }

    /// `_proc_pax`.
    fn proc_pax(&mut self, info: Info) -> Result<Info, Exc> {
        let buf = self.f.read(Some(to_u64(block(info.size)?)))?;
        let global = info.typ == b'g';
        let mut pax = self.pax.clone();
        // The GNU.sparse.offset/numbytes records, in order, for `_proc_gnusparse_00`.
        let mut sparse_raw: Vec<(bool, &[u8])> = Vec::new();
        let mut sparse_overflow = false;
        let mut pos = 0;
        while pos < buf.len() && buf[pos] != 0 {
            // `_header_length_prefix_re`: `([0-9]{1,20}) ` at `pos`.
            let digits = buf[pos..]
                .iter()
                .take(21)
                .take_while(|b| b.is_ascii_digit())
                .count();
            if digits == 0 || digits > 20 || buf.get(pos + digits) != Some(&b' ') {
                return Err(INVALID);
            }
            let length = buf[pos..pos + digits]
                .iter()
                .fold(0u128, |n, &d| n * 10 + u128::from(d - b'0'));
            if length < 5 || pos as u128 + length > buf.len() as u128 {
                return Err(INVALID);
            }
            let end = pos + length as usize - 1;
            let kv = buf.get(pos + digits + 1..end).unwrap_or(&[]);
            let Some(eq) = kv.iter().position(|&b| b == b'=') else {
                return Err(INVALID);
            };
            let (key, value) = (&kv[..eq], &kv[eq + 1..]);
            if key.is_empty() || buf[end] != b'\n' {
                return Err(INVALID);
            }
            if key == b"GNU.sparse.offset" || key == b"GNU.sparse.numbytes" {
                if sparse_raw.len() <= 2 * MAX_SPARSE + 1 {
                    sparse_raw.push((key == b"GNU.sparse.offset", value));
                } else {
                    sparse_overflow = true;
                }
            }
            pax.set(key, value);
            pos += length as usize;
        }
        if global {
            self.pax = pax.clone();
        }
        let mut next = self.fromtarfile(false).map_err(subsequent)?;
        // A global header's dict is the live one: a nested global header changes it too.
        let pax = if global { self.pax.clone() } else { pax };
        if let Some(map) = pax.raw(b"GNU.sparse.map") {
            let mut numbers = Vec::new();
            for x in map.split(|&b| b == b',') {
                if numbers.len() > 2 * self.sparse_left + 1 {
                    return Err(too_many_sparse());
                }
                numbers.push(sparse_int(x, || bad_int(x))?);
            }
            next.sparse = Some(self.pairs(&numbers)?);
        } else if pax.get(b"GNU.sparse.size").is_some() {
            if sparse_overflow {
                return Err(too_many_sparse());
            }
            let (mut offsets, mut numbytes) = (Vec::new(), Vec::new());
            for &(is_offset, v) in &sparse_raw {
                let n = sparse_int(v, || INVALID)?;
                if is_offset {
                    offsets.push(n);
                } else {
                    numbytes.push(n);
                }
            }
            let pairs: Vec<(i128, i128)> = offsets.into_iter().zip(numbytes).collect();
            self.take_sparse(pairs.len())?;
            next.sparse = Some(pairs);
        } else if pax.raw(b"GNU.sparse.major") == Some(b"1")
            && pax.raw(b"GNU.sparse.minor") == Some(b"0")
        {
            self.gnusparse_10(&mut next)?;
        }
        if matches!(info.typ, b'x' | b'X') {
            apply_pax_info(&mut next, &pax)?;
            if pax.get(b"size").is_some() {
                let mut offset = i128::from(next.offset_data);
                if has_data(next.typ) {
                    offset = offset.saturating_add(block(next.size)?);
                }
                self.offset = offset;
            }
        }
        Ok(next)
    }

    /// `_proc_gnusparse_10`: the map is in the data, one decimal number per line.
    fn gnusparse_10(&mut self, next: &mut Info) -> Result<(), Exc> {
        let unpack = || fatal("not enough values to unpack (expected 2, got 1)");
        let mut buf = self.f.read(Some(BLOCK))?;
        let nl = buf.iter().position(|&b| b == b'\n').ok_or_else(unpack)?;
        let fields_raw: Vec<u8> = buf.drain(..=nl).take(nl).collect();
        let fields = ascii_int(&fields_raw).ok_or_else(|| bad_int(&fields_raw))?;
        let mut numbers = Vec::new();
        while (numbers.len() as i128) < fields.saturating_mul(2) {
            if numbers.len() > 2 * self.sparse_left + 1 {
                return Err(too_many_sparse());
            }
            if !buf.contains(&b'\n') {
                buf.extend(self.f.read(Some(BLOCK))?);
            }
            let nl = buf.iter().position(|&b| b == b'\n').ok_or_else(unpack)?;
            let raw: Vec<u8> = buf.drain(..=nl).take(nl).collect();
            match ascii_int(&raw) {
                None => return Err(bad_int(&raw)),
                Some(n) if i64::try_from(n).is_ok() => numbers.push(n),
                Some(_) => return Err(out_of_range()),
            }
        }
        next.offset_data = self.f.tell()?;
        next.sparse = Some(self.pairs(&numbers)?);
        Ok(())
    }

    /// `list(zip(sparse[::2], sparse[1::2]))`, counted against [`MAX_SPARSE`].
    fn pairs(&mut self, numbers: &[i128]) -> Result<Vec<(i128, i128)>, Exc> {
        let pairs: Vec<(i128, i128)> = numbers
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&[a, b]| (a, b))
            .collect();
        self.take_sparse(pairs.len())?;
        Ok(pairs)
    }

    fn take_sparse(&mut self, n: usize) -> Result<(), Exc> {
        self.sparse_left = self
            .sparse_left
            .checked_sub(n)
            .ok_or_else(too_many_sparse)?;
        Ok(())
    }

    /// `tar.extractfile(m).read(m.size + 1)` of a regular member: `ExFileObject` (a
    /// `BufferedReader`) over `_FileInFile`, read for read, since each read is a seek and a
    /// read of the gzip stream.
    fn extract(&mut self, m: &Info) -> Result<Vec<u8>, Rejected> {
        let mut file = FileInFile::new(m);
        let mut out = Vec::new();
        let mut remaining = file.size as u64 + 1;
        while remaining >= BUFFER as u64 {
            let got = self.file_read(&mut file, remaining - remaining % BUFFER as u64, &mut out)?;
            if got == 0 {
                return Ok(out);
            }
            remaining -= got;
        }
        let mut buffered = 0;
        while remaining > 0 && buffered < BUFFER {
            let got = self.file_read(&mut file, (BUFFER - buffered) as u64, &mut out)?;
            if got == 0 {
                break;
            }
            buffered += got as usize;
            remaining -= got;
        }
        Ok(out)
    }

    /// `_FileInFile.read(n)`, appended to `out`: how many bytes.
    fn file_read(
        &mut self,
        f: &mut FileInFile,
        n: u64,
        out: &mut Vec<u8>,
    ) -> Result<u64, Rejected> {
        let mut size = i128::from(n).min(f.size - f.position);
        let mut got = 0;
        while size > 0 {
            // The entry holding `position`, searched from the last one round the map as
            // Python does (entries can overlap). Every position is held by one.
            let mut found = false;
            for _ in 0..f.map.len() {
                let (_, start, stop, _) = f.map[f.index];
                if start <= f.position && f.position < stop {
                    found = true;
                    break;
                }
                f.index = (f.index + 1) % f.map.len();
            }
            if !found {
                return Err(not_readable("unexpected end of data"));
            }
            let (data, start, stop, offset) = f.map[f.index];
            let length = size.min(stop - f.position);
            if data {
                self.f.seek(offset + (f.position - start))?;
                let b = self.f.read(Some(length as u64))?;
                if b.len() as i128 != length {
                    return Err(not_readable("unexpected end of data"));
                }
                out.extend_from_slice(&b);
            } else {
                out.resize(out.len() + length as usize, 0);
            }
            size -= length;
            f.position += length;
            got += length as u64;
        }
        Ok(got)
    }
}

/// `_FileInFile`: a member's data, as runs of the stream and runs of zeros.
struct FileInFile {
    size: i128,
    /// `(data, start, stop, where the data is in the stream)`.
    map: Vec<(bool, i128, i128, i128)>,
    index: usize,
    position: i128,
}

impl FileInFile {
    fn new(m: &Info) -> FileInFile {
        let size = m.size;
        let whole = [(0, size)];
        let blocks = m.sparse.as_deref().unwrap_or(&whole);
        let mut map = Vec::new();
        let (mut lastpos, mut realpos) = (0i128, i128::from(m.offset_data));
        for &(offset, n) in blocks {
            if offset > lastpos {
                map.push((false, lastpos, offset, 0));
            }
            map.push((true, offset, offset + n, realpos));
            realpos += n;
            lastpos = offset + n;
        }
        if lastpos < size {
            map.push((false, lastpos, size, 0));
        }
        FileInFile {
            size,
            map,
            index: 0,
            position: 0,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// _load_archive
// ---------------------------------------------------------------------------------------------

/// `_read_archive_bytes(archive)`: the file's bytes, read once, after `fstat` showed a regular
/// file of at most [`MAX_ARCHIVE`] bytes.
fn read_archive_bytes(archive: &Path) -> Result<Vec<u8>, Rejected> {
    let cannot = |e: io::Error| {
        let text = match PyExc::from(e) {
            PyExc::OSError { errno, .. } => errno_tables::strerror(errno),
            other => other.py_str(),
        };
        Rejected(format!("cannot read the archive: {text}"))
    };
    let larger = || {
        Rejected(format!(
            "the archive is larger than {} MiB",
            MAX_ARCHIVE >> 20
        ))
    };
    let mut f = OpenOptions::new()
        .read(true)
        .custom_flags(O_NONBLOCK)
        .open(archive)
        .map_err(cannot)?;
    let meta = f.metadata().map_err(cannot)?;
    // `os.fdopen` (FileIO) refuses a folder itself.
    if meta.is_dir() {
        return Err(Rejected(format!(
            "cannot read the archive: {}",
            errno_tables::strerror(EISDIR)
        )));
    }
    if !meta.file_type().is_file() {
        return Err(Rejected("the archive is not a regular file".to_string()));
    }
    if meta.len() > MAX_ARCHIVE {
        return Err(larger());
    }
    let mut blob = Vec::new();
    (&mut f)
        .take(MAX_ARCHIVE + 1)
        .read_to_end(&mut blob)
        .map_err(cannot)?;
    if blob.len() as u64 > MAX_ARCHIVE {
        return Err(larger());
    }
    Ok(blob)
}

/// The members of the gzip'd tar `blob`, validated: `{name: bytes}` of the regular ones.
fn read_members(blob: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, Rejected> {
    let mut tar = Tar::open(blob)?;
    let mut members: Vec<Info> = Vec::new();
    let mut names: HashSet<Rc<[u8]>> = HashSet::new();
    let mut total: i128 = 0;
    while let Some(m) = tar.next()? {
        if members.len() >= MAX_MEMBERS {
            return Err(Rejected(format!(
                "the archive has more than {MAX_MEMBERS} members"
            )));
        }
        let kind = check_name_b(&m.name)?;
        let r = repr_surrogateescape(&m.name);
        if !names.insert(m.name.clone()) {
            return Err(Rejected(format!("duplicate member: {r}")));
        }
        if !matches!(m.typ, b'0' | b'\0' | DIRTYPE) {
            return Err(Rejected(format!(
                "member {r} is not a plain file or folder"
            )));
        }
        if (kind == Kind::Dir) != (m.typ == DIRTYPE) {
            return Err(Rejected(format!("member {r} has the wrong type")));
        }
        if m.typ == DIRTYPE && m.size != 0 {
            return Err(Rejected(format!("folder member {r} has data")));
        }
        if m.size < 0 || (kind == Kind::Manifest && m.size > i128::from(MAX_MANIFEST)) {
            return Err(Rejected(format!("member {r} has a bad size")));
        }
        total = total.saturating_add(m.size);
        if total > i128::from(MAX_TOTAL) {
            return Err(Rejected(format!(
                "the notes add up to more than {} MiB",
                MAX_TOTAL >> 20
            )));
        }
        members.push(m);
    }
    // tarfile.data_filter (3.12): for a member that passed the checks above, the one refusal
    // left is `os.path.realpath` under its temporary folder failing with ENAMETOOLONG.
    for m in &members {
        if FILTER_DEST_LEN + m.name.len() >= PATH_MAX {
            return Err(Rejected(format!(
                "member {} rejected by the tar filter: [Errno {ENAMETOOLONG}] {}",
                repr_surrogateescape(&m.name),
                errno_tables::strerror(ENAMETOOLONG)
            )));
        }
    }
    let mut contents = BTreeMap::new();
    for m in &members {
        if !is_reg(m.typ) {
            continue;
        }
        let data = tar.extract(m)?;
        if std::str::from_utf8(&data).is_err() {
            return Err(Rejected(format!(
                "member {} is not valid UTF-8",
                repr_surrogateescape(&m.name)
            )));
        }
        contents.insert(String::from_utf8_lossy(&m.name).into_owned(), data);
    }
    Ok(contents)
}

/// `manifest.get("format") != FORMAT` is false: `1`, `1.0` or `true` (Python's `==`).
fn is_format(v: Option<&Value>) -> bool {
    match v {
        Some(Value::Int(Int::Small(n))) => *n == FORMAT,
        Some(Value::Float(f)) => *f == FORMAT as f64,
        Some(Value::Bool(b)) => *b,
        _ => false,
    }
}

/// `_load_archive(archive)`: validates the whole archive, then returns it. Never writes.
pub fn load_archive(archive: &Path) -> Result<Loaded, Rejected> {
    load_blob(&read_archive_bytes(archive)?)
}

/// [`load_archive`] of the archive's bytes.
fn load_blob(blob: &[u8]) -> Result<Loaded, Rejected> {
    let sha256 = Sha256::digest(blob)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let contents = read_members(blob)?;
    let Some(raw) = contents.get("manifest.json") else {
        return Err(Rejected("manifest.json is missing".to_string()));
    };
    let parsed = std::str::from_utf8(raw)
        .ok()
        .and_then(|text| json::loads(text).ok());
    let Some(manifest) = parsed else {
        return Err(Rejected("manifest.json is not valid JSON".to_string()));
    };
    match manifest.into_object() {
        Some(manifest) if is_format(manifest.get("format")) => Ok(Loaded {
            manifest,
            contents,
            sha256,
        }),
        _ => Err(Rejected("unsupported archive format".to_string())),
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use flate2::Compression;
    use flate2::write::GzEncoder;

    use super::*;

    const MANIFEST: &[u8] = br#"{"format": 1, "date": "2026-01-01", "counts": {}}"#;
    const END: [u8; 1024] = [0; 1024];

    fn octf(n: u64, width: usize) -> Vec<u8> {
        let mut v = format!("{n:0w$o}", w = width - 1).into_bytes();
        v.push(0);
        v
    }

    /// The checksum field set for `h`.
    fn fixsum(mut h: Vec<u8>) -> Vec<u8> {
        h[148..156].copy_from_slice(b"        ");
        let sum: u32 = h.iter().map(|&b| u32::from(b)).sum();
        h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        h
    }

    /// A ustar header block.
    fn hdr(name: &[u8], size: u64, typ: u8) -> Vec<u8> {
        let mut h = vec![0u8; 512];
        h[..name.len().min(100)].copy_from_slice(&name[..name.len().min(100)]);
        h[100..108].copy_from_slice(&octf(0o644, 8));
        h[108..116].copy_from_slice(&octf(0, 8));
        h[116..124].copy_from_slice(&octf(0, 8));
        h[124..136].copy_from_slice(&octf(size, 12));
        h[136..148].copy_from_slice(&octf(0, 12));
        h[156] = typ;
        h[257..265].copy_from_slice(b"ustar\x0000");
        h[329..337].copy_from_slice(&octf(0, 8));
        h[337..345].copy_from_slice(&octf(0, 8));
        fixsum(h)
    }

    /// `h` with `bytes` at `at`, checksum fixed.
    fn with(mut h: Vec<u8>, at: usize, bytes: &[u8]) -> Vec<u8> {
        h[at..at + bytes.len()].copy_from_slice(bytes);
        fixsum(h)
    }

    fn pad(data: &[u8]) -> Vec<u8> {
        let mut v = data.to_vec();
        v.resize(data.len().div_ceil(512) * 512, 0);
        v
    }

    fn entry(h: Vec<u8>, data: &[u8]) -> Vec<u8> {
        [h, pad(data)].concat()
    }

    fn member(name: &[u8], data: &[u8]) -> Vec<u8> {
        entry(hdr(name, data.len() as u64, b'0'), data)
    }

    fn manifest() -> Vec<u8> {
        member(b"manifest.json", MANIFEST)
    }

    /// A PAX header of type `typ` with `records`.
    fn pax(typ: u8, records: &[(&[u8], &[u8])]) -> Vec<u8> {
        let mut body = Vec::new();
        for (k, v) in records {
            let rec = [b" ", *k, b"=", *v, b"\n"].concat();
            let mut n = rec.len() + 1;
            while n.to_string().len() + rec.len() != n {
                n += 1;
            }
            body.extend(n.to_string().into_bytes());
            body.extend(rec);
        }
        entry(hdr(b"PaxHeader/x", body.len() as u64, typ), &body)
    }

    fn gnulong(typ: u8, name: &[u8]) -> Vec<u8> {
        let data = [name, b"\0"].concat();
        entry(hdr(b"././@LongLink", data.len() as u64, typ), &data)
    }

    fn gz(data: &[u8]) -> Vec<u8> {
        let mut e = GzEncoder::new(Vec::new(), Compression::fast());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    /// `_load_archive` of the gzip'd `tar`: the member names, or the rejection.
    fn load(tar: &[u8]) -> Result<Vec<String>, String> {
        load_gz(&gz(tar))
    }

    fn load_gz(blob: &[u8]) -> Result<Vec<String>, String> {
        load_blob(blob)
            .map(|l| l.contents.into_keys().collect())
            .map_err(|r| r.0)
    }

    fn names(v: &[&str]) -> Result<Vec<String>, String> {
        Ok(v.iter().map(|s| (*s).to_string()).collect())
    }

    fn rejected(m: &str) -> Result<Vec<String>, String> {
        Err(m.to_string())
    }

    fn good() -> Vec<u8> {
        [manifest(), member(b"personal/good.md", b"good\n")].concat()
    }

    // --- names ------------------------------------------------------------------------------

    #[test]
    fn names_with_a_newline_anywhere_are_not_note_names() {
        for bad in ["foo\n.md", "foo.md\n", "\nfoo.md", "foo.md\r", "fo o.md"] {
            assert!(!ok_note_name(bad), "{bad:?}");
        }
        assert!(!name_ok("ag\n"));
        assert!(!name_ok("\nag"));
        assert!(name_ok("ag"));
        assert!(ok_note_name("foo.md"));
    }

    #[test]
    fn memory_md_in_any_case_is_not_a_note_name() {
        for bad in [
            "memory.md",
            "MEMORY.md",
            "Memory.md",
            "mEmOrY.md",
            "memory.MD",
            "MEMORY.MD",
        ] {
            assert!(!ok_note_name(bad), "{bad:?}");
        }
        for good in [
            "memory-x.md",
            "memory.md.md",
            "xmemory.md",
            "a.md",
            "A_b-c.d.md",
            "1.md",
        ] {
            assert!(ok_note_name(good), "{good:?}");
        }
        for bad in [
            ".md",
            "-a.md",
            "_a.md",
            ".a.md",
            "a.MD",
            "a.md.txt",
            "\u{e9}.md",
            "a",
            "",
        ] {
            assert!(!ok_note_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn name_pattern_is_ascii_and_anchored() {
        for good in ["a", "A", "0", "a.b", "a-b_c.d", "9_."] {
            assert!(name_ok(good), "{good:?}");
        }
        for bad in [
            "",
            ".",
            "-a",
            "_a",
            ".a",
            "a/b",
            "a b",
            "a\0",
            "\u{e9}",
            "a\u{e9}",
            "a\n",
            "K\u{212a}",
        ] {
            assert!(!name_ok(bad), "{bad:?}");
        }
    }

    #[test]
    fn check_name_long_tail() {
        use Kind::{Dir, Manifest, Note};
        let ok = [
            ("manifest.json", Manifest),
            ("personal", Dir),
            ("repo", Dir),
            ("repo/memory", Dir),
            ("repo/agent-memory", Dir),
            ("repo/agent-memory/a1", Dir),
            // A name with a dot is an agent folder name, so this is a folder (refused later as
            // a file of the wrong type).
            ("repo/agent-memory/x.md", Dir),
            ("personal/a.md", Note),
            ("repo/memory/b.md", Note),
            ("repo/agent-memory/x/c.md", Note),
        ];
        for (name, kind) in ok {
            assert_eq!(check_name(name), Ok(kind), "{name:?}");
        }
        let unsafe_names = [
            "../evil.md",
            "personal/../../evil.md",
            "repo/agent-memory/../x.md",
            "/etc/evil.md",
            "/personal/a.md",
            "personal//x.md",
            "personal/./x.md",
            "./personal/a.md",
            "personal/",
            "personal/a.md/",
            "",
            ".",
            "..",
            "personal\\x.md",
            "personal\\..\\x.md",
            "personal/x\0.md",
        ];
        for name in unsafe_names {
            let want = format!(
                "unsafe member name: {}",
                anthology_pycompat::str::repr(name)
            );
            assert_eq!(check_name(name), Err(Rejected(want)), "{name:?}");
        }
        let outside = [
            "repo/settings.json",
            "personal/sub/x.md",
            "other/x.md",
            ".claude/settings.json",
            "personal/MEMORY.md",
            "personal/memory.md",
            "repo/memory/MEMORY.md",
            "repo/agent-memory/a/MEMORY.md",
            "personal/x.txt",
            "personal/.x.md",
            "repo/agent-memory/.hid/x.md",
            "repo/agent-memory/ag\n/bar.md",
            "repo/agent-memory/ag\n",
            "personal/foo\n.md",
            "personal/foo.md\n",
            "Personal/a.md",
            "repo/agent-memory/a/b/c.md",
            "repo/memory/sub/x.md",
            "repo/memory/a",
            "manifest.json/x",
            "Manifest.json",
            "personal/a b.md",
            "personal/\u{e9}.md",
            "repo/agent",
        ];
        for name in outside {
            let want = format!(
                "member outside the memory locations: {}",
                anthology_pycompat::str::repr(name)
            );
            assert_eq!(check_name(name), Err(Rejected(want)), "{name:?}");
        }
    }

    #[test]
    fn check_name_messages_are_python_repr() {
        assert_eq!(
            check_name("personal/it's.md").unwrap_err().0,
            r#"member outside the memory locations: "personal/it's.md""#
        );
        assert_eq!(
            check_name("a\nb").unwrap_err().0,
            r"member outside the memory locations: 'a\nb'"
        );
        // tarfile decodes names with surrogateescape: a byte that is not UTF-8 is \udcXX.
        assert_eq!(
            check_name_b(b"personal/\xff.md").unwrap_err().0,
            r"member outside the memory locations: 'personal/\udcff.md'"
        );
        assert_eq!(
            check_name_b(b"../\xfe").unwrap_err().0,
            r"unsafe member name: '../\udcfe'"
        );
    }

    // --- the capped reader ------------------------------------------------------------------

    #[test]
    fn capped_reader_stops_at_one_byte_past_the_limit() {
        let mut c = CappedReader::new(Cursor::new(vec![0u8; 1000]), 50);
        assert_eq!(c.read(Some(10u64.pow(9))), Err(cap()));
        assert!(c.get_ref().position() <= 51);
        let mut c = CappedReader::new(Cursor::new(vec![0u8; 1000]), 50);
        assert_eq!(c.read(None), Err(cap()));
        assert!(c.get_ref().position() <= 51);
        let mut c = CappedReader::new(Cursor::new(b"abc".repeat(30)), 50);
        assert_eq!(c.read(Some(10)).unwrap(), b"abcabcabca");
        assert_eq!(c.seek(51), Err(cap()));
        assert_eq!(c.seek(50), Ok(50));
        assert_eq!(c.read(Some(1)), Err(cap()));
        // Exactly up to the limit is fine; a short stream is a short read, not a rejection.
        let mut c = CappedReader::new(Cursor::new(vec![7u8; 50]), 50);
        assert_eq!(c.read(None).unwrap().len(), 50);
        let mut c = CappedReader::new(Cursor::new(vec![7u8; 20]), 50);
        assert_eq!(c.read(Some(1000)).unwrap().len(), 20);
    }

    /// An endless stream of zeros that remembers the most bytes one read call asked for.
    struct Zeros {
        pos: u64,
        largest: usize,
    }

    impl Read for Zeros {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            self.largest = self.largest.max(out.len());
            out.fill(0);
            self.pos += out.len() as u64;
            Ok(out.len())
        }
    }

    impl Seek for Zeros {
        fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
            if let SeekFrom::Start(p) = to {
                self.pos = p;
            }
            Ok(self.pos)
        }
    }

    #[test]
    fn a_read_past_the_limit_is_measured_not_buffered() {
        let mut c = CappedReader::new(Zeros { pos: 0, largest: 0 }, STREAM_LIMIT);
        assert_eq!(c.read(Some(80 * 1024 * 1024)), Err(cap()));
        assert_eq!(c.get_ref().pos, STREAM_LIMIT + 1);
        assert!(c.get_ref().largest <= 8192);
    }

    // --- gzip -------------------------------------------------------------------------------

    #[test]
    fn gzip_is_read_lazily_like_gzipfile() {
        let tar = [good(), END.to_vec()].concat();
        let ok = names(&["manifest.json", "personal/good.md"]);
        let g = gz(&tar);
        assert_eq!(load_gz(&g), ok);
        // Never read: trailing garbage, a bad CRC, a missing trailer (tarfile stops at the
        // first zero block).
        assert_eq!(load_gz(&[g.clone(), b"garbage".to_vec()].concat()), ok);
        let mut bad_crc = g.clone();
        let n = bad_crc.len();
        bad_crc[n - 8] ^= 0xff;
        assert_eq!(load_gz(&bad_crc), ok);
        assert_eq!(load_gz(&g[..n - 8]), ok);
        // Concatenated members, zero padding between them.
        let split = [
            gz(&manifest()),
            vec![0, 0, 0],
            gz(&member(b"personal/good.md", b"good\n")),
            gz(&END),
        ]
        .concat();
        assert_eq!(load_gz(&split), ok);
        // ...but a read past a member's end does check it, and what follows it.
        let mut first = gz(&manifest());
        let m = first.len();
        first[m - 8] ^= 0xff;
        let err = load_gz(
            &[
                first,
                gz(&[member(b"personal/good.md", b"g\n"), END.to_vec()].concat()),
            ]
            .concat(),
        );
        assert!(
            err.unwrap_err()
                .starts_with("not a readable memory archive: CRC check failed")
        );
        let junk = [gz(&manifest()), b"junk".to_vec(), gz(&END)].concat();
        assert_eq!(
            load_gz(&junk),
            rejected("not a readable memory archive: Not a gzipped file (b'ju')")
        );
        // Truncated where tarfile still reads: refused.
        assert_eq!(
            load_gz(&g[..g.len() / 2]),
            rejected(&format!("not a readable memory archive: {GZ_EOF}"))
        );
        assert_eq!(
            load_gz(b""),
            rejected("not a readable memory archive: empty file")
        );
        assert_eq!(
            load_gz(b"this is not an archive"),
            rejected("not a readable memory archive: Not a gzipped file (b'th')")
        );
        assert_eq!(
            load_gz(b"\x1f\x8b\x07\0\0\0\0\0\0\0"),
            rejected("not a readable memory archive: Unknown compression method")
        );
    }

    #[test]
    fn a_match_reaching_before_the_member_start_is_refused_as_zlib_does() {
        // A fixed-Huffman block: literal 'a', then a match of length 3 at distance 2 (one
        // byte back is all there is), then end of block.
        let deflate = far_back_deflate();
        let mut g = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0xff];
        g.extend(&deflate);
        // As in Python, the read that has zlib decode the bad match fails, even for one byte.
        let mut r = Gz::new(&g);
        assert_eq!(
            r.read_n(1, true).unwrap_err().to_string(),
            format!("{ZLIB}invalid distance too far back")
        );
        // A valid stream of the same shape reads.
        let mut ok = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0xff];
        ok.extend(fixed_deflate(1));
        assert_eq!(Gz::new(&ok).read_n(4, true).unwrap().0, b"aaaa");
    }

    #[test]
    fn corrupt_data_fails_the_read_whose_zlib_call_meets_it() {
        // A stored block of 20000 bytes, then a block of the invalid type 3. GzipFile reads
        // 16384 bytes in one call (output full: zlib stops before the bad block); its next
        // call may produce up to 24576, so it meets the bad block (checked on the 3.12 image).
        let mut blob = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0xff, 0];
        blob.extend(20000u16.to_le_bytes());
        blob.extend((!20000u16).to_le_bytes());
        blob.extend([b'a'; 20000]);
        blob.push(0x07);
        let mut g = Gz::new(&blob);
        assert_eq!(g.read_n(16384, true).unwrap().1, 16384);
        let err = g.read_n(1, true).unwrap_err().to_string();
        assert!(err.starts_with(ZLIB), "{err}");
        assert!(Gz::new(&blob).read_n(16385, false).is_err());
        assert!(Gz::new(&blob).read_n(20000, false).is_err());
        // A seek reads in calls of at most 8192 up to the target: only the call that ends
        // right before the bad block meets it (zlib goes on to a header with a full output).
        for pos in [16384, 16385, 19999] {
            assert_eq!(Gz::new(&blob).seek_to(pos).unwrap(), pos);
        }
        assert!(Gz::new(&blob).seek_to(20000).is_err());
    }

    /// Deflate bits for: fixed block, literal `a`, length 3 distance 2, end of block.
    fn far_back_deflate() -> Vec<u8> {
        fixed_deflate(2)
    }

    /// Fixed block: literal `a`, then a match of length 3 at `distance` (1 or 2), end of block.
    fn fixed_deflate(distance: u32) -> Vec<u8> {
        let mut bits: Vec<u8> = Vec::new();
        let mut put = |v: u32, n: u32, reversed: bool| {
            for i in 0..n {
                let bit = if reversed {
                    (v >> (n - 1 - i)) & 1
                } else {
                    (v >> i) & 1
                };
                bits.push(bit as u8);
            }
        };
        put(1, 1, false); // BFINAL
        put(1, 2, false); // BTYPE fixed
        put(0x30 + u32::from(b'a'), 8, true); // literal 'a' (codes 0x30..0xbf)
        put(1, 7, true); // length code 257 = length 3 (7-bit codes start at 256 = 0)
        put(distance - 1, 5, true); // distance code 0 = distance 1, code 1 = distance 2
        put(0, 7, true); // end of block (256)
        bits.chunks(8)
            .map(|c| c.iter().enumerate().fold(0u8, |b, (i, &x)| b | (x << i)))
            .collect()
    }

    // --- tar structure ----------------------------------------------------------------------

    #[test]
    fn the_first_bad_or_empty_header_is_an_error_a_later_one_ends_the_archive() {
        let ok = names(&["manifest.json", "personal/good.md"]);
        let mut bad = hdr(b"personal/b.md", 3, b'0');
        bad[148..156].copy_from_slice(b"0000000\0");
        assert_eq!(
            load(&[good(), bad.clone(), pad(b"abc"), END.to_vec()].concat()),
            ok
        );
        assert_eq!(
            load(&[bad, pad(b"abc")].concat()),
            rejected("not a readable memory archive: bad checksum")
        );
        let bad_mode = with(hdr(b"personal/b.md", 3, b'0'), 100, b"9999999\0");
        assert_eq!(load(&[good(), bad_mode.clone(), pad(b"abc")].concat()), ok);
        assert_eq!(
            load(&bad_mode),
            rejected("not a readable memory archive: invalid header")
        );
        // A zero block first is an empty archive.
        assert_eq!(
            load(&[END.to_vec(), good()].concat()),
            rejected("manifest.json is missing")
        );
        // No end blocks, a short end, junk at the end: all end the archive quietly.
        assert_eq!(load(&good()), ok);
        assert_eq!(load(&[good(), vec![0; 100]].concat()), ok);
        assert_eq!(load(&[good(), vec![b'x'; 100]].concat()), ok);
        assert_eq!(
            load(&manifest()[..300]),
            rejected("not a readable memory archive: truncated header")
        );
        // Data cut short: the next header read finds the end of the stream.
        let short = [
            manifest(),
            hdr(b"personal/b.md", 600, b'0'),
            b"abc".to_vec(),
        ]
        .concat();
        assert_eq!(
            load(&short),
            rejected("not a readable memory archive: unexpected end of data")
        );
        // The signed checksum is accepted too.
        let mut h = hdr(b"personal/\xe9.md", 1, b'0');
        h[148..156].copy_from_slice(b"        ");
        let signed: i32 = h.iter().map(|&b| i32::from(b as i8)).sum();
        h[148..156].copy_from_slice(format!("{signed:06o}\0 ").as_bytes());
        let err = load(&[manifest(), h, pad(b"x")].concat()).unwrap_err();
        assert!(
            err.starts_with("member outside the memory locations"),
            "{err}"
        );
    }

    #[test]
    fn links_devices_and_other_types_are_refused() {
        for t in *b"123467SVZ" {
            let tar = [good(), entry(hdr(b"personal/t.md", 0, t), b"")].concat();
            assert_eq!(
                load(&tar),
                rejected("member 'personal/t.md' is not a plain file or folder"),
                "{}",
                char::from(t)
            );
        }
        let link_dir = [good(), with(hdr(b"repo", 0, b'2'), 157, b"/etc")].concat();
        assert_eq!(
            load(&link_dir),
            rejected("member 'repo' is not a plain file or folder")
        );
        let file_as_dir = [good(), member(b"repo/agent-memory/x.md", b"x")].concat();
        assert_eq!(
            load(&file_as_dir),
            rejected("member 'repo/agent-memory/x.md' has the wrong type")
        );
        let dir_as_file = [good(), entry(hdr(b"personal/d.md", 0, DIRTYPE), b"")].concat();
        assert_eq!(
            load(&dir_as_file),
            rejected("member 'personal/d.md' has the wrong type")
        );
        let dir_data = [good(), entry(hdr(b"repo", 3, DIRTYPE), b"abc")].concat();
        assert_eq!(load(&dir_data), rejected("folder member 'repo' has data"));
    }

    #[test]
    fn folders_v7_slashes_and_the_ustar_prefix() {
        let tar = [
            good(),
            entry(hdr(b"repo/", 0, b'\0'), b""),
            entry(hdr(b"repo/memory//", 0, DIRTYPE), b""),
            with(hdr(b"x.md", 1, b'0'), 345, b"repo/memory"),
            pad(b"x"),
        ]
        .concat();
        assert_eq!(
            load(&tar),
            names(&["manifest.json", "personal/good.md", "repo/memory/x.md"])
        );
        // A regular file named with a slash is not a folder.
        let tar = [good(), entry(hdr(b"repo/", 0, b'0'), b"")].concat();
        assert_eq!(load(&tar), rejected("unsafe member name: 'repo/'"));
    }

    #[test]
    fn duplicate_members_are_refused() {
        let tar = [good(), member(b"personal/good.md", b"again")].concat();
        assert_eq!(load(&tar), rejected("duplicate member: 'personal/good.md'"));
        let tar = [
            good(),
            entry(hdr(b"personal", 0, DIRTYPE), b""),
            entry(hdr(b"personal/", 0, DIRTYPE), b""),
        ]
        .concat();
        assert_eq!(load(&tar), rejected("duplicate member: 'personal'"));
    }

    // --- number fields ----------------------------------------------------------------------

    #[test]
    fn octal_fields_follow_python_int_base_8() {
        let cases: [(&[u8], Option<i128>); 16] = [
            (b"0000644", Some(0o644)),
            (b" 644 ", Some(0o644)),
            (b"", Some(0)),
            (b"   ", Some(0)),
            (b"0o17", Some(15)),
            (b"0O17", Some(15)),
            (b"0o_17", Some(15)),
            (b"1_7", Some(15)),
            (b"+7", Some(7)),
            (b" -0o1_7 ", Some(-15)),
            (b"\x1c7\x1f", Some(7)),
            (b"8", None),
            (b"0o", None),
            (b"1__7", None),
            (b"17_", None),
            (b"\xc3\xa97", None),
        ];
        for (text, want) in cases {
            assert_eq!(octal(text), want, "{text:?}");
        }
        assert_eq!(octal(b"_17"), None);
        assert_eq!(octal(b"--7"), None);
        assert_eq!(octal(b"7 7"), None);
    }

    #[test]
    fn base_256_fields_and_sizes_cannot_overflow() {
        assert_eq!(nti(&[0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3]).unwrap(), 3);
        assert_eq!(nti(&[0xff; 12]).unwrap(), -1);
        assert_eq!(
            nti(&[
                0x80, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff
            ])
            .unwrap(),
            (1i128 << 88) - 1
        );
        assert!(matches!(nti(b""), Err(Exc::Fatal(_))));
        // A member of base-256 size 3 loads; a negative one is a bad size; the largest one is
        // over the total, before anything is read.
        let size3 = with(
            hdr(b"personal/b.md", 0, b'0'),
            124,
            &[0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3],
        );
        assert_eq!(
            load(&[manifest(), size3, pad(b"abc")].concat()),
            names(&["manifest.json", "personal/b.md"])
        );
        // A negative size has no block count: `_block` raises, which ends the archive.
        let neg = with(hdr(b"personal/b.md", 0, b'0'), 124, &[0xff; 12]);
        assert_eq!(load(&[manifest(), neg].concat()), names(&["manifest.json"]));
        // Set after the offset is computed (a global GNU.sparse.size), it is a bad size.
        let tar = [
            manifest(),
            pax(b'g', &[(b"GNU.sparse.size", b"-1")]),
            member(b"personal/b.md", b""),
        ]
        .concat();
        assert_eq!(
            load(&tar),
            rejected("member 'personal/b.md' has a bad size")
        );
        let huge = with(
            hdr(b"personal/b.md", 0, b'0'),
            124,
            &[
                0x80, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
            ],
        );
        assert_eq!(
            load(&[manifest(), huge].concat()),
            rejected("the notes add up to more than 50 MiB")
        );
        assert_eq!(block(-1).unwrap_err().text(), "invalid offset");
        assert_eq!(block(i128::MAX).unwrap(), i128::MAX);
    }

    #[test]
    fn the_manifest_limit_and_the_total() {
        let big = [member(b"manifest.json", &vec![b' '; 1024 * 1024 + 1])].concat();
        assert_eq!(
            load(&big),
            rejected("member 'manifest.json' has a bad size")
        );
        // The declared size is enough: no data follows.
        let declared = [manifest(), hdr(b"personal/big.md", 5 << 30, b'0')].concat();
        assert_eq!(
            load(&declared),
            rejected("the notes add up to more than 50 MiB")
        );
        let parts = [
            manifest(),
            hdr(b"personal/a.md", 30 << 20, b'0'),
            vec![0; 30 << 20],
            hdr(b"personal/b.md", 21 << 20, b'0'),
        ]
        .concat();
        assert_eq!(
            load(&parts),
            rejected("the notes add up to more than 50 MiB")
        );
    }

    // --- GNU and PAX headers ----------------------------------------------------------------

    #[test]
    fn gnu_longname_and_longlink() {
        let tar = [
            manifest(),
            gnulong(b'L', b"personal/long.md"),
            member(b"ignored", b"x"),
            gnulong(b'K', b"/etc/passwd"),
            member(b"personal/k.md", b"k"),
        ]
        .concat();
        assert_eq!(
            load(&tar),
            names(&["manifest.json", "personal/k.md", "personal/long.md"])
        );
        let dir = [
            manifest(),
            gnulong(b'L', b"repo/"),
            entry(hdr(b"zz", 0, DIRTYPE), b""),
        ]
        .concat();
        assert_eq!(load(&dir), names(&["manifest.json"]));
        let then_end = [manifest(), gnulong(b'L', b"personal/a.md")].concat();
        assert_eq!(
            load(&then_end),
            rejected("not a readable memory archive: empty header")
        );
        let neg = with(hdr(b"././@LongLink", 0, b'L'), 124, b"-1\0");
        assert_eq!(
            load(&[good(), neg].concat()),
            names(&["manifest.json", "personal/good.md"])
        );
    }

    #[test]
    fn header_chains_stop_where_python_runs_out_of_recursion() {
        for (k, ok) in [(MAX_DEPTH - 1, true), (MAX_DEPTH, false)] {
            let mut tar = manifest();
            for _ in 0..k {
                tar.extend(gnulong(b'L', b"personal/c.md"));
            }
            tar.extend(member(b"x", b"c"));
            let got = load(&tar);
            if ok {
                assert_eq!(got, names(&["manifest.json", "personal/c.md"]));
            } else {
                assert_eq!(
                    got,
                    rejected("not a readable memory archive: maximum recursion depth exceeded")
                );
            }
        }
        let mut tar = manifest();
        for _ in 0..MAX_DEPTH {
            tar.extend(pax(b'x', &[(b"path", b"personal/c.md")]));
        }
        tar.extend(member(b"x", b"c"));
        assert_eq!(
            load(&tar),
            rejected("not a readable memory archive: maximum recursion depth exceeded")
        );
    }

    #[test]
    fn pax_path_and_size() {
        let tar = [
            manifest(),
            pax(b'x', &[(b"path", b"personal/p.md///")]),
            member(b"x", b"pp"),
        ]
        .concat();
        assert_eq!(load(&tar), names(&["manifest.json", "personal/p.md"]));
        // `size` overrides the header, and moves the next header.
        let tar = [
            manifest(),
            pax(b'x', &[(b"size", b"6")]),
            hdr(b"personal/s.md", 0, b'0'),
            pad(b"abcdef"),
            member(b"personal/t.md", b"t"),
        ]
        .concat();
        let got = load_blob(&gz(&tar)).unwrap();
        assert_eq!(got.contents["personal/s.md"], b"abcdef");
        assert!(got.contents.contains_key("personal/t.md"));
        // Not a number: 0. Negative: the archive ends there. Over 4300 digits: 0.
        let tar = [
            manifest(),
            pax(b'x', &[(b"size", b"x")]),
            member(b"personal/s.md", b"abc"),
        ]
        .concat();
        assert_eq!(load_blob(&gz(&tar)).unwrap().contents["personal/s.md"], b"");
        let tar = [
            manifest(),
            pax(b'x', &[(b"size", b"-1")]),
            member(b"personal/s.md", b"abc"),
        ]
        .concat();
        assert_eq!(load(&tar), names(&["manifest.json"]));
        let digits = vec![b'1'; 4301];
        let tar = [
            manifest(),
            pax(b'x', &[(b"size", &digits)]),
            member(b"personal/s.md", b"abc"),
        ]
        .concat();
        assert_eq!(load_blob(&gz(&tar)).unwrap().contents["personal/s.md"], b"");
        let tar = [
            manifest(),
            pax(b'x', &[(b"size", &[b'9'; 40])]),
            member(b"personal/s.md", b"abc"),
        ]
        .concat();
        assert_eq!(load(&tar), rejected("the notes add up to more than 50 MiB"));
        // Unicode digits count, as in `int()`.
        let tar = [
            manifest(),
            pax(b'x', &[(b"size", "\u{663}".as_bytes())]),
            member(b"personal/s.md", b"abc"),
        ]
        .concat();
        assert_eq!(
            load_blob(&gz(&tar)).unwrap().contents["personal/s.md"],
            b"abc"
        );
        // A NUL or a name that is not UTF-8 from PAX is still checked.
        let tar = [
            manifest(),
            pax(b'x', &[(b"path", b"personal/p\0.md")]),
            member(b"x", b"pp"),
        ]
        .concat();
        assert_eq!(
            load(&tar),
            rejected(r"unsafe member name: 'personal/p\x00.md'")
        );
        let tar = [
            manifest(),
            pax(
                b'x',
                &[(b"hdrcharset", b"BINARY"), (b"path", b"personal/\xe9.md")],
            ),
            member(b"x", b"pp"),
        ]
        .concat();
        assert_eq!(
            load(&tar),
            rejected(r"member outside the memory locations: 'personal/\udce9.md'")
        );
    }

    #[test]
    fn pax_record_framing_is_checked_as_in_3_12_16() {
        let raw = |body: &[u8]| entry(hdr(b"PaxHeader/x", body.len() as u64, b'x'), body);
        let ok = names(&["manifest.json", "personal/good.md"]);
        assert_eq!(
            load(
                &[
                    manifest(),
                    raw(b"5 x=\n"),
                    member(b"personal/good.md", b"g")
                ]
                .concat()
            ),
            ok
        );
        // A NUL ends the records: the path still applies.
        assert_eq!(
            load(
                &[
                    manifest(),
                    raw(b"13 path=a.md\n\0junk"),
                    member(b"personal/good.md", b"g")
                ]
                .concat()
            ),
            rejected("member outside the memory locations: 'a.md'")
        );
        for body in [
            &b"4 x=\n"[..],
            b"99 x=\n",
            b"6 x=y\r",
            b"6 =yy\n",
            b"6 xyyy\n",
            b"123456789012345678901 x=\n",
            b"junk",
        ] {
            // Invalid after the first member: the archive ends quietly there.
            assert_eq!(
                load(&[good(), raw(body), member(b"personal/b.md", b"b")].concat()),
                ok,
                "{body:?}"
            );
            // As the first header: an error.
            assert_eq!(
                load(&[raw(body), good()].concat()),
                rejected("not a readable memory archive: invalid header"),
                "{body:?}"
            );
        }
    }

    #[test]
    fn pax_global_headers_apply_to_every_later_member() {
        let tar = [
            manifest(),
            pax(b'g', &[(b"path", b"personal/g.md")]),
            member(b"x", b"g"),
            member(b"y", b"h"),
        ]
        .concat();
        assert_eq!(load(&tar), rejected("duplicate member: 'personal/g.md'"));
        // A local path wins over the global one.
        let tar = [
            manifest(),
            pax(b'g', &[(b"path", b"personal/g.md")]),
            member(b"x", b"g"),
            pax(b'x', &[(b"path", b"personal/i.md")]),
            member(b"y", b"h"),
        ]
        .concat();
        assert_eq!(
            load(&tar),
            names(&["manifest.json", "personal/g.md", "personal/i.md"])
        );
        // A global size changes the size but not where the next header is.
        let tar = [
            manifest(),
            pax(b'g', &[(b"size", b"2")]),
            member(b"personal/a.md", b"abcd"),
        ]
        .concat();
        assert_eq!(
            load_blob(&gz(&tar)).unwrap().contents["personal/a.md"],
            b"ab"
        );
    }

    #[test]
    fn gnu_sparse_members() {
        // PAX 0.1: the map is data runs; holes are zeros.
        let tar = [
            manifest(),
            pax(
                b'x',
                &[
                    (b"GNU.sparse.map", b"0,2,4,1"),
                    (b"GNU.sparse.realsize", b"6"),
                ],
            ),
            member(b"personal/sp.md", b"abc"),
        ]
        .concat();
        assert_eq!(
            load_blob(&gz(&tar)).unwrap().contents["personal/sp.md"],
            b"ab\0\0c\0"
        );
        let tar = [
            manifest(),
            pax(b'x', &[(b"GNU.sparse.map", b"0,2,x")]),
            member(b"personal/sp.md", b"abc"),
        ]
        .concat();
        assert_eq!(
            load(&tar),
            rejected("not a readable memory archive: invalid literal for int() with base 10: 'x'")
        );
        // PAX 0.0: a bad number is an invalid header, which ends the archive.
        let tar = [
            manifest(),
            pax(
                b'x',
                &[(b"GNU.sparse.size", b"5"), (b"GNU.sparse.offset", b"z")],
            ),
            member(b"personal/sp.md", b"abc"),
        ]
        .concat();
        assert_eq!(load(&tar), names(&["manifest.json"]));
        // Old GNU `S`: parsed, then refused.
        let tar = [manifest(), entry(hdr(b"personal/sp.md", 3, b'S'), b"abc")].concat();
        assert_eq!(
            load(&tar),
            rejected("member 'personal/sp.md' is not a plain file or folder")
        );
        let ext = with(hdr(b"personal/sp.md", 3, b'S'), 482, b"\x01");
        assert_eq!(
            load(&[manifest(), ext].concat()),
            rejected("not a readable memory archive: index out of range")
        );
        // A map of more than MAX_SPARSE pairs, or a number outside i64, is refused.
        let map = vec!["0,0"; MAX_SPARSE + 1].join(",");
        let tar = [
            manifest(),
            pax(b'x', &[(b"GNU.sparse.map", map.as_bytes())]),
            member(b"personal/sp.md", b"abc"),
        ]
        .concat();
        assert_eq!(
            load(&tar),
            rejected("not a readable memory archive: too many sparse entries")
        );
        let tar = [
            manifest(),
            pax(b'x', &[(b"GNU.sparse.map", b"0,99999999999999999999")]),
            member(b"personal/sp.md", b"abc"),
        ]
        .concat();
        assert_eq!(
            load(&tar),
            rejected("not a readable memory archive: sparse value out of range")
        );
    }

    // --- limits -----------------------------------------------------------------------------

    #[test]
    fn an_extended_header_bomb_is_refused_by_the_stream_cap() {
        let mut tar = hdr(b"PaxHeader/x", 80 << 20, b'x');
        tar.resize(512 + (80 << 20), 0);
        assert_eq!(load(&tar), Err(CAP.to_string()));
        let mut tar = [manifest(), hdr(b"././@LongLink", 80 << 20, b'L')].concat();
        tar.resize(1536 + (80 << 20), 0);
        assert_eq!(load(&tar), Err(CAP.to_string()));
    }

    #[test]
    fn member_count_limit() {
        let mut tar = manifest();
        for i in 0..MAX_MEMBERS {
            tar.extend(member(format!("personal/n{i:05}.md").as_bytes(), b"x"));
        }
        assert_eq!(
            load(&tar),
            Err(format!("the archive has more than {MAX_MEMBERS} members"))
        );
    }

    #[test]
    fn names_too_long_for_the_tar_filter_are_refused() {
        let long = |n: usize| [b"personal/".to_vec(), vec![b'a'; n], b".md".to_vec()].concat();
        let ok = long(PATH_MAX - FILTER_DEST_LEN - 13);
        let tar = [manifest(), pax(b'x', &[(b"path", &ok)]), member(b"x", b"n")].concat();
        assert_eq!(load(&tar).unwrap().len(), 2);
        let bad = long(PATH_MAX - FILTER_DEST_LEN - 12);
        let tar = [
            manifest(),
            pax(b'x', &[(b"path", &bad)]),
            member(b"x", b"n"),
        ]
        .concat();
        let err = load(&tar).unwrap_err();
        assert!(err.starts_with("member 'personal/aaa"), "{err}");
        assert!(
            err.contains(".md' rejected by the tar filter: [Errno "),
            "{err}"
        );
    }

    // --- contents and the manifest ----------------------------------------------------------

    #[test]
    fn contents_must_be_utf8_and_the_manifest_format_1() {
        let tar = [good(), member(b"personal/bin.md", b"\xff\xfe")].concat();
        assert_eq!(
            load(&tar),
            rejected("member 'personal/bin.md' is not valid UTF-8")
        );
        let tar = [good(), member(b"personal/sur.md", b"\xed\xa0\x80")].concat();
        assert_eq!(
            load(&tar),
            rejected("member 'personal/sur.md' is not valid UTF-8")
        );
        let only = |m: &[u8]| load(&member(b"manifest.json", m));
        for m in [
            &br#"{"format": 1}"#[..],
            br#"{"format": 1.0}"#,
            br#"{"format": true}"#,
            br#"{"format": 2, "format": 1}"#,
        ] {
            assert_eq!(only(m), names(&["manifest.json"]), "{m:?}");
        }
        for m in [
            &br#"{"format": "1"}"#[..],
            br#"{"format": 99}"#,
            b"[1]",
            br#"{"format": NaN}"#,
            b"{}",
        ] {
            assert_eq!(only(m), rejected("unsupported archive format"), "{m:?}");
        }
        assert_eq!(
            only(b"not json"),
            rejected("manifest.json is not valid JSON")
        );
        assert_eq!(
            load(&member(b"personal/good.md", b"g")),
            rejected("manifest.json is missing")
        );
    }

    #[test]
    fn the_archive_file_is_checked_before_it_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.tar.gz");
        assert_eq!(
            load_archive(&missing).unwrap_err().0,
            "cannot read the archive: No such file or directory"
        );
        assert_eq!(
            load_archive(dir.path()).unwrap_err().0,
            "cannot read the archive: Is a directory"
        );
        #[cfg(unix)]
        {
            let fifo = dir.path().join("fifo");
            let made = std::process::Command::new("mkfifo").arg(&fifo).status();
            if made.is_ok_and(|s| s.success()) {
                assert_eq!(
                    load_archive(&fifo).unwrap_err().0,
                    "the archive is not a regular file"
                );
            }
        }
        let huge = dir.path().join("huge.tar.gz");
        std::fs::File::create(&huge)
            .unwrap()
            .set_len(MAX_ARCHIVE + 1)
            .unwrap();
        assert_eq!(
            load_archive(&huge).unwrap_err().0,
            "the archive is larger than 50 MiB"
        );
        let ok = dir.path().join("ok.tar.gz");
        let blob = gz(&[good(), END.to_vec()].concat());
        std::fs::write(&ok, &blob).unwrap();
        let loaded = load_archive(&ok).unwrap();
        let want: String = Sha256::digest(&blob)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(loaded.sha256, want);
        assert_eq!(loaded.contents["personal/good.md"], b"good\n");
        assert_eq!(
            loaded.manifest.get("date"),
            Some(&Value::Str("2026-01-01".to_string()))
        );
    }
}
