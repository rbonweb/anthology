//! The SQLite memory index and `anthology mem` (MI1): `memory_index.py`'s database, queries and
//! CLI (GC-12, GC-5, GC-4).
//!
//! The Markdown notes are the source of truth; the database is a catalog built from them. It
//! lives in the plugin data folder (never in a repo), carries a schema version
//! ([`Limits::schema_version`], `PRAGMA user_version`), and is rebuilt whenever that version
//! changes or the file is not a usable database. The schema text is Python's, verbatim, so
//! `sqlite_master` reads the same; the version is **2** (F-11), so an index Python built (1) is
//! rebuilt once. FTS5 is always there (bundled SQLite), so Python's LIKE fallback is gone (F-11,
//! I-11).
//!
//! Paths behave as in Python: a relative source or database path is relative to the process's
//! working folder, and messages show paths as `pathlib` prints them.
//!
//! Where Python raised an exception the CLI does not catch (a traceback, exit 1), the Rust CLI
//! prints `anthology-mem: <Type>: <message>` and exits 1 (D-1).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::ErrorKind;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use anthology_core::Env;
use anthology_core::clock::{Date, today_local};
use anthology_core::git::{Git, RealGit};
use anthology_core::repos::{REPO_SCAN_CAP, repos_below};
use anthology_pycompat::argparse::{self, Arg, Nargs, Spec, Subcommands, Type};
use anthology_pycompat::json::{self, Int, Map, Value};
use anthology_pycompat::pyerr::PyExc;
use anthology_pycompat::shlex;
use anthology_pycompat::str::{char_slice, lower, split_ws, strip_ws};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

pub use crate::{approve, notes, repo, save};

use notes::NoteInfo;
use save::{SaveError, Saved};

/// `PRAGMA user_version` of an index this port builds (F-11; Python's is 1).
pub const SCHEMA_VERSION: i64 = 2;
/// The typed relations a note's `metadata.relations` may name, in the order they are stored.
pub const RELATION_KINDS: [&str; 3] = ["supersedes", "depends_on", "relates_to"];
/// The number of search hits.
pub const SEARCH_LIMIT: i64 = 20;

/// The constants Python's tests patch (GC-19).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// `SCHEMA_VERSION`.
    pub schema_version: i64,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            schema_version: SCHEMA_VERSION,
        }
    }
}

/// Python's `_SCHEMA`, verbatim.
const SCHEMA: &str = "
CREATE TABLE files(path TEXT PRIMARY KEY, mtime INTEGER, size INTEGER, sha256 TEXT);
CREATE TABLE nodes(id TEXT PRIMARY KEY, name TEXT, type TEXT, scope TEXT, file TEXT,
                   description TEXT, updated TEXT);
CREATE TABLE refs(src TEXT, target TEXT, kind TEXT);
CREATE TABLE edges(src TEXT, dst TEXT, kind TEXT);
CREATE INDEX edges_src ON edges(src);
CREATE INDEX edges_dst ON edges(dst);
CREATE INDEX nodes_name ON nodes(name);
";

const FTS: &str = "CREATE VIRTUAL TABLE notes_fts USING fts5(name, description, body, \
                   id UNINDEXED, tokenize = 'porter unicode61')";

/// An error of the index, as the Python exception it stands for. The CLI prints
/// `anthology-mem: <py_str>` for the three classes it catches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// `ValueError` (the database location checks, a path SQLite cannot take).
    Value(String),
    /// `sqlite3.Error`: SQLite's message.
    Sqlite(String),
    /// `OSError`.
    Os(PyExc),
    /// An exception the CLI does not catch: `(class, message)`.
    Uncaught(&'static str, String),
}

impl Error {
    /// `str(exc)`.
    pub fn py_str(&self) -> String {
        match self {
            Error::Value(m) | Error::Sqlite(m) | Error::Uncaught(_, m) => m.clone(),
            Error::Os(e) => e.py_str(),
        }
    }
}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Error {
        match e {
            rusqlite::Error::SqliteFailure(_, Some(m)) => Error::Sqlite(m),
            other => Error::Sqlite(other.to_string()),
        }
    }
}

/// An `OSError` naming `path` as `pathlib` prints it.
fn os_error(e: std::io::Error, path: &Path) -> Error {
    let errno = match PyExc::from(e) {
        PyExc::OSError { errno, .. } => errno,
        _ => 5,
    };
    Error::Os(PyExc::OSError {
        errno,
        filename: Some(shown(path)),
    })
}

/// `sqlite3.connect(path)`: read-write, created when missing. (The bundled SQLite is built with
/// `SQLITE_USE_URI`, as the oracle image's is, so a `file:` name is a URI on both sides.) A
/// failed open's message is SQLite's alone: rusqlite appends `: <path>` to it, Python does not.
fn connect(path: &Path) -> rusqlite::Result<Connection> {
    use rusqlite::{Error as E, OpenFlags, ffi::ErrorCode};
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_CREATE
        | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(path, flags).map_err(|e| match e {
        E::SqliteFailure(f, Some(m)) if f.code == ErrorCode::CannotOpen => {
            let suffix = format!(": {}", path.to_string_lossy());
            let m = m.strip_suffix(&suffix).map(str::to_string).unwrap_or(m);
            E::SqliteFailure(f, Some(m))
        }
        other => other,
    })?;
    // `sqlite3.connect`'s default `timeout=5.0`: wait for a lock instead of failing at once.
    conn.busy_timeout(BUSY_TIMEOUT)?;
    Ok(conn)
}

/// Python's `sqlite3.connect` default wait for a locked database.
const BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

// ---- small Python helpers --------------------------------------------------------------------

/// `str(pathlib.PurePosixPath(p))`: repeated and trailing `/` and `.` parts dropped (`..` kept),
/// a leading `//` (exactly two) kept, an empty path `.`.
pub fn pathlib_str(p: &[u8]) -> Vec<u8> {
    let (root, rest): (&[u8], &[u8]) = if p.starts_with(b"//") && !p.starts_with(b"///") {
        (b"//", &p[2..])
    } else if p.starts_with(b"/") {
        (b"/", p)
    } else {
        (b"", p)
    };
    let parts: Vec<&[u8]> = rest
        .split(|&b| b == b'/')
        .filter(|s| !s.is_empty() && *s != b".")
        .collect();
    let mut out = root.to_vec();
    out.extend_from_slice(&parts.join(&b'/'));
    if out.is_empty() {
        out.push(b'.');
    }
    out
}

fn shown(p: &Path) -> String {
    String::from_utf8_lossy(&pathlib_str(p.as_os_str().as_bytes())).into_owned()
}

/// `os.path.splitext(name)[0]` (leading dots never start an extension).
pub fn py_stem(name: &[u8]) -> &[u8] {
    let Some(dot) = name.iter().rposition(|&b| b == b'.') else {
        return name;
    };
    if name[..dot].iter().all(|&b| b == b'.') {
        return name;
    }
    &name[..dot]
}

fn basename(p: &[u8]) -> &[u8] {
    match p.iter().rposition(|&b| b == b'/') {
        Some(i) => &p[i + 1..],
        None => p,
    }
}

/// `os.path.dirname`: the head without its trailing slashes (unless it is all slashes).
fn dirname(p: &[u8]) -> &[u8] {
    let head = match p.iter().rposition(|&b| b == b'/') {
        Some(i) => &p[..=i],
        None => return b"",
    };
    if head.iter().all(|&b| b == b'/') {
        head
    } else {
        let end = head.iter().rposition(|&b| b != b'/').map_or(0, |i| i + 1);
        &head[..end]
    }
}

/// The code points of `b` decoded with `surrogateescape`: Python's `str` order for a path.
fn py_order_key(b: &[u8]) -> Vec<u32> {
    let mut out = Vec::with_capacity(b.len());
    for chunk in b.utf8_chunks() {
        out.extend(chunk.valid().chars().map(u32::from));
        out.extend(chunk.invalid().iter().map(|&x| 0xDC00 + u32::from(x)));
    }
    out
}

/// The `ValueError` sqlite3 raises when a str parameter holds a surrogate (a path that is not
/// UTF-8): `'utf-8' codec can't encode character(s) ...`.
fn surrogate_error(b: &[u8]) -> Error {
    let key = py_order_key(b);
    let start = key
        .iter()
        .position(|c| (0xD800..0xE000).contains(c))
        .unwrap_or(0);
    let end = key[start..]
        .iter()
        .position(|c| !(0xD800..0xE000).contains(c))
        .map_or(key.len(), |n| start + n);
    let msg = if end - start == 1 {
        format!(
            "'utf-8' codec can't encode character '\\u{:04x}' in position {start}: surrogates not allowed",
            key[start]
        )
    } else {
        format!(
            "'utf-8' codec can't encode characters in position {start}-{}: surrogates not allowed",
            end - 1
        )
    };
    Error::Value(msg)
}

fn utf8(b: &[u8]) -> Result<&str, Error> {
    std::str::from_utf8(b).map_err(|_| surrogate_error(b))
}

/// `os.path.abspath`.
fn abspath(p: &Path) -> Vec<u8> {
    let b = p.as_os_str().as_bytes();
    if b.starts_with(b"/") {
        return anthology_pycompat::path::normpath_bytes(b);
    }
    let cwd = std::env::current_dir().unwrap_or_default();
    let joined = cwd.join(p);
    anthology_pycompat::path::normpath_bytes(joined.as_os_str().as_bytes())
}

fn realpath_bytes(p: &Path) -> Vec<u8> {
    anthology_pycompat::path::realpath(p)
        .into_os_string()
        .into_vec()
}

// ---- notes -------------------------------------------------------------------------------

/// `extract_links(body)`: the targets of `[[note]]` links, in order, without duplicates.
pub fn extract_links(body: &str) -> Vec<String> {
    static LINK: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = LINK.get_or_init(|| {
        regex::Regex::new(r"\[\[([^\[\]|#\n]+)(?:[|#][^\[\]\n]*)?\]\]").expect("link pattern")
    });
    let mut seen: Vec<String> = Vec::new();
    for c in re.captures_iter(body) {
        let target = strip_ws(&c[1]).to_string();
        if !target.is_empty() && !seen.contains(&target) {
            seen.push(target);
        }
    }
    seen
}

fn note_type(info: &NoteInfo) -> String {
    match info.kind.as_deref() {
        Some(k) if !k.is_empty() => k.to_string(),
        _ => "other".to_string(),
    }
}

fn note_updated(info: &NoteInfo) -> String {
    info.updated.clone().unwrap_or_default()
}

// ---- the database ------------------------------------------------------------------------

/// One note folder to index: scope `repo`, `agent` or `personal`, and its folder (each agent
/// folder on its own).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub scope: String,
    pub dir: PathBuf,
}

impl Source {
    pub fn new(scope: &str, dir: impl Into<PathBuf>) -> Source {
        Source {
            scope: scope.to_string(),
            dir: dir.into(),
        }
    }
}

/// What [`build`] did: Python's `{"added", "updated", "removed", "rebuilt"}`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub added: u64,
    pub updated: u64,
    pub removed: u64,
    pub rebuilt: bool,
}

impl Stats {
    /// The JSON object `mem rebuild --json` prints (keys sorted by the dump).
    pub fn to_value(self) -> Value {
        let mut m = Map::default();
        m.insert("added".into(), Value::Int(Int::Small(self.added as i64)));
        m.insert(
            "updated".into(),
            Value::Int(Int::Small(self.updated as i64)),
        );
        m.insert(
            "removed".into(),
            Value::Int(Int::Small(self.removed as i64)),
        );
        m.insert("rebuilt".into(), Value::Bool(self.rebuilt));
        Value::Object(m)
    }
}

fn is_within(path: &[u8], dir: &[u8]) -> bool {
    if path == dir {
        return true;
    }
    let end = dir.iter().rposition(|&b| b != b'/').map_or(0, |i| i + 1);
    let mut prefix = dir[..end].to_vec();
    prefix.push(b'/');
    path.starts_with(&prefix)
}

/// `_check_db_location`: the database never lives inside a notes folder, nor inside the git
/// work tree around a repo or agent folder (a personal folder may sit in a dotfiles repo, R23a).
fn check_db_location(sources: &[Source], db_path: &Path) -> Result<(), Error> {
    let db = realpath_bytes(db_path);
    for s in sources {
        let src = realpath_bytes(&s.dir);
        if is_within(&db, &src) {
            return Err(Error::Value(format!(
                "the index database must not live inside a notes folder: {}",
                shown(db_path)
            )));
        }
        if s.scope == "personal" {
            continue;
        }
        let mut probe = src;
        loop {
            let mut git = probe.clone();
            if !git.ends_with(b"/") {
                git.push(b'/');
            }
            git.extend_from_slice(b".git");
            if fs::symlink_metadata(OsStr::from_bytes(&git)).is_ok() {
                if is_within(&db, &probe) {
                    return Err(Error::Value(format!(
                        "the index database must not live inside a git work tree: {}",
                        shown(db_path)
                    )));
                }
                break;
            }
            let parent = dirname(&probe).to_vec();
            if parent == probe {
                break;
            }
            probe = parent;
        }
    }
    Ok(())
}

/// `Path.mkdir(parents=True, exist_ok=True)`.
fn mkdir_parents(p: &Path) -> Result<(), Error> {
    match fs::create_dir(p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            let parent = p.parent().filter(|q| !q.as_os_str().is_empty());
            match parent {
                Some(q) if q != p => {
                    mkdir_parents(q)?;
                    match fs::create_dir(p) {
                        Ok(()) => Ok(()),
                        Err(_) if p.is_dir() => Ok(()),
                        Err(e) => Err(os_error(e, p)),
                    }
                }
                _ => Err(os_error(e, p)),
            }
        }
        Err(_) if p.is_dir() => Ok(()),
        Err(e) => Err(os_error(e, p)),
    }
}

/// `_remove_db_files`: the database and its `-wal`, `-shm` and `-journal` files.
pub fn remove_db_files(db_path: &Path) -> Result<(), Error> {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut name = db_path.as_os_str().to_os_string();
        name.push(suffix);
        let p = PathBuf::from(name);
        match fs::remove_file(&p) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(os_error(e, &p)),
        }
    }
    Ok(())
}

/// `_has_current_schema`: whether the index at `db_path` can be kept. A database that is busy or
/// locked (or that cannot be read for an I/O error) is neither current nor stale: its error is
/// returned and the file is left alone (D-8, LR-M: Python treats every `sqlite3.Error` as "stale"
/// and deletes the database out from under the process holding the lock). Any other error is
/// "stale", as in Python.
fn has_current_schema(db_path: &Path, limits: &Limits) -> Result<bool, Error> {
    use rusqlite::{Error as E, ffi::ErrorCode};
    let check = || -> rusqlite::Result<bool> {
        let conn = connect(db_path)?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version != limits.schema_version {
            return Ok(false);
        }
        conn.query_row("SELECT count(*) FROM files", [], |r| r.get::<_, i64>(0))?;
        Ok(true)
    };
    match check() {
        Ok(current) => Ok(current),
        Err(E::SqliteFailure(f, m))
            if matches!(
                f.code,
                ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked | ErrorCode::SystemIoFailure
            ) =>
        {
            Err(E::SqliteFailure(f, m).into())
        }
        Err(_) => Ok(false),
    }
}

fn create_schema(conn: &Connection, limits: &Limits) -> Result<(), Error> {
    conn.execute_batch(SCHEMA)?;
    conn.execute_batch(FTS)?;
    conn.execute_batch(&format!("PRAGMA user_version = {}", limits.schema_version))?;
    Ok(())
}

struct Found {
    path: Vec<u8>,
    id: Vec<u8>,
    scope: String,
    mtime: i64,
    size: i64,
}

/// `_scan`: every note file of the sources, by absolute path (`MEMORY.md`, symlinks and
/// anything not a regular file skipped).
fn scan(sources: &[Source]) -> Result<BTreeMap<Vec<u32>, Found>, Error> {
    let mut found = BTreeMap::new();
    for s in sources {
        if !s.dir.is_dir() {
            continue;
        }
        let prefix: Vec<u8> = if s.scope == "agent" {
            let mut p = s
                .dir
                .file_name()
                .map(|n| n.as_bytes().to_vec())
                .unwrap_or_default();
            p.push(b'/');
            p
        } else {
            Vec::new()
        };
        let rd = fs::read_dir(&s.dir).map_err(|e| os_error(e, &s.dir))?;
        let mut entries: Vec<fs::DirEntry> = Vec::new();
        for e in rd {
            entries.push(e.map_err(|e| os_error(e, &s.dir))?);
        }
        entries.sort_by_key(|e| py_order_key(e.file_name().as_bytes()));
        for entry in entries {
            let name = entry.file_name();
            let nb = name.as_bytes();
            if !nb.ends_with(b".md") || nb == b"MEMORY.md" {
                continue;
            }
            match entry.file_type() {
                Ok(t) if t.is_file() => {}
                _ => continue,
            }
            let Ok(meta) = fs::symlink_metadata(entry.path()) else {
                continue;
            };
            if !meta.file_type().is_file() {
                continue;
            }
            let path = abspath(&entry.path());
            let mut id = format!("{}:", s.scope).into_bytes();
            id.extend_from_slice(&prefix);
            id.extend_from_slice(nb);
            let mtime = meta.mtime() * 1_000_000_000 + meta.mtime_nsec();
            found.insert(
                py_order_key(&path),
                Found {
                    path,
                    id,
                    scope: s.scope.clone(),
                    mtime,
                    size: meta.size() as i64,
                },
            );
        }
    }
    Ok(found)
}

fn drop_node(conn: &Connection, id: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM notes_fts WHERE id = ?", [id])?;
    conn.execute("DELETE FROM refs WHERE src = ?", [id])?;
    conn.execute("DELETE FROM nodes WHERE id = ?", [id])?;
    Ok(())
}

fn store_node(
    conn: &Connection,
    id: &str,
    scope: &str,
    path: &str,
    info: &NoteInfo,
) -> rusqlite::Result<()> {
    drop_node(conn, id)?;
    conn.execute(
        "INSERT INTO nodes(id, name, type, scope, file, description, updated) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
        params![
            id,
            info.name,
            note_type(info),
            scope,
            path,
            info.description,
            note_updated(info)
        ],
    )?;
    conn.execute(
        "INSERT INTO notes_fts(name, description, body, id) VALUES (?, ?, ?, ?)",
        params![info.name, info.description, info.body, id],
    )?;
    for target in extract_links(&info.body) {
        conn.execute(
            "INSERT INTO refs(src, target, kind) VALUES (?, ?, 'links')",
            params![id, target],
        )?;
    }
    let r = &info.relations;
    for (kind, targets) in RELATION_KINDS
        .iter()
        .zip([&r.supersedes, &r.depends_on, &r.relates_to])
    {
        for target in targets {
            conn.execute(
                "INSERT INTO refs(src, target, kind) VALUES (?, ?, ?)",
                params![id, target, kind],
            )?;
        }
    }
    Ok(())
}

fn file_stem(file: &str) -> String {
    String::from_utf8_lossy(py_stem(basename(file.as_bytes()))).into_owned()
}

/// `_rebuild_edges`: each reference resolved to a node by name, else by file stem, preferring
/// the source's scope, then the smallest id; an unresolved reference keeps its target text.
fn rebuild_edges(conn: &Connection) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare("SELECT id, name, scope, file FROM nodes")?;
    let nodes: Vec<(String, String, String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut by_name: HashMap<&str, Vec<(&str, &str)>> = HashMap::new();
    let mut by_stem: HashMap<String, Vec<(&str, &str)>> = HashMap::new();
    let mut scope_of: HashMap<&str, &str> = HashMap::new();
    for (id, name, scope, file) in &nodes {
        by_name.entry(name).or_default().push((id, scope));
        by_stem
            .entry(file_stem(file))
            .or_default()
            .push((id, scope));
        scope_of.insert(id, scope);
    }
    let mut refs = conn.prepare("SELECT src, target, kind FROM refs")?;
    let refs: Vec<(String, String, String)> = refs
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut edges: BTreeSet<(String, String, String)> = BTreeSet::new();
    for (src, target, kind) in refs {
        let Some(src_scope) = scope_of.get(src.as_str()) else {
            continue;
        };
        let candidates = by_name
            .get(target.as_str())
            .or_else(|| by_stem.get(&target));
        let dst = candidates.map(|c| {
            let same: Vec<&(&str, &str)> = c.iter().filter(|x| x.1 == *src_scope).collect();
            let pool: Vec<&(&str, &str)> = if same.is_empty() {
                c.iter().collect()
            } else {
                same
            };
            pool.into_iter()
                .min()
                .map(|x| x.0.to_string())
                .unwrap_or_default()
        });
        if dst.as_deref() == Some(src.as_str()) {
            continue;
        }
        edges.insert((src, dst.unwrap_or(target), kind));
    }
    conn.execute("DELETE FROM edges", [])?;
    let mut ins = conn.prepare("INSERT INTO edges(src, dst, kind) VALUES (?, ?, ?)")?;
    for (s, d, k) in &edges {
        ins.execute(params![s, d, k])?;
    }
    Ok(())
}

/// `build(sources, db_path)`: brings the index at `db_path` up to date with the notes of
/// `sources`, incrementally (by mtime and size, then sha256), or from scratch when the schema
/// version differs or the file is not a usable database.
pub fn build(sources: &[Source], db_path: &Path, limits: &Limits) -> Result<Stats, Error> {
    check_db_location(sources, db_path)?;
    if let Some(parent) = db_path.parent().filter(|p| !p.as_os_str().is_empty()) {
        mkdir_parents(parent)?;
    }
    let mut stats = Stats::default();
    if db_path.exists() && !has_current_schema(db_path, limits)? {
        remove_db_files(db_path)?;
        stats.rebuilt = true;
    }
    let mut conn = connect(db_path)?;
    let has_files = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE name = 'files'",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !has_files {
        create_schema(&conn, limits)?;
    }
    let disk = scan(sources)?;
    let known: HashMap<Vec<u8>, (i64, i64, String)> = {
        let mut stmt = conn.prepare("SELECT path, mtime, size, sha256 FROM files")?;
        stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?.into_bytes(),
                (r.get(1)?, r.get(2)?, r.get(3)?),
            ))
        })?
        .collect::<rusqlite::Result<_>>()?
    };
    let on_disk: BTreeSet<&[u8]> = disk.values().map(|f| f.path.as_slice()).collect();
    let tx = conn.transaction()?;
    let mut gone: Vec<&Vec<u8>> = known
        .keys()
        .filter(|p| !on_disk.contains(p.as_slice()))
        .collect();
    gone.sort_by_key(|p| py_order_key(p));
    for path in gone {
        let path = utf8(path)?;
        let ids: Vec<String> = {
            let mut stmt = tx.prepare("SELECT id FROM nodes WHERE file = ?")?;
            stmt.query_map([path], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        for id in ids {
            drop_node(&tx, &id)?;
        }
        tx.execute("DELETE FROM files WHERE path = ?", [path])?;
        stats.removed += 1;
    }
    for f in disk.values() {
        let old = known.get(&f.path);
        if let Some(o) = old
            && o.0 == f.mtime
            && o.1 == f.size
        {
            continue;
        }
        let Ok(data) = fs::read(OsStr::from_bytes(&f.path)) else {
            continue;
        };
        let sha = hex(&Sha256::digest(&data));
        if let Some(o) = old
            && o.2 == sha
        {
            tx.execute(
                "UPDATE files SET mtime = ?, size = ? WHERE path = ?",
                params![f.mtime, f.size, utf8(&f.path)?],
            )?;
            continue;
        }
        let stem = String::from_utf8_lossy(py_stem(basename(&f.path))).into_owned();
        let info = notes::parse_text(&String::from_utf8_lossy(&data), &stem);
        // sqlite3 binds the id first, then the path (a name from a non-UTF-8 stem implies a
        // non-UTF-8 id).
        let id = utf8(&f.id)?;
        let path = utf8(&f.path)?;
        store_node(&tx, id, &f.scope, path, &info)?;
        tx.execute(
            "INSERT OR REPLACE INTO files(path, mtime, size, sha256) VALUES (?, ?, ?, ?)",
            params![path, f.mtime, f.size, sha],
        )?;
        if old.is_some() {
            stats.updated += 1;
        } else {
            stats.added += 1;
        }
    }
    if stats.added > 0 || stats.updated > 0 || stats.removed > 0 {
        rebuild_edges(&tx)?;
    }
    tx.commit()?;
    Ok(stats)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

// ---- queries -----------------------------------------------------------------------------

fn open(db_path: &Path) -> Result<Option<Connection>, Error> {
    if !db_path.exists() {
        return Ok(None);
    }
    Ok(Some(connect(db_path)?))
}

/// A search hit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub name: String,
    pub scope: String,
    pub file: String,
    pub snippet: String,
}

/// `search(q, db_path)`: the notes that match every whitespace-separated term of `q`, best
/// first (`bm25`), at most `limit`. Each term is quoted, so user input is never FTS syntax.
pub fn search(q: &str, db_path: &Path, limit: i64) -> Result<Vec<Hit>, Error> {
    let terms = split_ws(q);
    let Some(conn) = open(db_path)? else {
        return Ok(Vec::new());
    };
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    let query: Vec<String> = terms
        .iter()
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect();
    let mut stmt = conn.prepare(
        "SELECT n.name, n.scope, n.file, snippet(notes_fts, -1, '', '', '…', 12) \
         FROM notes_fts JOIN nodes n ON n.id = notes_fts.id \
         WHERE notes_fts MATCH ? ORDER BY bm25(notes_fts), n.name, n.id LIMIT ?",
    )?;
    let rows = stmt
        .query_map(params![query.join(" "), limit], |r| {
            let snippet: Option<String> = r.get(3)?;
            Ok(Hit {
                name: r.get(0)?,
                scope: r.get(1)?,
                file: r.get(2)?,
                snippet: split_ws(snippet.as_deref().unwrap_or("")).join(" "),
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

/// A note reached by [`related`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Related {
    pub name: String,
    pub scope: String,
    pub file: String,
    pub kind: String,
    pub distance: i64,
    /// `out` when the walk followed the edge, `in` when it went against it.
    pub direction: &'static str,
}

/// `related(name, depth)`: the notes within `depth` hops of `name` (by name or file stem),
/// following edges both ways. Python loops `depth` times even after the walk is done; this
/// stops when nothing is left to visit, with the same result.
pub fn related(name: &str, depth: i64, db_path: &Path) -> Result<Vec<Related>, Error> {
    let Some(conn) = open(db_path)? else {
        return Ok(Vec::new());
    };
    if depth < 1 {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare("SELECT id, name, scope, file FROM nodes")?;
    let nodes: HashMap<String, (String, String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?, r.get(3)?))))?
        .collect::<rusqlite::Result<_>>()?;
    let mut adj: HashMap<String, Vec<(String, String, &'static str)>> = HashMap::new();
    let mut stmt = conn.prepare("SELECT src, dst, kind FROM edges")?;
    let edges: Vec<(String, String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (src, dst, kind) in edges {
        if nodes.contains_key(&src) && nodes.contains_key(&dst) {
            adj.entry(src.clone())
                .or_default()
                .push((dst.clone(), kind.clone(), "out"));
            adj.entry(dst).or_default().push((src, kind, "in"));
        }
    }
    let mut start: Vec<&String> = nodes
        .iter()
        .filter(|(_, n)| n.0 == name || file_stem(&n.2) == name)
        .map(|(id, _)| id)
        .collect();
    start.sort();
    let mut seen: BTreeSet<String> = start.iter().map(|s| s.to_string()).collect();
    let mut frontier: Vec<String> = start.into_iter().cloned().collect();
    let mut found = Vec::new();
    let mut distance = 1;
    while distance <= depth && !frontier.is_empty() {
        let mut next = Vec::new();
        for node in &frontier {
            let mut out = adj.get(node).cloned().unwrap_or_default();
            out.sort();
            for (other, kind, direction) in out {
                if seen.insert(other.clone()) {
                    let n = &nodes[&other];
                    found.push(Related {
                        name: n.0.clone(),
                        scope: n.1.clone(),
                        file: n.2.clone(),
                        kind,
                        distance,
                        direction,
                    });
                    next.push(other);
                }
            }
        }
        frontier = next;
        distance += 1;
    }
    found.sort_by(|a, b| (a.distance, &a.name, &a.file).cmp(&(b.distance, &b.name, &b.file)));
    Ok(found)
}

/// A note of [`orphans`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Orphan {
    pub name: String,
    pub scope: String,
    pub file: String,
}

/// `orphans()`: the notes no other note links to.
pub fn orphans(db_path: &Path) -> Result<Vec<Orphan>, Error> {
    let Some(conn) = open(db_path)? else {
        return Ok(Vec::new());
    };
    let mut stmt = conn.prepare(
        "SELECT n.name, n.scope, n.file FROM nodes n WHERE NOT EXISTS (\
         SELECT 1 FROM edges e JOIN nodes s ON s.id = e.src WHERE e.dst = n.id) \
         ORDER BY n.name, n.id",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(Orphan {
                name: r.get(0)?,
                scope: r.get(1)?,
                file: r.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

/// A note of [`stale`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StaleNote {
    pub name: String,
    pub scope: String,
    pub file: String,
    pub reason: String,
}

/// `date.toordinal()` (proleptic Gregorian, 0001-01-01 is 1).
pub fn to_ordinal(y: i64, m: i64, d: i64) -> i64 {
    let y1 = y - 1;
    let before_year = y1 * 365 + y1 / 4 - y1 / 100 + y1 / 400;
    const DAYS_BEFORE_MONTH: [i64; 13] = [0, 0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let before_month = DAYS_BEFORE_MONTH[m as usize] + i64::from(m > 2 && leap);
    before_year + before_month + d
}

/// `date.fromordinal(n)` for `1 <= n <= 3_652_059`.
pub fn from_ordinal(n: i64) -> Date {
    let mut y = (n * 400) / 146_097 + 1;
    while to_ordinal(y, 1, 1) > n {
        y -= 1;
    }
    while to_ordinal(y + 1, 1, 1) <= n {
        y += 1;
    }
    let mut m = 12;
    while to_ordinal(y, m, 1) > n {
        m -= 1;
    }
    let d = n - to_ordinal(y, m, 1) + 1;
    Date {
        y: y as i32,
        m: m as u8,
        d: d as u8,
    }
}

fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn valid_date(y: i64, m: i64, d: i64) -> Option<Date> {
    let ok = (1..=9999).contains(&y) && (1..=12).contains(&m) && d >= 1 && d <= days_in_month(y, m);
    ok.then_some(Date {
        y: y as i32,
        m: m as u8,
        d: d as u8,
    })
}

/// CPython 3.12's `date.fromisoformat(s)` (the C parser: 7, 8 or 10 UTF-8 bytes; `YYYY-MM-DD`,
/// `YYYYMMDD`, and the ISO week forms; what follows the parsed fields is not checked).
pub fn fromisoformat(s: &str) -> Option<Date> {
    let b = s.as_bytes();
    if ![7, 8, 10].contains(&b.len()) {
        return None;
    }
    let at = |i: usize| b.get(i).copied().unwrap_or(0);
    let digits = |p: usize, n: usize| -> Option<i64> {
        let mut v = 0i64;
        for i in 0..n {
            let c = at(p + i);
            if !c.is_ascii_digit() {
                return None;
            }
            v = v * 10 + i64::from(c - b'0');
        }
        Some(v)
    };
    let year = digits(0, 4)?;
    let mut p = 4;
    let sep = at(p) == b'-';
    if sep {
        p += 1;
    }
    if at(p) == b'W' {
        p += 1;
        let week = digits(p, 2)?;
        p += 2;
        let day = if p < b.len() {
            if sep {
                if at(p) != b'-' {
                    return None;
                }
                p += 1;
            }
            digits(p, 1)?
        } else {
            1
        };
        if !(1..=9999).contains(&year) {
            return None;
        }
        if !(1..53).contains(&week) {
            let first_weekday = (to_ordinal(year, 1, 1) + 6) % 7;
            let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
            if !(week == 53 && (first_weekday == 3 || (first_weekday == 2 && leap))) {
                return None;
            }
        }
        if !(1..8).contains(&day) {
            return None;
        }
        let first = to_ordinal(year, 1, 1);
        let first_weekday = (first + 6) % 7;
        let mut week1_monday = first - first_weekday;
        if first_weekday > 3 {
            week1_monday += 7;
        }
        let n = week1_monday + (week - 1) * 7 + day - 1;
        if !(1..=3_652_059).contains(&n) {
            return None;
        }
        return Some(from_ordinal(n));
    }
    let month = digits(p, 2)?;
    p += 2;
    if sep {
        if at(p) != b'-' {
            return None;
        }
        p += 1;
    }
    let day = digits(p, 2)?;
    valid_date(year, month, day)
}

/// `stale(db_path, days, today)`: the notes a `supersedes` relation points at, and the notes
/// whose date is older than `days` before `today`.
pub fn stale(db_path: &Path, days: &Int, today: Date) -> Result<Vec<StaleNote>, Error> {
    let Some(conn) = open(db_path)? else {
        return Ok(Vec::new());
    };
    let days_shown = match days {
        Int::Small(n) => n.to_string(),
        Int::Big(t) => t.clone(),
    };
    // `timedelta(days=n)`: a C int first, then at most 999,999,999 days.
    let n = match days {
        Int::Small(n) if n.unsigned_abs() <= 999_999_999 => *n,
        Int::Small(n) if i32::try_from(*n).is_ok() => {
            return Err(Error::Uncaught(
                "OverflowError",
                format!("days={days_shown}; must have magnitude <= 999999999"),
            ));
        }
        _ => {
            return Err(Error::Uncaught(
                "OverflowError",
                "Python int too large to convert to C int".into(),
            ));
        }
    };
    let cutoff = to_ordinal(i64::from(today.y), i64::from(today.m), i64::from(today.d)) - n;
    if !(1..=3_652_059).contains(&cutoff) {
        return Err(Error::Uncaught(
            "OverflowError",
            "date value out of range".into(),
        ));
    }
    let mut out: BTreeMap<String, StaleNote> = BTreeMap::new();
    let mut stmt = conn.prepare(
        "SELECT DISTINCT d.id, d.name, d.scope, d.file FROM edges e \
         JOIN nodes s ON s.id = e.src JOIN nodes d ON d.id = e.dst \
         WHERE e.kind = 'supersedes'",
    )?;
    let rows: Vec<(String, String, String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (id, name, scope, file) in rows {
        out.insert(
            id,
            StaleNote {
                name,
                scope,
                file,
                reason: "superseded".into(),
            },
        );
    }
    let mut stmt = conn.prepare("SELECT id, name, scope, file, updated FROM nodes")?;
    let rows: Vec<(String, String, String, String, Option<String>)> = stmt
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    for (id, name, scope, file, updated) in rows {
        let updated = updated.unwrap_or_default();
        if out.contains_key(&id) || updated.is_empty() {
            continue;
        }
        let head = char_slice(&updated, 0, 10);
        let Some(when) = fromisoformat(head) else {
            continue;
        };
        if to_ordinal(i64::from(when.y), i64::from(when.m), i64::from(when.d)) < cutoff {
            out.insert(
                id,
                StaleNote {
                    name,
                    scope,
                    file,
                    reason: format!("older than {days_shown} days ({head})"),
                },
            );
        }
    }
    let mut rows: Vec<StaleNote> = out.into_values().collect();
    rows.sort_by(|a, b| (&a.name, &a.file).cmp(&(&b.name, &b.file)));
    Ok(rows)
}

// ---- locating the database ---------------------------------------------------------------

/// `db_path_for(data_dir, key)`: `<data_dir>/memory-index/<slug>-<sha256(key)[:12]>.db`; the
/// slug is the key's last two `/`-parts, lowercased, every character outside `[a-z0-9._-]`
/// made `-`, cut to 40 characters, leading `.` and `-` stripped, else `repo` (GC-1).
pub fn db_path_for(data_dir: &Path, key: &str) -> PathBuf {
    let parts: Vec<&str> = key.split('/').filter(|p| !p.is_empty()).collect();
    let tail = parts[parts.len().saturating_sub(2)..].join("-");
    let slug: String = lower(&tail)
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .take(40)
        .collect();
    let slug = slug.trim_start_matches(['.', '-']);
    let slug = if slug.is_empty() { "repo" } else { slug };
    let digest = hex(&Sha256::digest(key.as_bytes()));
    data_dir
        .join("memory-index")
        .join(format!("{slug}-{}.db", &digest[..12]))
}

// ---- the `anthology mem` CLI ---------------------------------------------------------------

/// What a command produces (GC-18).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Output {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub code: i32,
}

/// The CLI's seams (GC-19).
pub struct MemDeps<'a> {
    /// `os.getcwd()`; `None` is the process's working folder.
    pub cwd: Option<PathBuf>,
    pub git: &'a dyn Git,
    /// `datetime.date.today()` (`stale`).
    pub today: fn() -> Date,
    pub limits: Limits,
}

impl Default for MemDeps<'_> {
    fn default() -> MemDeps<'static> {
        MemDeps {
            cwd: None,
            git: &RealGit,
            today: today_local,
            limits: Limits::default(),
        }
    }
}

/// The `anthology-mem` argument parser (`memory_index._parser`).
pub fn parser(env: &Env) -> Spec {
    let json = || {
        Arg::option(&["--json"])
            .store_true()
            .help("machine-readable output")
    };
    let mut save = Spec::command().allow_abbrev(false);
    for flag in ["agent", "type", "slug", "name", "description"] {
        let option = format!("--{flag}");
        save = save.arg(Arg::option(&[option.as_str()]).required());
    }
    save = save
        .mutex(
            true,
            vec![
                Arg::option(&["--body"]).help("the note text, as one argument"),
                Arg::option(&["--body-file"]).metavar("PATH").help(
                    "read the note text from PATH, a file in the repo or the temp folder ('-' \
                     reads stdin); use it for long text or lines a command check may \
                     misread; same checks as --body",
                ),
            ],
        )
        .arg(
            Arg::option(&["--replace"])
                .store_true()
                .help("overwrite an existing note"),
        )
        .arg(json());
    let sub = Subcommands::new()
        .dest("command")
        .required()
        .parser(
            "search",
            Some("full-text search over note bodies"),
            Spec::command()
                .arg(Arg::positional("terms").nargs(Nargs::OneOrMore))
                .arg(json()),
        )
        .parser(
            "related",
            Some("notes linked to a note, to a depth"),
            Spec::command()
                .arg(Arg::positional("note"))
                .arg(
                    Arg::option(&["--depth"])
                        .ty(Type::Int)
                        .default(argparse::Value::Int(Int::Small(2))),
                )
                .arg(json()),
        )
        .parser(
            "orphans",
            Some("notes nothing links to"),
            Spec::command().arg(json()),
        )
        .parser(
            "stale",
            Some("superseded or old notes"),
            Spec::command()
                .arg(
                    Arg::option(&["--days"])
                        .ty(Type::Int)
                        .default(argparse::Value::Int(Int::Small(180))),
                )
                .arg(json()),
        )
        .parser(
            "rebuild",
            Some("delete and rebuild the index"),
            Spec::command().arg(json()),
        )
        .parser(
            "save",
            Some("save an agent memory note (no heredoc needed)"),
            save,
        );
    Spec::new("anthology-mem")
        .description("Search and explore Anthology memory notes.")
        .subcommands(sub)
        .width(argparse::help_width(env.get("COLUMNS"), None))
}

struct Out {
    stdout: String,
    stderr: String,
}

impl Out {
    fn line(&mut self, s: &str) {
        self.stdout.push_str(s);
        self.stdout.push('\n');
    }

    fn err(&mut self, s: &str) {
        self.stderr.push_str(s);
        self.stderr.push('\n');
    }

    fn done(self, code: i32) -> Output {
        Output {
            stdout: self.stdout.into_bytes(),
            stderr: self.stderr.into_bytes(),
            code,
        }
    }
}

fn s(v: &str) -> Value {
    Value::Str(v.to_string())
}

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    let mut m = Map::default();
    for (k, v) in pairs {
        m.insert(k.to_string(), v);
    }
    Value::Object(m)
}

fn print_rows(out: &mut Out, as_json: bool, rows: Vec<(Value, String)>) {
    if as_json {
        let list = Value::Array(rows.into_iter().map(|(v, _)| v).collect());
        out.line(&json::dumps(&list, &json::INDENT2_SORTED));
        return;
    }
    if rows.is_empty() {
        out.line("(none)");
    }
    for (_, line) in rows {
        out.line(&line);
    }
}

fn arg_str(p: &argparse::Parsed, dest: &str) -> String {
    p.os_str(dest)
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn arg_int(p: &argparse::Parsed, dest: &str) -> Int {
    match p.get(dest) {
        Some(argparse::Value::Int(n)) => n.clone(),
        _ => Int::Small(0),
    }
}

/// `os.getcwd()`; its failure is uncaught in Python (`FileNotFoundError` and the like).
fn cwd_of(deps: &MemDeps) -> Result<PathBuf, (&'static str, String)> {
    match &deps.cwd {
        Some(c) => Ok(c.clone()),
        None => std::env::current_dir().map_err(|e| {
            let exc = PyExc::from(e);
            (exc.type_name(), exc.py_str())
        }),
    }
}

/// Python's traceback, written as the one exception line (D-1).
fn uncaught(out: &mut Out, ty: &str, msg: &str) {
    out.err(&format!("anthology-mem: {ty}: {msg}"));
}

/// The most repo names the refusal lists.
const HINT_NAMES: usize = 5;

/// The second line of the "not inside a git work tree" refusal: the git repos one level below
/// `cwd` (a session started in a folder that holds several repos), or `None` when there are
/// none or the folder cannot be listed.
fn repos_below_hint(cwd: &Path) -> Option<String> {
    let repos = repos_below(cwd, REPO_SCAN_CAP);
    let first = repos.first()?;
    let mut list = repos[..repos.len().min(HINT_NAMES)].join(", ");
    if repos.len() > HINT_NAMES {
        list.push_str(&format!(" and {} more", repos.len() - HINT_NAMES));
    }
    Some(format!(
        "anthology-mem: git repos sit below this folder: {list}. Run the command from inside \
         one, for example `cd {} && anthology-mem save ...`, or start Claude Code inside the \
         repo.",
        shlex::quote(first)
    ))
}

fn run_save(p: &argparse::Parsed, env: &Env, deps: &MemDeps) -> Output {
    let mut out = Out {
        stdout: String::new(),
        stderr: String::new(),
    };
    let cwd = match cwd_of(deps) {
        Ok(c) => c,
        Err((ty, msg)) => {
            uncaught(&mut out, ty, &msg);
            return out.done(1);
        }
    };
    let Some(root) = repo::repo_root(&cwd, env, deps.git) else {
        out.err("anthology-mem: not inside a git work tree; memory notes live in the repo");
        if let Some(hint) = repos_below_hint(&cwd) {
            out.err(&hint);
        }
        return out.done(1);
    };
    let body = match p.os_str("body_file") {
        Some(arg) => match read_body_file(arg, &cwd, &root, env) {
            Ok(text) => text,
            Err(m) => {
                out.err(&format!("anthology-mem: {m}"));
                return out.done(1);
            }
        },
        None => arg_str(p, "body"),
    };
    let saved: Result<Saved, SaveError> = save::save_note(
        &root,
        &arg_str(p, "agent"),
        &arg_str(p, "type"),
        &arg_str(p, "slug"),
        &arg_str(p, "name"),
        &arg_str(p, "description"),
        &body,
        p.flag("replace"),
        None,
        env,
    );
    let saved = match saved {
        Ok(s) => s,
        Err(SaveError(m)) => {
            out.err(&format!("anthology-mem: {m}"));
            return out.done(1);
        }
    };
    let root_text = saved.root.to_string_lossy().into_owned();
    if p.flag("json") {
        let v = obj(vec![
            ("action", s(&saved.action)),
            ("path", s(&saved.relative)),
            ("root", s(&root_text)),
        ]);
        out.line(&json::dumps(&v, &json::PY_DEFAULT_SORTED));
    } else {
        out.line(&format!(
            "{} {} in {}",
            saved.action, saved.relative, root_text
        ));
    }
    if let Some(w) = &saved.index_warning {
        out.err(&format!(
            "anthology-mem: note saved, but MEMORY.md was not regenerated: {w}"
        ));
    }
    if !saved.similar.is_empty() {
        out.err(&format!(
            "anthology-mem: warning: similar note(s) already exist: {}; if this repeats one of \
             them, update it (same --type and --slug, with --replace) instead of adding another",
            saved.similar.join(", ")
        ));
    }
    out.done(0)
}

/// The system temp folders a `--body-file` may be in besides the repo (`$TMPDIR` is added).
const TEMP_ROOTS: [&str; 4] = [
    "/tmp",
    "/private/tmp",
    "/var/folders",
    "/private/var/folders",
];

/// The path policy of `--body-file`, on the file's real path (symlinks resolved): inside the
/// repo or a temp folder, and at no obvious secret location. `shown` is the path as given.
/// The secret names are the guard's own (`anthology_shell::is_secret_path`, case-insensitive),
/// checked on the path as written too, so a `.env` is refused whatever it links to.
fn check_body_file_path(
    given: &Path,
    real: &Path,
    root: &Path,
    env: &Env,
    shown: &str,
) -> Result<(), String> {
    let text = |p: &Path| p.to_string_lossy().into_owned();
    if anthology_shell::is_secret_path(&text(given)) || anthology_shell::is_secret_path(&text(real))
    {
        return Err(format!(
            "refusing to read the body file {shown}: it is at a secret location (a key, \
             credentials, .env or shell history file); copy the note text to a plain file"
        ));
    }
    let real_of = |p: &Path| fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let inside = std::iter::once(real_of(root))
        .chain(TEMP_ROOTS.iter().map(|t| real_of(Path::new(t))))
        .chain(
            env.get_os("TMPDIR")
                .filter(|t| !t.is_empty())
                .map(|t| real_of(Path::new(t))),
        )
        .any(|r| real.starts_with(r));
    if !inside {
        return Err(format!(
            "refusing to read the body file {shown}: it must be inside the repo or the temp folder"
        ));
    }
    Ok(())
}

/// `--body-file`: the text of the file `arg` names (relative to `cwd`), or of standard input
/// when `arg` is `-`. A file must pass [`check_body_file_path`] and be a regular file (checked
/// before it is opened, so a FIFO is never waited on). A body over the limit is refused, never
/// cut: the file's size is checked, and the read is bounded in case it grows. The text is not
/// checked further here: `save` applies the checks of an inline body.
fn read_body_file(arg: &OsStr, cwd: &Path, root: &Path, env: &Env) -> Result<String, String> {
    use std::io::Read;
    use std::os::fd::AsFd;
    let max = save::MAX_BODY_BYTES as u64;
    let shown = arg.to_string_lossy().into_owned();
    let cannot = |e: std::io::Error| {
        format!(
            "cannot read the body file {shown}: {}",
            crate::sync::strerror(&PyExc::from(e))
        )
    };
    let too_long = || format!("the body file {shown} is too long (at most {max} bytes)");
    let not_regular = || format!("the body file {shown} is not a regular file");
    let mut bytes = Vec::new();
    if arg == "-" {
        // A duplicate of fd 0, not `std::io::stdin()`: the binary's `main` holds that lock for
        // the whole run, so locking it again here would wait forever. Only the bound limits
        // what stdin can give.
        let fd = std::io::stdin()
            .as_fd()
            .try_clone_to_owned()
            .map_err(cannot)?;
        fs::File::from(fd)
            .take(max + 1)
            .read_to_end(&mut bytes)
            .map_err(cannot)?;
    } else {
        let given = cwd.join(arg);
        let real = fs::canonicalize(&given).map_err(cannot)?;
        check_body_file_path(&given, &real, root, env, &shown)?;
        if !fs::metadata(&real).is_ok_and(|m| m.is_file()) {
            return Err(not_regular());
        }
        let file = fs::File::open(&real).map_err(cannot)?;
        let meta = file.metadata().map_err(cannot)?;
        if !meta.is_file() {
            return Err(not_regular());
        }
        if meta.len() > max {
            return Err(too_long());
        }
        file.take(max + 1).read_to_end(&mut bytes).map_err(cannot)?;
    }
    if bytes.len() as u64 > max {
        return Err(too_long());
    }
    String::from_utf8(bytes).map_err(|_| format!("the body file {shown} is not valid UTF-8"))
}

/// Python's truthiness of a JSON value.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Int(Int::Small(n)) => *n != 0,
        Value::Int(Int::Big(_)) => true,
        Value::Float(f) => *f != 0.0,
        Value::Str(t) => !t.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(m) => m.iter().next().is_some(),
    }
}

fn py_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Int(_) => "int",
        Value::Float(_) => "float",
        Value::Str(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// `anthology mem …` (`memory_index.main`): `args` are the arguments after `mem`.
pub fn main(args: &[OsString], env: &Env, deps: &MemDeps) -> Output {
    let parsed = match argparse::parse(&parser(env), args) {
        Ok(p) => p,
        Err(e) => {
            return Output {
                stdout: e.stdout.into_bytes(),
                stderr: e.stderr.into_bytes(),
                code: e.code,
            };
        }
    };
    let command = parsed.str("command").unwrap_or("").to_string();
    if command == "save" {
        return run_save(&parsed, env, deps);
    }
    let mut out = Out {
        stdout: String::new(),
        stderr: String::new(),
    };
    let state = anthology_core::state::read(env);
    // A truthy `data_dir` that is not a str: Python 3.12's `Path()` raises a TypeError, after
    // the repo lookup (`not_str` holds its type name until then).
    let mut not_str: Option<&'static str> = None;
    let data_dir: Option<PathBuf> = match state.get("data_dir") {
        Some(v) if truthy(v) => match v {
            Value::Str(t) => Some(PathBuf::from(t)),
            other => {
                not_str = Some(py_type_name(other));
                Some(PathBuf::new())
            }
        },
        _ => env
            .get_os("CLAUDE_PLUGIN_DATA")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from),
    };
    let Some(data_dir) = data_dir else {
        out.err("anthology-mem: run /anthology:setup first");
        return out.done(1);
    };
    let home = repo::anthology_home(env);
    let cwd = match cwd_of(deps) {
        Ok(c) => c,
        Err((ty, msg)) => {
            uncaught(&mut out, ty, &msg);
            return out.done(1);
        }
    };
    let root = repo::repo_root(&cwd, env, deps.git);
    let approved_key = root.as_ref().and_then(|r| {
        let key = repo::repo_key(r, env, deps.git);
        approve::is_approved(&key, env).then_some(key)
    });
    // `note_sources(None, home)` is personal only (§3.6 takes a root, so the other scopes are
    // filtered out).
    let (key, sources): (String, Vec<Source>) = match (&root, approved_key) {
        (Some(r), Some(key)) => {
            let src = repo::note_sources(r, &home)
                .into_iter()
                .map(|n| Source::new(&n.scope, n.dir))
                .collect();
            (key, src)
        }
        _ => {
            let src = repo::note_sources(root.as_deref().unwrap_or(&home), &home)
                .into_iter()
                .filter(|n| n.scope == "personal")
                .map(|n| Source::new(&n.scope, n.dir))
                .collect();
            ("personal".to_string(), src)
        }
    };
    if let Some(ty) = not_str {
        let msg = format!(
            "argument should be a str or an os.PathLike object where __fspath__ returns a str, \
             not '{ty}'"
        );
        uncaught(&mut out, "TypeError", &msg);
        return out.done(1);
    }
    let db = db_path_for(&data_dir, &key);
    match run_query(&mut out, &command, &parsed, &sources, &db, deps) {
        Ok(()) => out.done(0),
        Err(Error::Uncaught(ty, msg)) => {
            uncaught(&mut out, ty, &msg);
            out.done(1)
        }
        Err(e) => {
            out.err(&format!("anthology-mem: {}", e.py_str()));
            out.done(1)
        }
    }
}

fn run_query(
    out: &mut Out,
    command: &str,
    p: &argparse::Parsed,
    sources: &[Source],
    db: &Path,
    deps: &MemDeps,
) -> Result<(), Error> {
    if command == "rebuild" {
        remove_db_files(db)?;
    }
    let stats = build(sources, db, &deps.limits)?;
    let as_json = p.flag("json");
    match command {
        "rebuild" => {
            if as_json {
                out.line(&json::dumps(&stats.to_value(), &json::PY_DEFAULT_SORTED));
            } else {
                out.line(&format!("rebuilt the index: {} notes", stats.added));
            }
        }
        "search" => {
            let terms: Vec<String> = p
                .list("terms")
                .iter()
                .filter_map(|v| v.as_os_str().map(|s| s.to_string_lossy().into_owned()))
                .collect();
            let rows = search(&terms.join(" "), db, SEARCH_LIMIT)?
                .into_iter()
                .map(|r| {
                    let line = format!("{} [{}] {}\n    {}", r.name, r.scope, r.file, r.snippet);
                    let v = obj(vec![
                        ("name", s(&r.name)),
                        ("scope", s(&r.scope)),
                        ("file", s(&r.file)),
                        ("snippet", s(&r.snippet)),
                    ]);
                    (v, line)
                })
                .collect();
            print_rows(out, as_json, rows);
        }
        "related" => {
            let depth = match arg_int(p, "depth") {
                Int::Small(n) => n,
                Int::Big(t) if t.starts_with('-') => -1,
                Int::Big(_) => i64::MAX,
            };
            let rows = related(&arg_str(p, "note"), depth, db)?
                .into_iter()
                .map(|r| {
                    let line = format!(
                        "{}  {} [{}] ({}, {}) {}",
                        r.distance, r.name, r.scope, r.kind, r.direction, r.file
                    );
                    let v = obj(vec![
                        ("name", s(&r.name)),
                        ("scope", s(&r.scope)),
                        ("file", s(&r.file)),
                        ("kind", s(&r.kind)),
                        ("distance", Value::Int(Int::Small(r.distance))),
                        ("direction", s(r.direction)),
                    ]);
                    (v, line)
                })
                .collect();
            print_rows(out, as_json, rows);
        }
        "orphans" => {
            let rows = orphans(db)?
                .into_iter()
                .map(|r| {
                    let line = format!("{} [{}] {}", r.name, r.scope, r.file);
                    let v = obj(vec![
                        ("name", s(&r.name)),
                        ("scope", s(&r.scope)),
                        ("file", s(&r.file)),
                    ]);
                    (v, line)
                })
                .collect();
            print_rows(out, as_json, rows);
        }
        "stale" => {
            let rows = stale(db, &arg_int(p, "days"), (deps.today)())?
                .into_iter()
                .map(|r| {
                    let line = format!("{} [{}] {}: {}", r.name, r.scope, r.reason, r.file);
                    let v = obj(vec![
                        ("name", s(&r.name)),
                        ("scope", s(&r.scope)),
                        ("file", s(&r.file)),
                        ("reason", s(&r.reason)),
                    ]);
                    (v, line)
                })
                .collect();
            print_rows(out, as_json, rows);
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A second connection holds the write lock: `build` waits, then reports "database is
    /// locked" and leaves the database exactly as it was (it is never deleted or rebuilt).
    #[test]
    fn a_locked_database_is_reported_and_never_deleted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let notes = dir.path().join("notes");
        fs::create_dir(&notes).expect("mkdir");
        fs::write(notes.join("a.md"), "# a\n\nhello zebra\n").expect("write");
        let db = dir.path().join("index.db");
        let sources = [Source::new("personal", &notes)];
        let limits = Limits::default();
        let stats = build(&sources, &db, &limits).expect("first build");
        assert_eq!(stats.added, 1);
        let before = fs::read(&db).expect("read db");

        let holder = connect(&db).expect("holder");
        holder.execute_batch("BEGIN EXCLUSIVE").expect("lock");
        let started = std::time::Instant::now();
        let err = build(&sources, &db, &limits).expect_err("busy");
        assert!(
            started.elapsed() >= BUSY_TIMEOUT / 2,
            "it waited for the lock"
        );
        assert!(err.py_str().contains("locked"), "{}", err.py_str());
        assert!(db.exists(), "the database survived");
        assert_eq!(fs::read(&db).expect("read db"), before);
        holder.execute_batch("ROLLBACK").expect("unlock");
        drop(holder);

        let again = build(&sources, &db, &limits).expect("build after unlock");
        assert!(!again.rebuilt, "no rebuild: the index was intact");
        assert_eq!(again.added, 0);
        assert_eq!((again.updated, again.removed), (0, 0));
    }

    #[test]
    fn pathlib_str_normalizes_as_pathlib() {
        for (a, b) in [
            ("a//b/./c/", "a/b/c"),
            ("", "."),
            ("/", "/"),
            ("//x", "//x"),
            ("///x", "/x"),
            ("./a/../b", "a/../b"),
        ] {
            assert_eq!(pathlib_str(a.as_bytes()), b.as_bytes(), "{a}");
        }
    }

    #[test]
    fn py_stem_keeps_leading_dots() {
        for (a, b) in [
            ("a.md", "a"),
            (".md", ".md"),
            ("..md", "..md"),
            ("a.b.md", "a.b"),
            ("noext", "noext"),
        ] {
            assert_eq!(py_stem(a.as_bytes()), b.as_bytes(), "{a}");
        }
    }

    #[test]
    fn fromisoformat_follows_cpython_312() {
        let d = |s: &str| fromisoformat(s).map(|d| d.iso());
        assert_eq!(d("2026-01-02").as_deref(), Some("2026-01-02"));
        assert_eq!(d("20260102").as_deref(), Some("2026-01-02"));
        assert_eq!(d("2026-W01-1").as_deref(), Some("2025-12-29"));
        assert_eq!(d("2026W011").as_deref(), Some("2025-12-29"));
        assert_eq!(d("2026W01").as_deref(), Some("2025-12-29"));
        assert_eq!(d("20260102ab").as_deref(), Some("2026-01-02"));
        assert_eq!(d("2026-02-30"), None);
        assert_eq!(d("0000-01-01"), None);
        assert_eq!(d("2026-01"), None);
        assert_eq!(d("2026-0102"), None);
        assert_eq!(d("2026/01/02"), None);
        assert_eq!(d("2020-W53-1").as_deref(), Some("2020-12-28"));
        assert_eq!(d("2021-W53-1"), None);
    }

    #[test]
    fn ordinals_round_trip() {
        for n in [1, 2, 365, 366, 730_120, 738_000, 3_652_059] {
            let d = from_ordinal(n);
            assert_eq!(
                to_ordinal(i64::from(d.y), i64::from(d.m), i64::from(d.d)),
                n
            );
        }
        assert_eq!(to_ordinal(1, 1, 1), 1);
        assert_eq!(from_ordinal(3_652_059).iso(), "9999-12-31");
    }

    #[test]
    fn surrogate_errors_name_the_run() {
        assert_eq!(
            surrogate_error(b"ab\xffc").py_str(),
            "'utf-8' codec can't encode character '\\udcff' in position 2: surrogates not allowed"
        );
        assert_eq!(
            surrogate_error(b"\xc3\xa9\xff\xfe").py_str(),
            "'utf-8' codec can't encode characters in position 1-2: surrogates not allowed"
        );
    }
}
