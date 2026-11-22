//! The export archive's bytes (MX1, m-20): what `memory_portable.export` writes with
//! `tarfile.open(fileobj=gz, mode="w", format=tarfile.PAX_FORMAT)` inside
//! `gzip.GzipFile(filename="", mode="wb", fileobj=buf, mtime=0)`. The `tar` crate only reads
//! archives here; this writer is custom so the tar stream is byte-identical to CPython 3.12's.
//!
//! What CPython 3.12 writes for `_tarinfo(name, size)` (`TarInfo.create_pax_header`,
//! `_create_header`, `_create_pax_generic_header`):
//! - a `././@PaxHeader` (type `x`) header only when the name is over 100 characters or not
//!   ASCII (uid, gid, size and mtime always fit their octal fields here: a size is at most
//!   50 MiB, far below `8**11`); its one record is `path=<name>`, its own fields are mode,
//!   uid, gid and mtime 0, empty uname and gname;
//! - the ustar header's name is then the name's first 100 bytes (no prefix split in PAX
//!   format), and a name of exactly 100 bytes fills the field with no NUL;
//! - number fields are `%0*o` plus a NUL; devmajor and devminor of a regular file are 8 NULs;
//!   the checksum is `%06o` plus a NUL, its eighth byte left a space;
//! - `close` appends two zero blocks, then zeros up to a multiple of 10240 bytes.

use std::io::Write as _;

const BLOCK: usize = 512;
const RECORD: usize = 10240;

/// `itn(n, digits)` for a value that fits: `digits - 1` octal digits and a NUL.
fn octal(field: &mut [u8], n: u64) {
    let digits = field.len() - 1;
    let text = format!("{n:0digits$o}");
    debug_assert_eq!(text.len(), digits, "number field overflow");
    field[..digits].copy_from_slice(text.as_bytes());
    field[digits] = 0;
}

/// `_create_header` for a ustar header (POSIX magic, mode as given, uid/gid/mtime 0, empty
/// linkname, uname, gname and prefix, devmajor/devminor NUL-filled).
fn ustar_header(name: &[u8], mode: u64, size: u64, typeflag: u8) -> [u8; BLOCK] {
    let mut h = [0u8; BLOCK];
    let n = name.len().min(100);
    h[..n].copy_from_slice(&name[..n]);
    octal(&mut h[100..108], mode);
    octal(&mut h[108..116], 0);
    octal(&mut h[116..124], 0);
    octal(&mut h[124..136], size);
    octal(&mut h[136..148], 0);
    h[148..156].fill(b' ');
    h[156] = typeflag;
    h[257..265].copy_from_slice(b"ustar\x0000");
    let sum: u32 = h.iter().map(|&b| u32::from(b)).sum();
    octal(&mut h[148..155], u64::from(sum));
    h
}

/// `TarInfo._create_payload`: zeros up to the next 512-byte border.
fn pad(out: &mut Vec<u8>) {
    let rem = out.len() % BLOCK;
    if rem > 0 {
        out.resize(out.len() + BLOCK - rem, 0);
    }
}

/// One pax record, `"%d %s=%s\n"`, with the length found as `_create_pax_generic_header` does.
fn pax_record(keyword: &str, value: &str) -> Vec<u8> {
    let l = keyword.len() + value.len() + 3;
    let mut p = 0;
    loop {
        let n = l + p.to_string().len();
        if n == p {
            break;
        }
        p = n;
    }
    format!("{p} {keyword}={value}\n").into_bytes()
}

/// The tar stream CPython 3.12 writes for `members`, in the order given, each added as
/// `memory_portable._tarinfo(name, len(data))` (a regular file, mode 0644, uid and gid 0, empty
/// uname and gname, mtime 0) in PAX format: a `././@PaxHeader` extended header only where
/// tarfile writes one, the data padded to 512 bytes, then the end-of-archive blocks and the
/// padding to a 10240-byte record.
///
/// Names must be ASCII (all export member names are); data sizes must fit the 11-digit octal
/// size field (`< 8 GiB`).
pub fn pax_archive(members: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    for &(name, data) in members {
        debug_assert!(name.is_ascii(), "pax_archive takes ASCII names only");
        if name.len() > 100 {
            let records = pax_record("path", name);
            out.extend_from_slice(&ustar_header(
                b"././@PaxHeader",
                0,
                records.len() as u64,
                b'x',
            ));
            out.extend_from_slice(&records);
            pad(&mut out);
        }
        out.extend_from_slice(&ustar_header(
            name.as_bytes(),
            0o644,
            data.len() as u64,
            b'0',
        ));
        out.extend_from_slice(data);
        pad(&mut out);
    }
    out.resize(out.len() + 2 * BLOCK, 0);
    let rem = out.len() % RECORD;
    if rem > 0 {
        out.resize(out.len() + RECORD - rem, 0);
    }
    out
}

/// `gzip.GzipFile(filename="", mode="wb", mtime=0)` (compresslevel 9) around `data`: the
/// header CPython writes (no file name, mtime 0, XFL 2, OS 255), a deflate stream, CRC32 and
/// ISIZE. The deflate bytes may differ from zlib's (D-5); decompressed they are `data`.
pub fn gzip(data: &[u8]) -> Vec<u8> {
    let header = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 2, 255];
    let mut enc = flate2::write::DeflateEncoder::new(header, flate2::Compression::best());
    enc.write_all(data).expect("writing to a Vec cannot fail");
    let mut out = enc.finish().expect("writing to a Vec cannot fail");
    let mut crc = flate2::Crc::new();
    crc.update(data);
    out.extend_from_slice(&crc.sum().to_le_bytes());
    // ISIZE is the length modulo 2**32, as `GzipFile.close` writes it.
    out.extend_from_slice(&((data.len() as u64 & 0xffff_ffff) as u32).to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read as _;

    fn sha(bytes: &[u8]) -> String {
        use sha2::Digest as _;
        sha2::Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    fn name(n: usize) -> String {
        format!("personal/{}.md", "a".repeat(n - 12))
    }

    /// `(set, decompressed length, sha256)` of the tar stream, from CPython 3.12
    /// (python:3.12-slim) running `tarfile.open(fileobj=buf, mode="w", format=PAX_FORMAT)` and
    /// `tar.addfile(memory_portable._tarinfo(name, len(data)), io.BytesIO(data))` per member.
    const GOLDENS: &[(&str, usize, &str)] = &[
        (
            "empty",
            10240,
            "84ff92691f909a05b224e1c56abb4864f01b4f8e3c854e4bb4c7baf1d3f6d652",
        ),
        (
            "one-note",
            10240,
            "46e1dcf02523011749ee8d39236c1884fe1ec31f7f7aa63e8bde3509eb6f3d39",
        ),
        (
            "manifest-and-notes",
            10240,
            "4ce70fda2af64498fd22f6c4631f75bd1f1c6ea6df485355cb27c3ccd1cc8655",
        ),
        (
            "name-99",
            10240,
            "cf689f51cbd51a55e9939a086ce6dd6c498c33f4a84e786ba6ca079e497aad8e",
        ),
        (
            "name-100",
            10240,
            "77c44d32270e362d773fbf7369947a76bd9ad8c0028bde8e39e88bf9a04f1937",
        ),
        (
            "name-101",
            10240,
            "7a6bf62b93550386f8e5b29d135167736d1d4403b358bc36a84a03c4d628211f",
        ),
        (
            "name-150",
            10240,
            "d16738a9013c7a1bfba7c9ec06f403c2a684f13e85b3dc6a9744e3c62007d50f",
        ),
        (
            "name-1002",
            10240,
            "c8d46fffc0bdebbf66534c3f9a4d93f601b3abf4d8ad75397b6e6ebd141a1976",
        ),
        (
            "name-1003",
            10240,
            "d3d14e608cf85b9c3d9eaee593ee58025e0b8cd86094dcf629d2a2822a441821",
        ),
        (
            "sizes-512-0",
            10240,
            "4fa5e68beaca27aab085b167009e41e44f662d41808275bfab0676adf805bac0",
        ),
        (
            "exact-record",
            10240,
            "ca63490edeb0334c80e47aff1db24cadee79b4364eace6ec84b5b89e8e510ac2",
        ),
        (
            "two-records",
            30720,
            "9086a2abd833e50bf4dd251e3a357f39b12a4545be27a3e9d5e148e2027078f1",
        ),
    ];

    fn set(key: &str) -> Vec<(String, Vec<u8>)> {
        let manifest = "{\n  \"counts\": {\n    \"agent\": 1,\n    \"personal\": 1,\n    \"repo\": 1\n  },\n  \"date\": \"2026-04-05\",\n  \"format\": 1\n}\n";
        let m = |n: &str, d: &[u8]| (n.to_owned(), d.to_vec());
        match key {
            "empty" => vec![],
            "one-note" => vec![m("personal/a.md", b"hello\n")],
            "manifest-and-notes" => vec![
                m("manifest.json", manifest.as_bytes()),
                m(
                    "personal/feedback-x.md",
                    b"---\nname: feedback-x\n---\nDo x\n",
                ),
                m(
                    "repo/agent-memory/anthology-w/feedback-w.md",
                    "agent \u{e9}\n".as_bytes(),
                ),
                m("repo/memory/project-r.md", b"repo\n"),
            ],
            "name-99" => vec![m(&name(99), b"x")],
            "name-100" => vec![m(&name(100), b"x")],
            "name-101" => vec![m(&name(101), b"x")],
            "name-150" => vec![m(&name(150), &[b'x'; 600])],
            "name-1002" => vec![m(&name(1002), b"y")],
            "name-1003" => vec![m(&name(1003), b"y")],
            "sizes-512-0" => vec![
                m("personal/full.md", &[b'z'; 512]),
                m("personal/empty.md", b""),
            ],
            "exact-record" => vec![m("personal/r.md", &[b'r'; 8704])],
            "two-records" => vec![m("personal/big.md", &[b'q'; 20000])],
            _ => unreachable!(),
        }
    }

    #[test]
    fn tar_stream_matches_cpython_3_12() {
        for &(key, len, digest) in GOLDENS {
            let owned = set(key);
            let members: Vec<(&str, &[u8])> = owned
                .iter()
                .map(|(n, d)| (n.as_str(), d.as_slice()))
                .collect();
            let tar = pax_archive(&members);
            assert_eq!((tar.len(), sha(&tar).as_str()), (len, digest), "{key}");
        }
    }

    #[test]
    fn pax_record_length_counts_its_own_digits() {
        assert_eq!(pax_record("path", &"a".repeat(91)).len(), 101);
        assert!(pax_record("path", &"a".repeat(91)).starts_with(b"101 path="));
        assert!(pax_record("path", &"a".repeat(989)).starts_with(b"999 path="));
        assert!(pax_record("path", &"a".repeat(990)).starts_with(b"1001 path="));
    }

    #[test]
    fn gzip_header_is_cpythons_and_round_trips() {
        let gz = gzip(b"abc");
        // python:3.12-slim: GzipFile(filename="", mode="wb", fileobj=buf, mtime=0) of b"abc".
        assert_eq!(gz[..10], [0x1f, 0x8b, 0x08, 0, 0, 0, 0, 0, 0x02, 0xff]);
        assert_eq!(gz[gz.len() - 8..], [0xc2, 0x41, 0x24, 0x35, 3, 0, 0, 0]);
        for payload in [
            Vec::new(),
            b"abc".to_vec(),
            pax_archive(&[("personal/a.md", b"hi\n")]),
        ] {
            let mut back = Vec::new();
            flate2::read::GzDecoder::new(gzip(&payload).as_slice())
                .read_to_end(&mut back)
                .unwrap();
            assert_eq!(back, payload);
        }
    }

    #[test]
    fn the_tar_crate_reads_the_stream_back() {
        let long = name(150);
        let tar = pax_archive(&[(long.as_str(), b"long\n"), ("personal/a.md", b"")]);
        let mut archive = tar::Archive::new(tar.as_slice());
        let got: Vec<(String, u64)> = archive
            .entries()
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                (e.path().unwrap().to_string_lossy().into_owned(), e.size())
            })
            .collect();
        assert_eq!(got, vec![(long, 5), ("personal/a.md".to_owned(), 0)]);
    }
}
