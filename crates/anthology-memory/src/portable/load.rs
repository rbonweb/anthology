//! Import validation (MX1): `memory_portable._load_archive` and its helpers. The archive is
//! read into memory once and parsed, never extracted; nothing here writes anywhere.
#![allow(dead_code, reason = "skeleton: MX1-B fills the bodies")]

use std::collections::BTreeMap;
use std::path::Path;

use anthology_pycompat::json;

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

/// `NAME.match(s)`: `^[A-Za-z0-9][A-Za-z0-9._-]*\Z`.
pub fn name_ok(s: &str) -> bool {
    let _ = s;
    todo!("MX1-B")
}

/// `_ok_note_name(filename)`: `<NAME>.md`, and not `memory.md` in any case.
pub fn ok_note_name(filename: &str) -> bool {
    let _ = filename;
    todo!("MX1-B")
}

/// `_check_name(name)`.
pub fn check_name(name: &str) -> Result<Kind, Rejected> {
    let _ = name;
    todo!("MX1-B")
}

/// `_load_archive(archive)`: validates the whole archive, then returns it. Never writes.
pub fn load_archive(archive: &Path) -> Result<Loaded, Rejected> {
    let _ = archive;
    todo!("MX1-B")
}
