//! rtk install and reconcile (RT2): `plugins/anthology/scripts/rtk_install.py`, GC-9.
//!
//! `rtk.lock` pins a version, the release tag, the GitHub repo and the sha256 of each release
//! tarball (`rtk-<target>.tar.gz`). [`install`] downloads the tarball for this machine into a
//! fresh `<data>/rtk/.tmp-XXXXXXXX/` folder, checks its sha256 against the lock **before the
//! tar is opened**, extracts only the one regular member named exactly `rtk` (`O_EXCL`, 0700,
//! then 0755), and places it at `<data>/rtk/<version>/rtk`: unlink `.sha256`, rename the
//! binary in, write `.sha256` (`<sha>\n`) last. The staging folder is removed on every path,
//! so no partial file is ever installed. [`reconcile`] is what session start and setup call:
//! it installs when the binary is missing or does not match the lock, records the path in
//! state.json (which `bin/rtk` and `bin/rtk-hook` read), and turns any failure into the GC-9
//! warning. It never panics outward.
//!
//! Seams (GC-19): [`RtkDeps`] carries the fetch (`default_fetch`, `build_opener`), `uname`
//! (`platform.system`/`machine`), `now` (`time.time` in prune) and the `tar_open` fault;
//! [`RtkLimits`] carries `MAX_DOWNLOAD_BYTES` (used by the production fetch),
//! `MAX_BINARY_BYTES` and `MAX_TAR_MEMBERS`.
//!
//! "Executable" is "a regular file with an execute bit": `os.access(X_OK)` needs the effective
//! uid, which safe std cannot read; for a binary this process installed the two agree.
#![forbid(unsafe_code)]

use std::fs::{self, File, OpenOptions};
use std::hash::{BuildHasher, Hasher};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anthology_core::lock::{self, Lock, RTK_TAG_PREFIX};
use anthology_core::platform::{self, Target};
use anthology_core::{Env, state};
use anthology_pycompat::json::{self, Map, Value};
use anthology_pycompat::pyerr::PyExc;
use sha2::{Digest, Sha256};

use crate::fetch::{self, Fetch, install_error};

/// `MAX_BINARY_BYTES`: the extracted rtk.
pub const MAX_BINARY_BYTES: u64 = 128 * 1024 * 1024;
/// `MAX_TAR_MEMBERS`.
pub const MAX_TAR_MEMBERS: usize = 64;
/// `PRUNE_AFTER_DAYS`.
pub const PRUNE_AFTER_DAYS: u64 = 30;
/// `STALE_TEMP_SECONDS`.
pub const STALE_TEMP_SECONDS: u64 = 24 * 3600;
/// `DEFAULT_TIMEOUT` (the CLI's; session start passes 8 and setup 60, GC-9).
pub const DEFAULT_TIMEOUT: f64 = 30.0;

/// The constants the Python tests patch (GC-19).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RtkLimits {
    pub max_download_bytes: u64,
    pub max_binary_bytes: u64,
    pub max_tar_members: usize,
}

impl Default for RtkLimits {
    fn default() -> RtkLimits {
        RtkLimits {
            max_download_bytes: fetch::MAX_DOWNLOAD_BYTES,
            max_binary_bytes: MAX_BINARY_BYTES,
            max_tar_members: MAX_TAR_MEMBERS,
        }
    }
}

/// The seams of this module (GC-19, module docs).
#[derive(Clone, Copy)]
pub struct RtkDeps<'a> {
    pub fetch: &'a dyn Fetch,
    /// `(platform.system(), platform.machine())`.
    pub uname: fn() -> (String, String),
    /// `time.time()`, for prune.
    pub now: fn() -> f64,
    /// Runs where `tarfile.open` is called; an error stands for whatever it raised.
    pub tar_open: Option<fn() -> Result<(), PyExc>>,
    pub limits: RtkLimits,
}

impl<'a> RtkDeps<'a> {
    /// The production seams around `fetch`.
    pub fn new(fetch: &'a dyn Fetch) -> RtkDeps<'a> {
        RtkDeps {
            fetch,
            uname: platform::uname,
            now: now_epoch,
            tar_open: None,
            limits: RtkLimits::default(),
        }
    }
}

/// The production fetch for `env` (`default_fetch`), with `limits`' download cap.
pub fn default_fetch(env: &Env, limits: &RtkLimits) -> fetch::UreqFetch {
    fetch::UreqFetch::new(env).with_max_bytes(limits.max_download_bytes)
}

fn now_epoch() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

// --- platform and lock ------------------------------------------------------------------

/// `target(system, machine)` (GC-9 messages).
pub fn target(system: &str, machine: &str) -> Result<Target, PyExc> {
    platform::detect_with((system, machine)).map_err(install_error)
}

/// `validate_lock(value)`.
pub fn validate_lock(value: &Value) -> Result<Lock, PyExc> {
    lock::validate(value, RTK_TAG_PREFIX).map_err(install_error)
}

/// `load_lock(path)`: `cannot read <path>: <error>`, `<path> is not valid JSON`, or a
/// `validate_lock` message.
pub fn load_lock(path: &Path) -> Result<Lock, PyExc> {
    let shown = path.to_string_lossy();
    let bytes = fs::read(path).map_err(|e| {
        let mut exc = PyExc::from(e);
        if let PyExc::OSError { filename, .. } = &mut exc {
            *filename = Some(shown.to_string());
        }
        install_error(format!("cannot read {shown}: {}", exc.py_str()))
    })?;
    // `read_text` raises UnicodeDecodeError (not OSError, so not "cannot read") on bytes that
    // are not UTF-8.
    let text = decode_utf8(bytes)?;
    let value =
        json::loads(&text).map_err(|_| install_error(format!("{shown} is not valid JSON")))?;
    validate_lock(&value)
}

/// `release_url(lock, target)`.
pub fn release_url(lock: &Lock, tgt: Target) -> String {
    format!(
        "https://github.com/{}/releases/download/{}/rtk-{}.tar.gz",
        lock.repo,
        lock.tag,
        tgt.triple()
    )
}

/// The lock's hash for `tgt`.
fn expected_hash(lock: &Lock, tgt: Target) -> Result<&str, PyExc> {
    lock.sha256
        .get(&tgt)
        .map(String::as_str)
        .ok_or_else(|| install_error(format!("no rtk build for {}", tgt.triple())))
}

// --- verify -------------------------------------------------------------------------------

/// `hmac.compare_digest(a, b)` on bytes: constant time for equal lengths.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The lowercase hex sha256 of the file at `path`, streamed.
pub fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        digest.update(&buf[..n]);
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// `verify(path, sha256)`: the file's sha256 equals `sha256` (lower-cased), compared in
/// constant time; false when the file cannot be read.
pub fn verify(path: &Path, sha256: &str) -> bool {
    match sha256_file(path) {
        Ok(hex) => ct_eq(hex.as_bytes(), sha256.to_lowercase().as_bytes()),
        Err(_) => false,
    }
}

// --- install ------------------------------------------------------------------------------

/// A regular file (not a symlink) with an execute bit (module docs).
pub fn is_executable_file(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|m| m.file_type().is_file() && m.permissions().mode() & 0o111 != 0)
}

/// `_is_current(vdir, expected)`: `vdir` is not a symlink and holds an executable regular-file
/// `rtk` whose recorded tarball hash (`.sha256`, stripped) is `expected`. A marker that is not
/// UTF-8 raises, as `read_text` does there (only `OSError` reads as "not current").
fn is_current(vdir: &Path, expected: &str) -> Result<bool, PyExc> {
    match fs::symlink_metadata(vdir) {
        Ok(m) if !m.file_type().is_symlink() => {}
        _ => return Ok(false),
    }
    if !is_executable_file(&vdir.join("rtk")) {
        return Ok(false);
    }
    let Ok(bytes) = fs::read(vdir.join(".sha256")) else {
        return Ok(false);
    };
    let text = decode_utf8(bytes)?;
    Ok(ct_eq(py_strip(&text).as_bytes(), expected.as_bytes()))
}

/// `bytes.decode("utf-8")`, with CPython's `UnicodeDecodeError` text.
pub fn decode_utf8(bytes: Vec<u8>) -> Result<String, PyExc> {
    String::from_utf8(bytes).map_err(|e| {
        let at = e.utf8_error().valid_up_to();
        let b = e.as_bytes();
        let msg = match e.utf8_error().error_len() {
            None if b.len() - at > 1 => format!(
                "'utf-8' codec can't decode bytes in position {at}-{}: unexpected end of data",
                b.len() - 1
            ),
            None => format!(
                "'utf-8' codec can't decode byte {:#04x} in position {at}: unexpected end of data",
                b[at]
            ),
            Some(_) => {
                let start = matches!(b[at], 0x80..=0xc1 | 0xf5..=0xff);
                format!(
                    "'utf-8' codec can't decode byte {:#04x} in position {at}: invalid {} byte",
                    b[at],
                    if start { "start" } else { "continuation" }
                )
            }
        };
        PyExc::UnicodeDecodeError(msg)
    })
}

/// `str.strip()`: Unicode whitespace and the separators U+001C..U+001F.
fn py_strip(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
}

/// A tar member as Python's `tarfile` shows it.
struct Member {
    name: Vec<u8>,
    regular: bool,
    size: u64,
}

fn member_of<R: Read>(entry: &tar::Entry<'_, R>) -> Member {
    // The raw typeflag: the crate maps `\0` and `0` to one `Regular`, tarfile does not.
    let flag = entry.header().as_bytes()[156];
    let mut name = entry.path_bytes().into_owned();
    // tarfile: an old-style (`\0`) regular file whose name ends in "/" is a folder, and a
    // folder's name loses its trailing slashes.
    let dir = flag == b'5' || (flag == b'\0' && name.last() == Some(&b'/'));
    if dir {
        while name.last() == Some(&b'/') {
            name.pop();
        }
    }
    Member {
        name,
        // tarfile's REGULAR_TYPES: REGTYPE, AREGTYPE, CONTTYPE, GNUTYPE_SPARSE.
        regular: !dir && matches!(flag, b'0' | b'\0' | b'7' | b'S'),
        size: entry.header().entry_size().unwrap_or(u64::MAX),
    }
}

/// `cannot read the rtk tarball: <library text>`. The `tar` crate may quote raw header bytes,
/// so control characters (a newline included) become `?`: the warning stays one line.
fn tar_error(e: impl std::fmt::Display) -> PyExc {
    let text: String = e
        .to_string()
        .chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect();
    install_error(format!("cannot read the rtk tarball: {text}"))
}

/// `_extract_rtk(archive, dest)`: copies the one regular-file member named exactly `rtk` to
/// `dest` (created `O_EXCL`, 0700); never extracts anything else.
fn extract_rtk(archive: &Path, dest: &Path, deps: &RtkDeps<'_>) -> Result<(), PyExc> {
    if let Some(hook) = deps.tar_open {
        hook()?;
    }
    let limits = &deps.limits;
    let file = File::open(archive).map_err(|e| os_error(e, archive))?;
    let gz = Watched::new(flate2::read::MultiGzDecoder::new(io::BufReader::new(file)));
    let (gz_failed, gz_bytes) = (gz.failed.clone(), gz.bytes.clone());
    let mut tf = tar::Archive::new(gz);
    let entries = tf.entries().map_err(tar_error)?;
    let mut found = false;
    let mut count = 0usize;
    for entry in entries {
        let mut entry = match entry {
            Ok(e) => e,
            // tarfile: a header it cannot read ends the archive, except at the very start;
            // a decompression error is always an error.
            Err(_) if count > 0 && !gz_failed.get() => break,
            Err(e) => return Err(tar_error(e)),
        };
        // A pax global header describes the archive; tarfile does not yield it as a member.
        if entry.header().entry_type() == tar::EntryType::XGlobalHeader {
            continue;
        }
        count += 1;
        if count > limits.max_tar_members {
            return Err(install_error("tarball has too many members"));
        }
        let member = member_of(&entry);
        if member.name != b"rtk" {
            continue;
        }
        if found {
            return Err(install_error("tarball has more than one rtk member"));
        }
        if !member.regular {
            return Err(install_error("the rtk member is not a regular file"));
        }
        if member.size > limits.max_binary_bytes {
            return Err(install_error("the rtk member is too large"));
        }
        found = true;
        let mut out = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o700)
            .open(dest)
            .map_err(|e| os_error(e, dest))?;
        let mut written: u64 = 0;
        let mut buf = vec![0u8; 1 << 16];
        loop {
            let n = entry.read(&mut buf).map_err(tar_error)?;
            if n == 0 {
                break;
            }
            written += n as u64;
            if written > limits.max_binary_bytes {
                return Err(install_error("the rtk member is too large"));
            }
            out.write_all(&buf[..n])?;
        }
        out.flush()?;
    }
    if count == 0 && gz_bytes.get() == 0 {
        // tarfile.open: a stream with no header block at all.
        return Err(tar_error("empty file"));
    }
    if !found {
        return Err(install_error("tarball has no rtk member"));
    }
    Ok(())
}

/// A reader that remembers whether it failed and how many bytes it gave.
struct Watched<R> {
    inner: R,
    failed: std::rc::Rc<std::cell::Cell<bool>>,
    bytes: std::rc::Rc<std::cell::Cell<u64>>,
}

impl<R> Watched<R> {
    fn new(inner: R) -> Watched<R> {
        Watched {
            inner,
            failed: Default::default(),
            bytes: Default::default(),
        }
    }
}

impl<R: Read> Read for Watched<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.inner.read(buf) {
            Ok(n) => {
                self.bytes.set(self.bytes.get() + n as u64);
                Ok(n)
            }
            Err(e) => {
                self.failed.set(true);
                Err(e)
            }
        }
    }
}

/// `_touch(path)`: create if missing, then set both times to now.
fn touch(path: &Path) -> Result<(), PyExc> {
    let file = OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
        .map_err(|e| os_error(e, path))?;
    let now = SystemTime::now();
    file.set_times(fs::FileTimes::new().set_accessed(now).set_modified(now))
        .map_err(|e| os_error(e, path))?;
    Ok(())
}

/// `tempfile.mkdtemp(prefix=".tmp-", dir=root)`: a new 0700 folder `.tmp-XXXXXXXX`.
fn mkdtemp(root: &Path) -> Result<PathBuf, PyExc> {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789_";
    let state = std::collections::hash_map::RandomState::new();
    for attempt in 0u64..100 {
        let mut h = state.build_hasher();
        h.write_u64(attempt);
        h.write_u128(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
        );
        h.write_u32(std::process::id());
        let mut bits = h.finish();
        let name: String = (0..8)
            .map(|_| {
                let c = ALPHABET[(bits % ALPHABET.len() as u64) as usize];
                bits /= ALPHABET.len() as u64;
                char::from(c)
            })
            .collect();
        let path = root.join(format!(".tmp-{name}"));
        match fs::DirBuilder::new().mode(0o700).create(&path) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(os_error(e, &path)),
        }
    }
    Err(PyExc::OSError {
        errno: 17,
        filename: Some(root.to_string_lossy().into_owned()),
    })
}

/// An `OSError` naming `path`, as Python's file functions raise it.
fn os_error(e: io::Error, path: &Path) -> PyExc {
    let mut exc = PyExc::from(e);
    if let PyExc::OSError { filename, .. } = &mut exc {
        *filename = Some(path.to_string_lossy().into_owned());
    }
    exc
}

/// `os.replace(src, dst)`'s `OSError`: `[Errno N] <text>: '<src>' -> '<dst>'` (`filename2`).
fn os_error2(e: io::Error, src: &Path, dst: &Path) -> PyExc {
    let PyExc::OSError { errno, .. } = PyExc::from(e) else {
        unreachable!("io::Error always maps to OSError")
    };
    let named = |p: &Path| {
        PyExc::OSError {
            errno,
            filename: Some(p.to_string_lossy().into_owned()),
        }
        .py_str()
    };
    let bare = PyExc::OSError {
        errno,
        filename: None,
    };
    // `named(p)` is `<bare>: <repr(p)>`; the second repr follows ` -> `.
    let tail = named(dst)[bare.py_str().len() + 2..].to_string();
    PyExc::Other(bare.type_name(), format!("{} -> {tail}", named(src)))
}

/// `os.replace(src, dst)`.
fn replace(src: &Path, dst: &Path) -> Result<(), PyExc> {
    fs::rename(src, dst).map_err(|e| os_error2(e, src, dst))
}

/// Removes the staging folder however the install ends (`finally: shutil.rmtree(tmp)`).
struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// `root.mkdir(parents=True, exist_ok=True)`.
fn mkdir_p(dir: &Path) -> Result<(), PyExc> {
    match fs::create_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(e) => {
            if fs::metadata(dir).is_ok_and(|m| m.is_dir()) {
                Ok(())
            } else {
                Err(os_error(e, dir))
            }
        }
    }
}

/// `_download_and_place(root, vdir, url, expected, fetch, timeout)` (module docs).
fn download_and_place(
    root: &Path,
    vdir: &Path,
    url: &str,
    expected: &str,
    timeout: f64,
    deps: &RtkDeps<'_>,
) -> Result<(), PyExc> {
    mkdir_p(root)?;
    let tmp = TempDir(mkdtemp(root)?);
    let archive = tmp.0.join("archive.tar.gz");
    fetch::fetch_to_path(deps.fetch, url, &archive, timeout)?;
    if !verify(&archive, expected) {
        return Err(install_error(format!(
            "sha256 of {url} does not match rtk.lock"
        )));
    }
    let staged = tmp.0.join("rtk");
    extract_rtk(&archive, &staged, deps)?;
    fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))
        .map_err(|e| os_error(e, &staged))?;
    if fs::symlink_metadata(vdir).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(install_error(format!(
            "{} is a symlink",
            vdir.to_string_lossy()
        )));
    }
    let created = fs::metadata(vdir).is_err();
    if let Err(e) = fs::create_dir(vdir) {
        let exists_as_dir = e.kind() == io::ErrorKind::AlreadyExists
            && fs::metadata(vdir).is_ok_and(|m| m.is_dir());
        if !exists_as_dir {
            return Err(os_error(e, vdir));
        }
    }
    let placed = (|| -> Result<(), PyExc> {
        // The marker is written last, so it is removed first.
        match fs::remove_file(vdir.join(".sha256")) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(os_error(e, &vdir.join(".sha256"))),
        }
        replace(&staged, &vdir.join("rtk"))?;
        let marker = tmp.0.join("marker");
        fs::write(&marker, format!("{expected}\n")).map_err(|e| os_error(e, &marker))?;
        replace(&marker, &vdir.join(".sha256"))?;
        Ok(())
    })();
    if placed.is_err() && created {
        let _ = fs::remove_dir_all(vdir);
    }
    placed
}

/// `VERSION_RE.fullmatch(name)`, through the lock validator's own version rule.
fn is_version_name(name: &str) -> bool {
    let mut probe = Map::new();
    probe.insert("version".into(), Value::Str(name.into()));
    probe.insert("tag".into(), Value::Null);
    probe.insert("repo".into(), Value::Null);
    probe.insert("sha256".into(), Value::Null);
    match lock::validate(&Value::Object(probe), RTK_TAG_PREFIX) {
        Ok(_) => true,
        Err(e) => !e.starts_with("rtk.lock: invalid version"),
    }
}

fn mtime(m: &fs::Metadata) -> f64 {
    m.mtime() as f64 + f64::from(m.mtime_nsec() as u32) / 1e9
}

/// `prune(root, current, now)`: deletes the version folders directly under `root` unused for
/// 30 days (by `.last-used`, else the folder's mtime) and `.tmp-*` folders older than 24 h;
/// never `current`, never a symlink or anything not a folder. Returns the names removed.
pub fn prune(root: &Path, current: &str, now: f64) -> Vec<String> {
    let mut removed = Vec::new();
    let Ok(entries) = fs::read_dir(root) else {
        return removed;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name == current {
            continue;
        }
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !meta.file_type().is_dir() {
            continue;
        }
        let (stamp, limit) = if is_version_name(name) {
            let stamp = fs::symlink_metadata(path.join(".last-used"))
                .map_or_else(|_| mtime(&meta), |m| mtime(&m));
            (stamp, (PRUNE_AFTER_DAYS * 86400) as f64)
        } else if name.starts_with(".tmp-") {
            (mtime(&meta), STALE_TEMP_SECONDS as f64)
        } else {
            continue;
        };
        if now - stamp > limit && fs::remove_dir_all(&path).is_ok() {
            removed.push(name.to_string());
        }
    }
    removed
}

/// `install(data_dir, lock, fetch, timeout, tgt)`: makes `<data>/rtk/<version>/rtk` exist and
/// match `lock`, and returns its path. Fetches only when the binary is missing or its recorded
/// tarball hash differs from the lock; on failure nothing is left behind.
pub fn install(
    data_dir: &Path,
    lock: &Lock,
    timeout: f64,
    tgt: Target,
    deps: &RtkDeps<'_>,
) -> Result<PathBuf, PyExc> {
    let expected = expected_hash(lock, tgt)?;
    let root = data_dir.join("rtk");
    let vdir = root.join(&lock.version);
    if !is_current(&vdir, expected)? {
        download_and_place(
            &root,
            &vdir,
            &release_url(lock, tgt),
            expected,
            timeout,
            deps,
        )?;
    }
    touch(&vdir.join(".last-used"))?;
    prune(&root, &lock.version, (deps.now)());
    Ok(vdir.join("rtk"))
}

/// `check_installed(data_dir, lock)` for this machine: the binary's path when
/// `<data>/rtk/<version>/rtk` is an executable regular file whose recorded tarball hash equals
/// the lock's; otherwise why not. Read-only, never downloads (RT1b).
pub fn check_installed(data_dir: &Path, lock: &Lock) -> Result<PathBuf, String> {
    let (system, machine) = platform::uname();
    check_installed_with(data_dir, lock, (&system, &machine))
}

/// [`check_installed`] for the platform `uname` (the `platform.system`/`machine` seam).
pub fn check_installed_with(
    data_dir: &Path,
    lock: &Lock,
    uname: (&str, &str),
) -> Result<PathBuf, String> {
    let tgt = target(uname.0, uname.1).map_err(|e| e.py_str())?;
    check_installed_for(data_dir, lock, tgt)
}

/// [`check_installed`] for `tgt`.
pub fn check_installed_for(data_dir: &Path, lock: &Lock, tgt: Target) -> Result<PathBuf, String> {
    let expected = expected_hash(lock, tgt).map_err(|e| e.py_str())?;
    let vdir = data_dir.join("rtk").join(&lock.version);
    if is_current(&vdir, expected).unwrap_or(false) {
        Ok(vdir.join("rtk"))
    } else {
        Err(format!(
            "rtk {} is not installed at {}",
            lock.version,
            vdir.join("rtk").to_string_lossy()
        ))
    }
}

/// What [`reconcile`] did (plan §3.6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reconciled {
    pub rtk_path: Option<PathBuf>,
    pub warning: Option<String>,
    pub installed: bool,
}

/// The GC-9 warning for a failure `exc`.
pub fn warning_text(exc: &PyExc) -> String {
    format!(
        "rtk is not installed or up to date ({}); commands run without it. It is retried at \
         the next session start.",
        exc.py_str()
    )
}

/// `reconcile(data_dir, lock, fetch, timeout)`: the session-start entry point (module docs).
/// On success `rtk_path` is the binary and state.json's `rtk_path` is updated when it differs
/// (`installed` is true only when something was downloaded). On any failure `rtk_path` is
/// `None`, `warning` explains it, and state.json and older version folders are left as they
/// were.
pub fn reconcile(
    env: &Env,
    data_dir: &Path,
    lock: &Lock,
    timeout: f64,
    deps: &RtkDeps<'_>,
) -> Reconciled {
    let attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        reconcile_inner(env, data_dir, lock, timeout, deps)
    }));
    let result = attempt.unwrap_or_else(|_| {
        Err(PyExc::Other(
            "RuntimeError",
            "internal error in the rtk installer".into(),
        ))
    });
    match result {
        Ok((path, installed)) => Reconciled {
            rtk_path: Some(path),
            warning: None,
            installed,
        },
        Err(exc) => Reconciled {
            rtk_path: None,
            warning: Some(warning_text(&exc)),
            installed: false,
        },
    }
}

fn reconcile_inner(
    env: &Env,
    data_dir: &Path,
    lock: &Lock,
    timeout: f64,
    deps: &RtkDeps<'_>,
) -> Result<(PathBuf, bool), PyExc> {
    let (system, machine) = (deps.uname)();
    let tgt = target(&system, &machine)?;
    let vdir = data_dir.join("rtk").join(&lock.version);
    let was_current = is_current(&vdir, expected_hash(lock, tgt)?)?;
    let path = install(data_dir, lock, timeout, tgt, deps)?;
    let shown = path.to_string_lossy().into_owned();
    if state::read(env).get("rtk_path").and_then(Value::as_str) != Some(shown.as_str()) {
        state::write(env, vec![("rtk_path".into(), Value::Str(shown))])?;
    }
    Ok((path, !was_current))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ct_eq_compares_whole_values() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"abcd"));
        assert!(ct_eq(b"", b""));
    }

    #[test]
    fn version_names_follow_version_re() {
        for ok in ["1.2.3", "1.2.1-anthology.1", "0.0.0-a-b.c", "١.2.3"] {
            assert!(is_version_name(ok), "{ok}");
        }
        for bad in [
            "notes",
            "1.2",
            "1.2.3-",
            "1.2.3-a..b",
            ".tmp-x",
            "1.2.3\n",
            "v1.2.3",
        ] {
            assert!(!is_version_name(bad), "{bad}");
        }
    }

    #[test]
    fn py_strip_matches_str_strip() {
        assert_eq!(py_strip("  abc\n"), "abc");
        assert_eq!(py_strip("\u{1c}abc\u{85}"), "abc");
    }
}
