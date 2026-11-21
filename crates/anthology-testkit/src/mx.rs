//! Test helpers of lane MX (plan §0.2): archives built the way `tests/test_memory_portable.py`
//! builds them with `tarfile` (`add`, `make_archive`, `MaliciousArchiveTest.bad` and the
//! special archives of its size tests), and its `tree()` snapshot.
//!
//! The tar headers are written byte for byte as CPython 3.12's `TarInfo.tobuf` writes them
//! (PAX format, or GNU format where the Python test asks for it), so a reader sees exactly what
//! it sees in Python's archives, hostile names included. Only the gzip wrapper differs: no file
//! name and mtime 0 in its header, and flate2's deflate bytes (a reader ignores both).

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read as _, Write};
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::write::GzEncoder;

/// `MIB`.
pub const MIB: u64 = 1024 * 1024;
/// `MANIFEST`: `json.dumps({"format": 1, "date": "2026-01-01", "counts": {}})`.
pub const MANIFEST: &[u8] = br#"{"format": 1, "date": "2026-01-01", "counts": {}}"#;

const BLOCK: u64 = 512;
const RECORD: u64 = 10240;

/// One member of [`make_archive`] (the `add` helper's `type_`, `data`, `linkname`, `size`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Spec {
    /// A regular file with these bytes.
    File(Vec<u8>),
    /// A regular file of `n` zero bytes, streamed (`ZeroFile(n)`), so huge members cost nothing.
    Zero(u64),
    Dir,
    Symlink(String),
    Hardlink(String),
    Char,
    Block,
    Fifo,
}

impl Spec {
    fn typeflag(&self) -> u8 {
        match self {
            Spec::File(_) | Spec::Zero(_) => b'0',
            Spec::Dir => b'5',
            Spec::Symlink(_) => b'2',
            Spec::Hardlink(_) => b'1',
            Spec::Char => b'3',
            Spec::Block => b'4',
            Spec::Fifo => b'6',
        }
    }

    fn size(&self) -> u64 {
        match self {
            Spec::File(d) => d.len() as u64,
            Spec::Zero(n) => *n,
            _ => 0,
        }
    }

    fn linkname(&self) -> &str {
        match self {
            Spec::Symlink(l) | Spec::Hardlink(l) => l,
            _ => "",
        }
    }
}

/// `stn(s, len, "ascii", "replace")`: a non-ASCII character becomes one `?`.
fn ascii_replace(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| if c.is_ascii() { c as u8 } else { b'?' })
        .collect()
}

/// `itn(n, digits)` for a value below `8 ** (digits - 1)`.
fn octal(field: &mut [u8], n: u64) {
    let digits = field.len() - 1;
    let text = format!("{n:0digits$o}");
    assert_eq!(text.len(), digits, "number field overflow");
    field[..digits].copy_from_slice(text.as_bytes());
    field[digits] = 0;
}

/// `TarInfo._create_header` with mode 0644 (or 0 for an extended header), uid, gid and mtime
/// 0, empty uname, gname and prefix. `gnu` picks the GNU magic and format.
fn raw_header(name: &[u8], typeflag: u8, size: u64, linkname: &[u8], gnu: bool) -> [u8; 512] {
    let mut h = [0u8; 512];
    let n = name.len().min(100);
    h[..n].copy_from_slice(&name[..n]);
    let mode = if typeflag == b'x' && name == b"././@PaxHeader" {
        0
    } else {
        0o644
    };
    octal(&mut h[100..108], mode);
    octal(&mut h[108..116], 0);
    octal(&mut h[116..124], 0);
    octal(&mut h[124..136], size);
    octal(&mut h[136..148], 0);
    h[148..156].fill(b' ');
    h[156] = typeflag;
    let l = linkname.len().min(100);
    h[157..157 + l].copy_from_slice(&linkname[..l]);
    h[257..265].copy_from_slice(if gnu { b"ustar  \0" } else { b"ustar\x0000" });
    if matches!(typeflag, b'3' | b'4') {
        octal(&mut h[329..337], 0);
        octal(&mut h[337..345], 0);
    }
    let sum: u32 = h.iter().map(|&b| u32::from(b)).sum();
    octal(&mut h[148..155], u64::from(sum));
    h
}

/// One pax record, `"%d %s=%s\n"`, its length counting its own digits.
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

fn padded(mut bytes: Vec<u8>) -> Vec<u8> {
    let rem = bytes.len() as u64 % BLOCK;
    if rem > 0 {
        bytes.resize(bytes.len() + (BLOCK - rem) as usize, 0);
    }
    bytes
}

/// `TarInfo.create_pax_header`: the user's `pax` records first, then `path` / `linkpath` for a
/// name or linkname over 100 characters or not ASCII; then the ustar header.
fn pax_header(name: &str, spec: &Spec, pax: &[(&str, String)]) -> Vec<u8> {
    let mut name = name.to_owned();
    if *spec == Spec::Dir && !name.ends_with('/') {
        name.push('/');
    }
    let mut records: Vec<(&str, String)> = pax.to_vec();
    for (keyword, value) in [("path", name.as_str()), ("linkpath", spec.linkname())] {
        let has = records.iter().any(|(k, _)| *k == keyword);
        if !has && (!value.is_ascii() || value.chars().count() > 100) {
            records.push((keyword, value.to_owned()));
        }
    }
    let mut out = Vec::new();
    if !records.is_empty() {
        let body: Vec<u8> = records.iter().flat_map(|(k, v)| pax_record(k, v)).collect();
        out.extend_from_slice(&raw_header(
            b"././@PaxHeader",
            b'x',
            body.len() as u64,
            b"",
            false,
        ));
        out.extend(padded(body));
    }
    out.extend_from_slice(&raw_header(
        &ascii_replace(&name),
        spec.typeflag(),
        spec.size(),
        &ascii_replace(spec.linkname()),
        false,
    ));
    out
}

/// A `tarfile` writer in PAX format over any sink: `addfile`, then `close` (two zero blocks
/// and the padding to a 10240-byte record).
struct PyTar<W: Write> {
    out: W,
    offset: u64,
}

impl<W: Write> PyTar<W> {
    fn new(out: W) -> Self {
        PyTar { out, offset: 0 }
    }

    fn raw(&mut self, bytes: &[u8]) {
        self.out.write_all(bytes).expect("write the archive");
        self.offset += bytes.len() as u64;
    }

    fn zeros(&mut self, n: u64) {
        let copied =
            io::copy(&mut io::repeat(0).take(n), &mut self.out).expect("write the archive");
        self.offset += copied;
    }

    /// The `add` helper (with optional extra pax records).
    fn add(&mut self, name: &str, spec: &Spec, pax: &[(&str, String)]) {
        self.raw(&pax_header(name, spec, pax));
        match spec {
            Spec::File(data) => self.raw(data),
            Spec::Zero(n) => self.zeros(*n),
            _ => return,
        }
        let rem = spec.size() % BLOCK;
        if rem > 0 {
            self.zeros(BLOCK - rem);
        }
    }

    fn close(mut self) -> W {
        self.zeros(2 * BLOCK);
        let rem = self.offset % RECORD;
        if rem > 0 {
            self.zeros(RECORD - rem);
        }
        self.out
    }
}

fn gz_file(path: &Path, level: u32) -> GzEncoder<io::BufWriter<fs::File>> {
    let file = fs::File::create(path).expect("create the archive");
    GzEncoder::new(io::BufWriter::new(file), Compression::new(level))
}

fn finish(gz: GzEncoder<io::BufWriter<fs::File>>) {
    gz.finish()
        .expect("finish the gzip stream")
        .flush()
        .expect("flush the archive");
}

/// `make_archive(path, members, manifest, level)`: `tarfile.open(path, "w:gz")` (PAX format,
/// the default) with `manifest.json` (= [`MANIFEST`]) first when `manifest`, then `members` in
/// order, each mode 0644, any name at all (unsafe names are the point). The helper's default
/// level is 6; `fast` is `compresslevel=1`. Returns `path`.
pub fn make_archive(path: &Path, members: &[(&str, Spec)], manifest: bool, fast: bool) -> PathBuf {
    let mut tar = PyTar::new(gz_file(path, if fast { 1 } else { 6 }));
    if manifest {
        tar.add("manifest.json", &Spec::File(MANIFEST.to_vec()), &[]);
    }
    for (name, spec) in members {
        tar.add(name, spec, &[]);
    }
    finish(tar.close());
    path.to_path_buf()
}

/// `MaliciousArchiveTest.KINDS`, in Python's order.
pub const KINDS: [&str; 36] = [
    "dotdot",
    "dotdot-inside",
    "absolute",
    "symlink",
    "hardlink",
    "chardev",
    "blockdev",
    "fifo",
    "symlink-folder",
    "outside-settings",
    "outside-subdir",
    "outside-agent-dotdot",
    "outside-agent-no-folder",
    "outside-toplevel",
    "outside-claude",
    "outside-memory-md",
    "outside-memory-md-lower",
    "outside-repo-memory-md",
    "outside-not-md",
    "outside-hidden",
    "outside-bad-agent-name",
    "dot-component",
    "empty-component",
    "newline-name",
    "newline-agent",
    "newline-trailing",
    "backslash",
    "backslash-traversal",
    "duplicate",
    "non-utf8",
    "no-manifest",
    "bad-manifest-format",
    "bad-manifest-json",
    "not-gzip",
    "empty-file",
    "truncated",
];

/// `MaliciousArchiveTest.GOOD`.
pub const GOOD: (&str, &[u8]) = ("personal/good.md", b"good\n");

/// `MaliciousArchiveTest.manifest_variant(path, manifest)`: `tarfile.open(path, "w:gz")` with
/// `manifest.json` = `manifest`, then [`GOOD`].
pub fn manifest_variant(path: &Path, manifest: &[u8]) -> PathBuf {
    let mut tar = PyTar::new(gz_file(path, 9));
    tar.add("manifest.json", &Spec::File(manifest.to_vec()), &[]);
    tar.add(GOOD.0, &Spec::File(GOOD.1.to_vec()), &[]);
    finish(tar.close());
    path.to_path_buf()
}

/// `MaliciousArchiveTest.truncate(path)`: keeps the first half of the file's bytes.
pub fn truncate(path: &Path) -> PathBuf {
    let data = fs::read(path).expect("read the archive");
    fs::write(path, &data[..data.len() / 2]).expect("write the archive");
    path.to_path_buf()
}

/// `MaliciousArchiveTest.bad(kind)`: writes `<dir>/bad-<kind>.tar.gz` and returns its path.
/// Panics on a kind that is not in [`KINDS`].
pub fn bad(dir: &Path, kind: &str) -> PathBuf {
    let p = dir.join(format!("bad-{kind}.tar.gz"));
    let file = |d: &[u8]| Spec::File(d.to_vec());
    let link = |l: &str| Spec::Symlink(l.to_owned());
    let one = |name: &str, spec: Spec| {
        make_archive(&p, &[(GOOD.0, file(GOOD.1)), (name, spec)], true, false)
    };
    match kind {
        "dotdot" => one("../evil.md", file(b"x")),
        "dotdot-inside" => one("personal/../../evil.md", file(b"x")),
        "absolute" => one("/etc/evil.md", file(b"x")),
        "symlink" => one("personal/link.md", link("/etc/passwd")),
        "hardlink" => one(
            "personal/hard.md",
            Spec::Hardlink("personal/good.md".to_owned()),
        ),
        "chardev" => one("personal/dev.md", Spec::Char),
        "blockdev" => one("personal/blk.md", Spec::Block),
        "fifo" => one("personal/fifo.md", Spec::Fifo),
        "symlink-folder" => one("personal", link("/etc")),
        "outside-settings" => one("repo/settings.json", file(b"{}")),
        "outside-subdir" => one("personal/sub/x.md", file(b"x")),
        "outside-agent-dotdot" => one("repo/agent-memory/../x.md", file(b"x")),
        "outside-agent-no-folder" => one("repo/agent-memory/x.md", file(b"x")),
        "outside-toplevel" => one("other/x.md", file(b"x")),
        "outside-claude" => one(".claude/settings.json", file(b"{}")),
        "outside-memory-md" => one("personal/MEMORY.md", file(b"x")),
        "outside-memory-md-lower" => one("personal/memory.md", file(b"x")),
        "outside-repo-memory-md" => one("repo/memory/MEMORY.md", file(b"x")),
        "outside-not-md" => one("personal/x.txt", file(b"x")),
        "outside-hidden" => one("personal/.x.md", file(b"x")),
        "outside-bad-agent-name" => one("repo/agent-memory/.hid/x.md", file(b"x")),
        "dot-component" => one("personal/./x.md", file(b"x")),
        "empty-component" => one("personal//x.md", file(b"x")),
        "newline-name" => one("personal/foo\n.md", file(b"x")),
        "newline-agent" => one("repo/agent-memory/ag\n/bar.md", file(b"x")),
        "newline-trailing" => one("personal/foo.md\n", file(b"x")),
        "backslash" => one("personal\\x.md", file(b"x")),
        "backslash-traversal" => one("personal\\..\\x.md", file(b"x")),
        "duplicate" => one(GOOD.0, file(GOOD.1)),
        "non-utf8" => one("personal/bin.md", file(b"\xff\xfe\xfa")),
        "no-manifest" => make_archive(&p, &[(GOOD.0, file(GOOD.1))], false, false),
        "bad-manifest-format" => manifest_variant(&p, br#"{"format": 99}"#),
        "bad-manifest-json" => manifest_variant(&p, b"not json"),
        "not-gzip" => {
            fs::write(&p, b"this is not an archive at all").expect("write the archive");
            p
        }
        "empty-file" => {
            fs::write(&p, b"").expect("write the archive");
            p
        }
        "truncated" => truncate(&one("personal/big.md", file(&[b'y'; 5000]))),
        _ => panic!("unknown malicious kind {kind:?}"),
    }
}

/// `test_declared_size_is_checked_before_any_data_is_read`: a gzip'd tar holding the manifest
/// (PAX format) and then only a GNU-format header for `personal/big.md` declaring `size` bytes
/// (no data, no end-of-archive blocks), gzip level 9.
pub fn declared_size_archive(path: &Path, size: u64) -> PathBuf {
    let mut gz = gz_file(path, 9);
    gz.write_all(&pax_header(
        "manifest.json",
        &Spec::File(MANIFEST.to_vec()),
        &[],
    ))
    .expect("write the archive");
    gz.write_all(&padded(MANIFEST.to_vec()))
        .expect("write the archive");
    gz.write_all(&raw_header(b"personal/big.md", b'0', size, b"", true))
        .expect("write the archive");
    finish(gz);
    path.to_path_buf()
}

/// `test_oversized_extended_header_is_not_read_into_memory`: a GNU-format header
/// `PaxHeader/x` of type `x` declaring `size` bytes, followed by `size` zero bytes, gzip level 1.
pub fn pax_header_bomb(path: &Path, size: u64) -> PathBuf {
    let mut gz = gz_file(path, 1);
    gz.write_all(&raw_header(b"PaxHeader/x", b'x', size, b"", true))
        .expect("write the archive");
    io::copy(&mut io::repeat(0).take(size), &mut gz).expect("write the archive");
    finish(gz);
    path.to_path_buf()
}

/// `test_headers_that_the_member_count_does_not_see_are_capped_too`: the manifest, then `n`
/// one-byte notes `personal/n%04d.md` (data `x`), each with a PAX `comment` record of
/// `comment_len` `x`s, gzip level 1.
pub fn pax_flood(path: &Path, n: usize, comment_len: usize) -> PathBuf {
    let mut tar = PyTar::new(gz_file(path, 1));
    tar.add("manifest.json", &Spec::File(MANIFEST.to_vec()), &[]);
    let comment = [("comment", "x".repeat(comment_len))];
    for i in 0..n {
        tar.add(
            &format!("personal/n{i:04}.md"),
            &Spec::File(b"x".to_vec()),
            &comment,
        );
    }
    finish(tar.close());
    path.to_path_buf()
}

/// `tree(root)`: `{relative path: Some("sha256:<hex>") for a file, Some("link:<target>") for a
/// symlink, None for a folder or anything else}` of everything under `root`, as `os.walk`
/// sees it: symlinks not followed, `.git` folders (or symlinks to folders) skipped, a missing
/// or unreadable folder empty.
pub fn tree(root: &Path) -> BTreeMap<String, Option<String>> {
    fn walk(dir: &Path, rel: &str, out: &mut BTreeMap<String, Option<String>>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            // `os.walk` sorts with `DirEntry.is_dir()`, which follows symlinks.
            let is_dir = fs::metadata(&path).is_ok_and(|m| m.is_dir());
            if is_dir && name == ".git" {
                continue;
            }
            let key = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            let meta = fs::symlink_metadata(&path).expect("lstat a walked entry");
            if meta.file_type().is_symlink() {
                let target = fs::read_link(&path).expect("read a symlink");
                out.insert(key, Some(format!("link:{}", target.to_string_lossy())));
            } else if meta.is_file() {
                let bytes = fs::read(&path).expect("read a walked file");
                out.insert(
                    key,
                    Some(format!("sha256:{}", crate::release::sha256_hex(&bytes))),
                );
            } else {
                out.insert(key.clone(), None);
                if meta.is_dir() {
                    walk(&path, &key, out);
                }
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, "", &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::GzDecoder;

    fn entries(path: &Path) -> Vec<(String, u8, u64)> {
        let mut archive = tar::Archive::new(GzDecoder::new(fs::File::open(path).unwrap()));
        archive
            .entries()
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                let name = String::from_utf8_lossy(&e.path_bytes()).into_owned();
                (name, e.header().entry_type().as_byte(), e.size())
            })
            .collect()
    }

    #[test]
    fn bad_writes_a_file_for_every_kind() {
        let dir = tempfile::tempdir().unwrap();
        for kind in KINDS {
            let p = bad(dir.path(), kind);
            assert_eq!(p, dir.path().join(format!("bad-{kind}.tar.gz")));
            assert!(p.is_file(), "{kind}");
        }
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), KINDS.len());
    }

    #[test]
    fn hostile_names_and_types_reach_the_reader_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let got = entries(&bad(dir.path(), "newline-agent"));
        assert_eq!(
            got[2],
            ("repo/agent-memory/ag\n/bar.md".to_owned(), b'0', 1)
        );
        let got = entries(&bad(dir.path(), "symlink-folder"));
        assert_eq!(got[2], ("personal".to_owned(), b'2', 0));
        let got = entries(&bad(dir.path(), "chardev"));
        assert_eq!(got[2], ("personal/dev.md".to_owned(), b'3', 0));
        let got = entries(&bad(dir.path(), "absolute"));
        assert_eq!(
            got[0],
            ("manifest.json".to_owned(), b'0', MANIFEST.len() as u64)
        );
        assert_eq!(got[2].0, "/etc/evil.md");
        let p = make_archive(
            &dir.path().join("d.tar.gz"),
            &[("personal", Spec::Dir)],
            false,
            true,
        );
        assert_eq!(entries(&p), vec![("personal/".to_owned(), b'5', 0)]);
    }

    /// Byte-identical to CPython 3.12's `tarfile` for the `add` helper (python:3.12-slim:
    /// `make_archive` of `[("personal/link.md", dict(type_=SYMTYPE, linkname="/etc/passwd")),
    /// ("personal/dev.md", dict(type_=CHRTYPE)), (name of 120 bytes, b"x"), ("personal",
    /// dict(type_=DIRTYPE))]`, `pax_flood(2, 8192)` and `declared_size_archive(5 GiB)`,
    /// decompressed).
    #[test]
    fn tar_stream_matches_cpython_3_12() {
        let dir = tempfile::tempdir().unwrap();
        let long = format!("personal/{}.md", "l".repeat(108));
        let p = make_archive(
            &dir.path().join("a.tar.gz"),
            &[
                ("personal/link.md", Spec::Symlink("/etc/passwd".to_owned())),
                ("personal/dev.md", Spec::Char),
                (long.as_str(), Spec::File(b"x".to_vec())),
                ("personal", Spec::Dir),
            ],
            true,
            false,
        );
        let mut tar = Vec::new();
        GzDecoder::new(fs::File::open(&p).unwrap())
            .read_to_end(&mut tar)
            .unwrap();
        assert_eq!(
            crate::release::sha256_hex(&tar),
            "2541501083ffaab7bc073c19e7911b7b6a381d688a71f06929a13921c900eb52",
        );
        let flood = pax_flood(&dir.path().join("f.tar.gz"), 2, 8192);
        let mut tar = Vec::new();
        GzDecoder::new(fs::File::open(&flood).unwrap())
            .read_to_end(&mut tar)
            .unwrap();
        assert_eq!(
            crate::release::sha256_hex(&tar),
            "8dca1c3c09858170cfd3447d742ba2fcc0f8829e55472b36bdd6332cabe34e92"
        );
        let declared = declared_size_archive(&dir.path().join("s.tar.gz"), 5 * 1024 * MIB);
        let mut tar = Vec::new();
        GzDecoder::new(fs::File::open(&declared).unwrap())
            .read_to_end(&mut tar)
            .unwrap();
        assert_eq!(
            crate::release::sha256_hex(&tar),
            "7dd038f6f2794ad531c93f6740ffba803b1c1f9392be142629927f844b05d0ba"
        );
    }

    #[test]
    fn zero_members_stream() {
        let dir = tempfile::tempdir().unwrap();
        let p = make_archive(
            &dir.path().join("z.tar.gz"),
            &[("personal/big.md", Spec::Zero(51 * MIB))],
            true,
            true,
        );
        assert!(fs::metadata(&p).unwrap().len() < 5 * MIB);
        assert_eq!(
            entries(&p)[1],
            ("personal/big.md".to_owned(), b'0', 51 * MIB)
        );
        let bomb = pax_header_bomb(&dir.path().join("b.tar.gz"), 3 * MIB);
        let mut all = Vec::new();
        GzDecoder::new(fs::File::open(&bomb).unwrap())
            .read_to_end(&mut all)
            .unwrap();
        assert_eq!(all.len() as u64, 512 + 3 * MIB);
        assert_eq!(
            crate::release::sha256_hex(&all[..512]),
            "59f75947423589b8c0103c359a0f1c4343499c357f18a8938b4ad35acad9065c"
        );
        assert_eq!(all[156], b'x');
    }

    #[test]
    fn tree_matches_the_python_definition() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert_eq!(tree(&root.join("missing")), BTreeMap::new());
        fs::create_dir_all(root.join("a/b")).unwrap();
        fs::write(root.join("a/b/f.md"), b"abc").unwrap();
        fs::create_dir_all(root.join(".git/objects")).unwrap();
        fs::create_dir_all(root.join("a/.git")).unwrap();
        std::os::unix::fs::symlink("a", root.join("dirlink")).unwrap();
        std::os::unix::fs::symlink("nowhere", root.join("dangling")).unwrap();
        let got = tree(root);
        let want: BTreeMap<String, Option<String>> = [
            ("a", None),
            ("a/b", None),
            (
                "a/b/f.md",
                Some("sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
            ),
            ("dangling", Some("link:nowhere")),
            ("dirlink", Some("link:a")),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.map(str::to_owned)))
        .collect();
        assert_eq!(got, want);
    }
}
