//! Test helpers of lane MX (plan §0.2): archives built the way `tests/test_memory_portable.py`
//! builds them with `tarfile` (`add`, `make_archive`, `MaliciousArchiveTest.bad` and the
//! special archives of its size tests), and its `tree()` snapshot.
#![allow(dead_code, reason = "skeleton: MX1-A fills the bodies")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// `MIB`.
pub const MIB: u64 = 1024 * 1024;
/// `MANIFEST`: `json.dumps({"format": 1, "date": "2026-01-01", "counts": {}})`.
pub const MANIFEST: &[u8] = br#"{"format": 1, "date": "2026-01-01", "counts": {}}"#;

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

/// `make_archive(path, members, manifest, level)`: `tarfile.open(path, "w:gz")` (PAX format,
/// the default) with `manifest.json` (= [`MANIFEST`]) first when `manifest`, then `members` in
/// order, each mode 0644, any name at all (unsafe names are the point). `fast` is
/// `compresslevel=1`. Returns `path`.
pub fn make_archive(path: &Path, members: &[(&str, Spec)], manifest: bool, fast: bool) -> PathBuf {
    let _ = (path, members, manifest, fast);
    todo!("MX1-A")
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

/// `MaliciousArchiveTest.bad(kind)`: writes `<dir>/bad-<kind>.tar.gz` and returns its path.
pub fn bad(dir: &Path, kind: &str) -> PathBuf {
    let _ = (dir, kind);
    todo!("MX1-A")
}

/// `test_declared_size_is_checked_before_any_data_is_read`: a gzip'd tar holding the manifest
/// and then only a GNU-format header for `personal/big.md` declaring `size` bytes (no data, no
/// end-of-archive blocks).
pub fn declared_size_archive(path: &Path, size: u64) -> PathBuf {
    let _ = (path, size);
    todo!("MX1-A")
}

/// `test_oversized_extended_header_is_not_read_into_memory`: a GNU-format header
/// `PaxHeader/x` of type `x` declaring `size` bytes, followed by `size` zero bytes, gzip level 1.
pub fn pax_header_bomb(path: &Path, size: u64) -> PathBuf {
    let _ = (path, size);
    todo!("MX1-A")
}

/// `test_headers_that_the_member_count_does_not_see_are_capped_too`: the manifest, then `n`
/// one-byte notes `personal/n%04d.md`, each with a PAX `comment` record of `comment_len` `x`s,
/// gzip level 1.
pub fn pax_flood(path: &Path, n: usize, comment_len: usize) -> PathBuf {
    let _ = (path, n, comment_len);
    todo!("MX1-A")
}

/// `tree(root)`: `{relative path: Some("sha256:<hex>") for a file, Some("link:<target>") for a
/// symlink, None for a folder}` of everything under `root`, lexical, symlinks not followed,
/// `.git` skipped.
pub fn tree(root: &Path) -> BTreeMap<String, Option<String>> {
    let _ = root;
    todo!("MX1-A")
}
