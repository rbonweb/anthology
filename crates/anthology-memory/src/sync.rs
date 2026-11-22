//! Repo memory sync (MS2): the SessionStart memory step, legacy-note migration and the
//! `type: user` sweep. A port of `memory_sync.py`'s `migrate`, `sweep_user_notes`, the
//! per-clone repairs (`ensure_memory_dir`, `ensure_ignored`, `settings_tracked`) and
//! `run` (plan §3.6, GC-12, GC-19).
//!
//! Privacy: old path-keyed notes are copied into personal memory only (setup can move chosen,
//! clean ones into the repo), `type: user` notes are moved out of repo memory at session start,
//! and nothing else here writes a note into repo memory. Memory is active only in repos the user
//! approved; elsewhere nothing is written and no repo notes are injected.
//!
//! Every user-visible string is verbatim from `memory_sync.py`. Rust differs from Python only
//! where Python would block forever: a FIFO (or a link to one) where a JSON ledger or a
//! hand-made `MEMORY.md` is read is "unreadable" here, never opened for a blocking read; and
//! (D-8, Rust stricter) a hand-made `MEMORY.md` that is a symlink is never followed into the
//! session context.
//!
//! [`run`] never panics outward: an error, or a panic, becomes one `anthology memory: …`
//! message with an empty context, as Python's `except Exception`.
#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use anthology_core::clock::{self, Date};
use anthology_core::git::{Git, RealGit};
use anthology_core::{Env, fsx};
use anthology_pycompat::json::{self, Int, Map, Value};
use anthology_pycompat::pyerr::PyExc;
use anthology_pycompat::str as pystr;
use sha2::{Digest, Sha256};

use crate::privacy::{self, Finding, PrivacyDeps};

use crate::approve::is_approved;
use crate::notes::{NoteInfo, REPO_EXCLUDE, USER_REASON, is_user_note, normalize_type, note_info};
use crate::render::{
    Budget, CONTEXT_SECTION_BUDGET, DEFAULT_BUDGET, GENERATED, regenerate_index, render_index,
    try_list_notes,
};
use crate::repo::{legacy_dirs, repo_key, repo_root, unsafe_dir};

/// `HINT`: an unapproved repo that has memory notes.
pub const HINT: &str = "this repo has memory notes; repo notes load only after /anthology:setup, \
                        but notes in .claude/agent-memory still reach their agents, so review them";
/// `DIR_SET`: this clone's `autoMemoryDirectory` was written in this run.
pub const DIR_SET: &str = "anthology memory: set autoMemoryDirectory for this clone; its memory \
                           index is injected for this session";
/// `EXCLUDE_LINE`: what `ensure_ignored` adds to the repo's exclude file.
pub const EXCLUDE_LINE: &str = "/.claude/settings.local.json";

fn personal_save(personal: &str) -> String {
    format!(
        "Save notes about the user (type: user: role, preferences) here as \
         {personal}/user-<slug>.md, never in the repo."
    )
}

fn personal_unavailable(personal: &str) -> String {
    format!(
        "Personal memory is unavailable: {personal} is a symlink or not a folder, so do not \
         read or write notes there."
    )
}

/// What `migrate` did (`memory_sync.migrate`'s dict): file names. `side_by_side` is Python's
/// `conflicts`; `blocked` is `(file, reason)` and `blocked_findings[i]` the findings of
/// `blocked[i]` (Python's `{"file", "findings", "reason"}`). `skipped` and `blocked_findings`
/// are MS2 additions beyond plan §3.6.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MigrateResult {
    pub copied: Vec<String>,
    pub side_by_side: Vec<String>,
    pub blocked: Vec<(String, String)>,
    pub promoted: Vec<String>,
    pub skipped: Vec<String>,
    pub blocked_findings: Vec<Vec<Finding>>,
}

/// What `sweep_user_notes` did: `moved` file names, and the `conflicts` among them (kept side
/// by side).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SweepResult {
    pub moved: Vec<String>,
    pub conflicts: Vec<String>,
}

/// `placement_status`: what copying a note into a folder would do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    New,
    Identical,
    Conflict,
}

/// The seams of `memory_sync` that tests patch (GC-19). `Default` is production.
pub struct SyncDeps<'a> {
    /// `memory_sync._git` (and the `_run` patches): every git call.
    pub git: &'a dyn Git,
    /// `memory_sync.repo_root`.
    pub repo_root: fn(&Path, &Env, &dyn Git) -> Option<PathBuf>,
    /// `memory_sync.sweep_user_notes`, as `_sync_repo` calls it (an error is an exception).
    pub sweep_user_notes:
        fn(&Path, &Path, Option<Date>, &SyncDeps<'_>) -> Result<SweepResult, PyExc>,
    /// `memory_sync.regenerate_index`.
    pub regenerate_index: fn(&Path, Budget, &[&str]) -> Result<bool, PyExc>,
    /// `memory_sync.privacy_findings`.
    pub privacy_findings: fn(&str, Option<&str>, Option<&str>, &PrivacyDeps) -> Vec<Finding>,
    /// The privacy scan's own seams (its host name and `$HOME` fallback).
    pub privacy: PrivacyDeps,
    /// `memory_sync._read_regular`, as the sweep calls it.
    pub read_regular: fn(&Path) -> Result<Vec<u8>, PyExc>,
    /// `datetime.date.today()`.
    pub today: fn() -> Date,
}

static REAL_GIT: RealGit = RealGit;

fn read_regular_default(p: &Path) -> Result<Vec<u8>, PyExc> {
    fsx::read_regular(p, None)
}

fn sweep_default(
    src: &Path,
    personal: &Path,
    today: Option<Date>,
    deps: &SyncDeps<'_>,
) -> Result<SweepResult, PyExc> {
    try_sweep_user_notes(src, personal, today, deps)
}

impl Default for SyncDeps<'static> {
    fn default() -> SyncDeps<'static> {
        SyncDeps {
            git: &REAL_GIT,
            repo_root,
            sweep_user_notes: sweep_default,
            regenerate_index,
            privacy_findings: privacy::privacy_findings,
            privacy: PrivacyDeps::from_env(&Env::default()),
            read_regular: read_regular_default,
            today: clock::today_local,
        }
    }
}

impl SyncDeps<'static> {
    /// Production deps whose privacy scan falls back to `env`'s `HOME`, as Python's falls back
    /// to `os.environ["HOME"]`.
    pub fn from_env(env: &Env) -> SyncDeps<'static> {
        SyncDeps {
            privacy: PrivacyDeps::from_env(env),
            ..SyncDeps::default()
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------------------------

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// `str(exc) or type(exc).__name__`.
fn exc_text(e: &PyExc) -> String {
    let t = e.py_str();
    if t.is_empty() {
        e.type_name().to_string()
    } else {
        t
    }
}

/// `exc.strerror`: the errno text of an `OSError`, else `str(exc)`.
pub(crate) fn strerror(e: &PyExc) -> String {
    match e {
        PyExc::OSError { errno, .. } => anthology_pycompat::errno_tables::strerror(*errno),
        other => other.py_str(),
    }
}

fn os_err(e: io::Error, p: &Path) -> PyExc {
    match PyExc::from(e) {
        PyExc::OSError { errno, .. } => PyExc::OSError {
            errno,
            filename: Some(s(p)),
        },
        other => other,
    }
}

/// A panic's message, as `str(exc)`; `panic` when it has none.
fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|m| (*m).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| "panic".into())
}

fn cut120(t: &str) -> String {
    pystr::char_slice(t, 0, 120).to_string()
}

#[derive(PartialEq, Eq)]
enum Kind {
    Absent,
    File,
    Other,
}

/// `_lstat_kind`.
fn lstat_kind(p: &Path) -> Kind {
    match fs::symlink_metadata(p) {
        Ok(m) if m.file_type().is_file() => Kind::File,
        Ok(_) => Kind::Other,
        Err(e) if e.kind() == io::ErrorKind::NotFound => Kind::Absent,
        Err(_) => Kind::Other,
    }
}

fn is_symlink(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink())
}

fn lexists(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok()
}

/// `Path.read_bytes()` (it follows links), except that a FIFO, device or folder is an error
/// instead of a blocking read.
fn read_follow(p: &Path) -> io::Result<Vec<u8>> {
    let m = fs::metadata(p)?;
    if !m.is_file() {
        return Err(io::Error::from_raw_os_error(if m.is_dir() {
            21
        } else {
            22
        }));
    }
    fs::read(p)
}

/// Python's `str.splitlines()`.
fn splitlines(t: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = t.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let br = matches!(
            c,
            '\n' | '\r'
                | '\x0b'
                | '\x0c'
                | '\x1c'
                | '\x1d'
                | '\x1e'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        );
        if br {
            out.push(&t[start..i]);
            let mut end = i + c.len_utf8();
            if c == '\r'
                && let Some((_, '\n')) = it.peek()
            {
                it.next();
                end += 1;
            }
            start = end;
        }
    }
    if start < t.len() {
        out.push(&t[start..]);
    }
    out
}

/// `_git(...)` and its return code, or `None`.
fn git_code(deps: &SyncDeps<'_>, env: &Env, root: &Path, args: &[&str]) -> Option<i32> {
    deps.git.run(env, root, args).map(|o| o.code)
}

/// `_git_out`.
fn git_out(deps: &SyncDeps<'_>, env: &Env, root: &Path, args: &[&str]) -> Option<String> {
    let o = deps.git.run(env, root, args)?;
    if o.code != 0 {
        return None;
    }
    let t = pystr::strip_ws(&String::from_utf8_lossy(&o.stdout)).to_string();
    (!t.is_empty()).then_some(t)
}

// ---------------------------------------------------------------------------------------------
// Repairs
// ---------------------------------------------------------------------------------------------

/// `_load_settings`: `Some(object)` (empty when the file is missing), or `None` when it is
/// unusable (invalid JSON, not an object, a symlink, not a regular file).
pub fn load_settings(path: &Path) -> Option<Map> {
    if lstat_kind(path) == Kind::Absent {
        return Some(Map::new());
    }
    let raw = fsx::read_regular(path, None).ok()?;
    let text = String::from_utf8(raw).ok()?;
    json::loads(&text).ok()?.into_object()
}

/// `settings_tracked`: whether `.claude/settings.local.json` is tracked by git.
pub fn settings_tracked(root: &Path, env: &Env, deps: &SyncDeps<'_>) -> bool {
    git_code(
        deps,
        env,
        root,
        &[
            "ls-files",
            "--error-unmatch",
            "--",
            ".claude/settings.local.json",
        ],
    ) == Some(0)
}

/// `ensure_memory_dir`: point `autoMemoryDirectory` in `settings_local` at this clone's
/// `.claude/memory`. Returns `(settings, changed)`; an unusable file, or a `.claude` /
/// `.claude/memory` that is a symlink or not a folder, is left untouched (`({}, false)`).
pub fn ensure_memory_dir(root: &Path, settings_local: &Path) -> Result<(Map, bool), PyExc> {
    let claude = anthology_pycompat::path::realpath(root).join(".claude");
    let mem = claude.join("memory");
    if is_symlink(&claude) || is_symlink(&mem) || (lexists(&mem) && !mem.is_dir()) {
        return Ok((Map::new(), false));
    }
    let Some(old) = load_settings(settings_local) else {
        return Ok((Map::new(), false));
    };
    fs::create_dir_all(&mem).map_err(|e| os_err(e, &mem))?;
    let target = s(&mem);
    if old.get("autoMemoryDirectory").and_then(Value::as_str) == Some(target.as_str()) {
        return Ok((old, false));
    }
    let mut new = old;
    new.insert("autoMemoryDirectory".into(), Value::Str(target));
    let new = Value::Object(new);
    let text = json::dumps(&new, &json::INDENT2) + "\n";
    fsx::atomic_write(settings_local, text.as_bytes(), 0o644, ".tmp-", ".part")?;
    Ok((new.into_object().unwrap_or_default(), true))
}

/// `ensure_ignored`: make `.claude/settings.local.json` git-ignored through the repo's own
/// exclude file (`git rev-parse --git-path info/exclude`). Idempotent; a tracked file is left
/// alone. Returns whether it changed anything; a symlinked or non-regular exclude file is an
/// `UnsafeTarget` error.
pub fn ensure_ignored(root: &Path, env: &Env, deps: &SyncDeps<'_>) -> Result<bool, PyExc> {
    if settings_tracked(root, env, deps) {
        return Ok(false);
    }
    let check = git_code(
        deps,
        env,
        root,
        &["check-ignore", "-q", "--", ".claude/settings.local.json"],
    );
    if check != Some(1) {
        return Ok(false);
    }
    let Some(out) = git_out(
        deps,
        env,
        root,
        &["rev-parse", "--git-path", "info/exclude"],
    ) else {
        return Ok(false);
    };
    let mut exclude = PathBuf::from(&out);
    if !exclude.is_absolute() {
        exclude = root.join(exclude);
    }
    let text = String::from_utf8_lossy(&fsx::read_regular(&exclude, None)?).into_owned();
    if splitlines(&text)
        .iter()
        .any(|l| pystr::strip_ws(l) == EXCLUDE_LINE)
    {
        return Ok(false);
    }
    fsx::append_line(&exclude, EXCLUDE_LINE)?;
    Ok(true)
}

// ---------------------------------------------------------------------------------------------
// Migration
// ---------------------------------------------------------------------------------------------

fn py_type(v: &Value) -> &'static str {
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

/// `_load_ledger`: the hashes a JSON ledger lists as done. Exactly Python's
/// `{h for h in data.get("done", []) if isinstance(h, str)}`, including iterating a string or
/// an object, and the `TypeError` of a `done` that cannot be iterated.
fn load_ledger(path: &Path) -> Result<BTreeSet<String>, PyExc> {
    let Ok(raw) = read_follow(path) else {
        return Ok(BTreeSet::new());
    };
    let Some(data) = String::from_utf8(raw)
        .ok()
        .and_then(|t| json::loads(&t).ok())
    else {
        return Ok(BTreeSet::new());
    };
    let Value::Object(map) = &data else {
        return Ok(BTreeSet::new()); // AttributeError: no .get
    };
    let mut out = BTreeSet::new();
    match map.get("done") {
        None => {}
        Some(Value::Array(items)) => {
            out.extend(items.iter().filter_map(Value::as_str).map(String::from));
        }
        Some(Value::Str(t)) => out.extend(t.chars().map(String::from)),
        Some(Value::Object(m)) => out.extend(m.iter().map(|(k, _)| k.to_string())),
        Some(other) => {
            return Err(PyExc::Other(
                "TypeError",
                format!("'{}' object is not iterable", py_type(other)),
            ));
        }
    }
    Ok(out)
}

fn is_plain(p: &Path) -> bool {
    p.is_file() && !is_symlink(p)
}

/// `_create_exclusive`: create `path` with `data`, never replacing anything; `false` when the
/// name is taken.
fn create_exclusive(path: &Path, data: &[u8]) -> Result<bool, PyExc> {
    match fsx::create_excl(path, data, 0o666, false) {
        Ok(()) => Ok(true),
        Err(PyExc::OSError { errno: 17, .. }) => Ok(false),
        Err(e) => Err(e),
    }
}

fn stem_of(name: &str) -> &str {
    match name.char_indices().rev().nth(2) {
        Some((i, _)) => &name[..i],
        None => "",
    }
}

/// `_same_copy`: the plain file in `dest_dir` holding exactly `data` (`name`, or an earlier
/// `<stem>.migrated-*.md` copy).
fn same_copy(dest_dir: &Path, name: &str, data: &[u8]) -> Option<PathBuf> {
    let stem = stem_of(name);
    let mut names: Vec<String> = fs::read_dir(dest_dir)
        .ok()?
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    let prefix = format!("{stem}.migrated-");
    let candidates = std::iter::once(name.to_string()).chain(
        names
            .into_iter()
            .filter(|c| c.starts_with(&prefix) && c.ends_with(".md")),
    );
    for c in candidates {
        let p = dest_dir.join(&c);
        if is_plain(&p) && read_follow(&p).is_ok_and(|b| b == data) {
            return Some(p);
        }
    }
    None
}

/// `placement_status`: `New` when nothing has the name, `Identical` when the note (or an
/// earlier side-by-side copy) already holds these bytes, else `Conflict`.
pub fn placement_status(dest_dir: &Path, name: &str, data: &[u8]) -> Placement {
    if !lexists(&dest_dir.join(name)) {
        return Placement::New;
    }
    if same_copy(dest_dir, name, data).is_some() {
        Placement::Identical
    } else {
        Placement::Conflict
    }
}

/// `side_by_side_name`: the first free `<stem>.migrated-<date>[-N].md` in `dest_dir`.
pub fn side_by_side_name(dest_dir: &Path, name: &str, date: &Date) -> String {
    side_by_side(dest_dir, name, &date.iso())
}

fn side_by_side(dest_dir: &Path, name: &str, date: &str) -> String {
    let stem = stem_of(name);
    let mut n = 1u64;
    loop {
        let tail = if n == 1 {
            String::new()
        } else {
            format!("-{n}")
        };
        let candidate = format!("{stem}.migrated-{date}{tail}.md");
        if !lexists(&dest_dir.join(&candidate)) {
            return candidate;
        }
        n += 1;
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Placed {
    Created,
    Identical,
    Conflict,
}

/// `_place`: copy `data` into `dest_dir` as `name`, never overwriting.
fn place(dest_dir: &Path, name: &str, data: &[u8], date: &str) -> Result<(Placed, PathBuf), PyExc> {
    let target = dest_dir.join(name);
    if create_exclusive(&target, data)? {
        return Ok((Placed::Created, target));
    }
    if let Some(same) = same_copy(dest_dir, name, data) {
        return Ok((Placed::Identical, same));
    }
    loop {
        let p = dest_dir.join(side_by_side(dest_dir, name, date));
        if create_exclusive(&p, data)? {
            return Ok((Placed::Conflict, p));
        }
    }
}

/// `_ensure_private_dir`: create a missing personal-memory folder as 0700.
fn ensure_private_dir(path: &Path) -> Result<(), PyExc> {
    if !lexists(path) {
        fs::create_dir_all(path).map_err(|e| os_err(e, path))?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|e| os_err(e, path))?;
    }
    Ok(())
}

fn try_legacy_entries(old_dir: &Path) -> Result<Vec<(String, Vec<u8>)>, PyExc> {
    let mut out = Vec::new();
    if !old_dir.is_dir() {
        return Ok(out);
    }
    let mut entries: Vec<fs::DirEntry> = fs::read_dir(old_dir)
        .map_err(|e| os_err(e, old_dir))?
        .flatten()
        .collect();
    entries.sort_by_key(fs::DirEntry::file_name);
    for e in entries {
        // A name that is not UTF-8 cannot be copied under the same name: skipped.
        let Ok(name) = e.file_name().into_string() else {
            continue;
        };
        if !name.ends_with(".md") || name == "MEMORY.md" {
            continue;
        }
        let Ok(ft) = e.file_type() else { continue };
        if !ft.is_file() {
            continue;
        }
        if let Ok(data) = read_follow(&e.path()) {
            out.push((name, data));
        }
    }
    Ok(out)
}

/// `legacy_entries`: `(file name, bytes)` of the flat `*.md` notes (not `MEMORY.md`) of an old
/// path-keyed memory folder; symlinks and non-regular files are skipped.
pub fn legacy_entries(old_dir: &Path) -> Vec<(String, Vec<u8>)> {
    try_legacy_entries(old_dir).unwrap_or_default()
}

/// `promotion_block` with its findings (an MS2 addition beyond plan §3.6, for setup's
/// preview): `(findings, reason)`, the reason `None` when the note may go into repo memory.
pub fn promotion_check(
    info: &NoteInfo,
    data: &[u8],
    home: Option<&str>,
    hostname: Option<&str>,
    deps: &SyncDeps<'_>,
) -> (Vec<Finding>, Option<String>) {
    if is_user_note(info) {
        return (Vec::new(), Some(USER_REASON.to_string()));
    }
    let text = String::from_utf8_lossy(data);
    let findings = (deps.privacy_findings)(&text, home, hostname, &deps.privacy);
    if findings.is_empty() {
        return (findings, None);
    }
    let reason = format!(
        "the privacy scan found {}; kept in personal memory",
        privacy::describe(&findings)
    );
    (findings, Some(reason))
}

/// `promotion_block`: why a note may not go into repo memory, or `None`.
///
/// Pass `home` (setup passes `$HOME`): this form has no `Env`, so `None` means no home-path
/// check, where Python falls back to `os.environ["HOME"]`. [`promotion_check`] with
/// [`SyncDeps::from_env`] has the fallback.
pub fn promotion_block(
    info: &NoteInfo,
    data: &[u8],
    home: Option<&str>,
    hostname: Option<&str>,
) -> Option<String> {
    promotion_check(info, data, home, hostname, &SyncDeps::default()).1
}

/// `migrate`, exactly: an error is Python's exception (setup reports it). An MS2 addition
/// beyond plan §3.6; [`migrate`] is the published form.
#[allow(
    clippy::too_many_arguments,
    reason = "Python's keyword arguments, as published"
)]
pub fn try_migrate(
    old_dir: &Path,
    repo_mem: Option<&Path>,
    personal_mem: &Path,
    ledger: Option<&Path>,
    today: Option<Date>,
    promote: &[String],
    home: Option<&str>,
    deps: &SyncDeps<'_>,
) -> Result<MigrateResult, PyExc> {
    let mut result = MigrateResult::default();
    let entries = try_legacy_entries(old_dir)?;
    if entries.is_empty() {
        return Ok(result);
    }
    let date = today.unwrap_or_else(deps.today).iso();
    let done = match ledger {
        Some(l) => load_ledger(l)?,
        None => BTreeSet::new(),
    };
    let mut handled = done.clone();
    for (name, data) in entries {
        let sha: String = Sha256::digest(&data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let mut to_repo = false;
        if promote.contains(&name) {
            let (findings, mut reason) =
                promotion_check(&note_info(&name, &data), &data, home, None, deps);
            if reason.is_none() {
                let unsafe_repo = match repo_mem {
                    None => true,
                    Some(r) => {
                        unsafe_dir(r).is_some()
                            || unsafe_dir(r.parent().unwrap_or(Path::new(""))).is_some()
                    }
                };
                if unsafe_repo {
                    reason = Some(
                        "repo memory (or .claude) is a symlink or not a folder; kept in personal memory"
                            .into(),
                    );
                }
            }
            match reason {
                None => to_repo = true,
                Some(r) => {
                    result.blocked.push((name.clone(), r));
                    result.blocked_findings.push(findings);
                }
            }
        }
        if !to_repo && done.contains(&sha) {
            result.skipped.push(name);
            continue;
        }
        let dest = match (to_repo, repo_mem) {
            (true, Some(r)) => r,
            _ => personal_mem,
        };
        if unsafe_dir(dest).is_some() {
            continue; // never write through a symlinked or non-folder destination
        }
        if to_repo {
            fs::create_dir_all(dest).map_err(|e| os_err(e, dest))?;
        } else {
            ensure_private_dir(dest)?;
        }
        let (status, _) = place(dest, &name, &data, &date)?;
        match status {
            Placed::Created => result.copied.push(name.clone()),
            Placed::Identical => result.skipped.push(name.clone()),
            Placed::Conflict => result.side_by_side.push(name.clone()),
        }
        if to_repo {
            result.promoted.push(name);
        }
        handled.insert(sha);
    }
    if let Some(l) = ledger {
        save_ledger(l, &done, handled)?;
    }
    Ok(result)
}

/// Write the ledger when `handled` holds a hash `done` did not.
fn save_ledger(
    path: &Path,
    done: &BTreeSet<String>,
    handled: BTreeSet<String>,
) -> Result<(), PyExc> {
    if &handled == done {
        return Ok(());
    }
    let mut m = Map::new();
    m.insert("version".into(), Value::Int(Int::Small(1)));
    m.insert(
        "done".into(),
        Value::Array(handled.into_iter().map(Value::Str).collect()),
    );
    let text = json::dumps(&Value::Object(m), &json::INDENT2) + "\n";
    fsx::atomic_write(path, text.as_bytes(), 0o644, ".tmp-", ".part")
}

/// `migrate`: copy the notes of an old path-keyed memory folder into personal memory; only a
/// note named in `promote` goes to `repo_mem`, and only if it is not `type: user` and the
/// privacy scan finds nothing. Never overwrites (a `<stem>.migrated-<date>.md` copy instead).
/// On an error (Python's exception) the result is empty; callers that must report it use
/// [`try_migrate`].
#[allow(
    clippy::too_many_arguments,
    reason = "Python's keyword arguments, as published"
)]
pub fn migrate(
    old_dir: &Path,
    repo_mem: Option<&Path>,
    personal_mem: &Path,
    ledger: Option<&Path>,
    today: Option<Date>,
    promote: &[String],
    home: Option<&str>,
    deps: &SyncDeps,
) -> MigrateResult {
    try_migrate(
        old_dir,
        repo_mem,
        personal_mem,
        ledger,
        today,
        promote,
        home,
        deps,
    )
    .unwrap_or_default()
}

// ---------------------------------------------------------------------------------------------
// Moving the notes of Claude Code's own auto memory, by type
// ---------------------------------------------------------------------------------------------

/// The reason for a note without one of the four note types.
pub const NO_TYPE_REASON: &str =
    "the note has no type (project, feedback, reference or user), so it is not moved";
/// The reason when repo memory cannot take a note.
pub const REPO_UNSAFE_REASON: &str = "repo memory (or .claude) is a symlink or not a folder";

/// Where [`plan_auto_memory_note`] sends a note.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Destination {
    /// The repo's `.claude/memory/`: `project`, `feedback` and `reference` notes.
    Repo,
    /// Personal memory: `user` notes.
    Personal,
    /// Nowhere: the privacy scan refused it, or it has no type.
    Nowhere,
}

impl Destination {
    pub fn label(self) -> &'static str {
        match self {
            Destination::Repo => "repo",
            Destination::Personal => "personal",
            Destination::Nowhere => "none",
        }
    }
}

/// What moving one old note would do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutoMemoryPlan {
    pub destination: Destination,
    /// The file name it gets there, `<type>-<slug>.md`; `None` when it goes nowhere.
    pub target: Option<String>,
    pub findings: Vec<Finding>,
    /// Why it goes nowhere.
    pub reason: Option<String>,
}

/// `[a-z0-9]+(-[a-z0-9]+)*` from any text: runs of other characters become one `-`.
fn slugify(text: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(c.to_ascii_lowercase());
        } else {
            pending_dash = true;
        }
    }
    out
}

/// The slug of `<type>-<slug>.md`: the file's stem (else the note's name) without a leading
/// `<type>-`, so `feedback_testing.md` becomes `feedback-testing.md`.
fn auto_memory_slug(kind: &str, file: &str, info: &NoteInfo) -> String {
    let own = |text: &str| {
        let slug = slugify(text);
        match slug.strip_prefix(&format!("{kind}-")) {
            Some(rest) => rest.to_string(),
            None if slug == kind => String::new(),
            None => slug,
        }
    };
    let mut slug = own(file.strip_suffix(".md").unwrap_or(file));
    if slug.is_empty() {
        slug = own(&info.name);
    }
    if slug.is_empty() {
        slug = "note".to_string();
    }
    let slug: String = slug.chars().take(crate::save::MAX_SLUG).collect();
    slug.trim_end_matches('-').to_string()
}

/// `<type>-<slug>.md`.
fn auto_memory_target(kind: &str, file: &str, info: &NoteInfo) -> String {
    format!("{kind}-{}.md", auto_memory_slug(kind, file, info))
}

/// Where an old auto-memory note goes, and under what name: `project`, `feedback` and
/// `reference` notes into the repo's memory, `user` notes into personal memory. A repo-bound
/// note goes through the privacy scan of the whole file (`promotion_check`) and then every check
/// `anthology-mem save` runs on a note's text (`save::check_note_text`: the secret and env-value
/// checks, control and hidden characters, the size limits and the scanner-crash guard); one it
/// refuses, or one without a type, goes nowhere. A `user` note goes to personal memory, which is
/// private and not in git, so it needs no repo check. `repo_ok` is whether the repo's memory
/// folder can take the note.
pub fn plan_auto_memory_note(
    file: &str,
    data: &[u8],
    repo_ok: bool,
    home: Option<&str>,
    env: &Env,
    deps: &SyncDeps<'_>,
) -> AutoMemoryPlan {
    let info = note_info(file, data);
    let kind = normalize_type(info.kind.as_deref());
    let nowhere = |findings, reason: &str| AutoMemoryPlan {
        destination: Destination::Nowhere,
        target: None,
        findings,
        reason: Some(reason.to_string()),
    };
    match kind.as_str() {
        "user" => AutoMemoryPlan {
            destination: Destination::Personal,
            target: Some(auto_memory_target("user", file, &info)),
            findings: Vec::new(),
            reason: None,
        },
        "project" | "feedback" | "reference" => {
            // A scanner bug is a clean refusal, never a crash or a move.
            let Ok((findings, reason)) = catch_unwind(AssertUnwindSafe(|| {
                promotion_check(&info, data, home, None, deps)
            })) else {
                return nowhere(
                    Vec::new(),
                    "the privacy scan failed (panic); nothing was moved",
                );
            };
            let save_deps = crate::save::SaveDeps {
                privacy_findings: deps.privacy_findings,
                ..crate::save::SaveDeps::default()
            };
            let save_check = crate::save::check_note_text(
                &auto_memory_slug(&kind, file, &info),
                &info.name,
                &info.description,
                &info.body,
                env,
                &save_deps,
            );
            match reason {
                Some(r) => nowhere(findings, &r),
                None if !repo_ok => nowhere(findings, REPO_UNSAFE_REASON),
                None if save_check.is_err() => {
                    let why = save_check.err().map(|e| e.to_string()).unwrap_or_default();
                    nowhere(
                        findings,
                        &format!("it fails the checks anthology-mem save runs: {why}"),
                    )
                }
                None => AutoMemoryPlan {
                    destination: Destination::Repo,
                    target: Some(auto_memory_target(&kind, file, &info)),
                    findings,
                    reason: None,
                },
            }
        }
        _ => nowhere(Vec::new(), NO_TYPE_REASON),
    }
}

/// What [`try_move_auto_memory`] did: the names the notes were placed under.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MoveResult {
    pub repo: Vec<String>,
    pub personal: Vec<String>,
    pub skipped: Vec<String>,
    /// `(old file, reason)`, with the findings of each in `blocked_findings`.
    pub blocked: Vec<(String, String)>,
    pub blocked_findings: Vec<Vec<Finding>>,
}

/// Move the notes of one old auto-memory folder (`legacy_dirs`) by type: see
/// [`plan_auto_memory_note`]. The old folder is only read. A note never overwrites one that
/// exists (a `<stem>.migrated-<date>.md` copy instead), and one already placed is skipped.
/// Personal notes already in the ledger are skipped; a repo note is placed whatever the ledger
/// says (the user chose to move it), and every note placed is added to the ledger, so session
/// start does not copy it into personal memory.
#[allow(
    clippy::too_many_arguments,
    reason = "the folders, ledger, date and the checks' inputs are all separate"
)]
pub fn try_move_auto_memory(
    old_dir: &Path,
    repo_mem: Option<&Path>,
    personal_mem: &Path,
    ledger: Option<&Path>,
    today: Option<Date>,
    home: Option<&str>,
    env: &Env,
    deps: &SyncDeps<'_>,
) -> Result<MoveResult, PyExc> {
    let mut result = MoveResult::default();
    let entries = try_legacy_entries(old_dir)?;
    if entries.is_empty() {
        return Ok(result);
    }
    let date = today.unwrap_or_else(deps.today).iso();
    let done = match ledger {
        Some(l) => load_ledger(l)?,
        None => BTreeSet::new(),
    };
    let mut handled = done.clone();
    let repo_ok = repo_mem.is_some_and(|r| {
        unsafe_dir(r).is_none() && unsafe_dir(r.parent().unwrap_or(Path::new(""))).is_none()
    });
    for (name, data) in entries {
        let plan = plan_auto_memory_note(&name, &data, repo_ok, home, env, deps);
        let (Some(target), Some(dest)) = (
            plan.target.as_deref(),
            match plan.destination {
                Destination::Repo => repo_mem,
                _ => Some(personal_mem),
            },
        ) else {
            result.blocked.push((name, plan.reason.unwrap_or_default()));
            result.blocked_findings.push(plan.findings);
            continue;
        };
        let sha: String = Sha256::digest(&data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let to_repo = plan.destination == Destination::Repo;
        if !to_repo && done.contains(&sha) {
            result.skipped.push(target.to_string());
            continue;
        }
        if unsafe_dir(dest).is_some() {
            // never write through a symlinked or non-folder destination; say so
            let which = if to_repo {
                "repo memory"
            } else {
                "personal memory"
            };
            result
                .blocked
                .push((name, format!("{which} is a symlink or not a folder")));
            result.blocked_findings.push(Vec::new());
            continue;
        }
        if to_repo {
            fs::create_dir_all(dest).map_err(|e| os_err(e, dest))?;
        } else {
            ensure_private_dir(dest)?;
        }
        let (status, placed) = place(dest, target, &data, &date)?;
        let placed_name = placed
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| target.to_string());
        match status {
            Placed::Identical => result.skipped.push(placed_name),
            _ if to_repo => result.repo.push(placed_name),
            _ => result.personal.push(placed_name),
        }
        handled.insert(sha);
    }
    if let Some(l) = ledger {
        save_ledger(l, &done, handled)?;
    }
    Ok(result)
}

// ---------------------------------------------------------------------------------------------
// `user` notes never stay in the repo
// ---------------------------------------------------------------------------------------------

/// `sweep_user_notes`, exactly: an error is Python's exception (`os.scandir` failing). An MS2
/// addition beyond plan §3.6.
pub fn try_sweep_user_notes(
    src_dir: &Path,
    personal_mem: &Path,
    today: Option<Date>,
    deps: &SyncDeps<'_>,
) -> Result<SweepResult, PyExc> {
    let mut result = SweepResult::default();
    if is_symlink(src_dir) || !src_dir.is_dir() || unsafe_dir(personal_mem).is_some() {
        return Ok(result);
    }
    let date = today.unwrap_or_else(deps.today).iso();
    let mut entries: Vec<fs::DirEntry> = fs::read_dir(src_dir)
        .map_err(|e| os_err(e, src_dir))?
        .flatten()
        .collect();
    entries.sort_by_key(fs::DirEntry::file_name);
    for e in entries {
        let Ok(name) = e.file_name().into_string() else {
            continue;
        };
        if !name.ends_with(".md") || name == "MEMORY.md" {
            continue;
        }
        let path = e.path();
        if !fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_file()) {
            continue;
        }
        let Ok(data) = (deps.read_regular)(&path) else {
            continue;
        };
        if !is_user_note(&note_info(&name, &data)) {
            continue;
        }
        let moved = (|| -> Result<Placed, PyExc> {
            ensure_private_dir(personal_mem)?;
            let (status, copy) = place(personal_mem, &name, &data, &date)?;
            if (deps.read_regular)(&copy)? != data || (deps.read_regular)(&path)? != data {
                return Err(PyExc::Other("Unverified", String::new()));
            }
            fs::remove_file(&path).map_err(|e| os_err(e, &path))?;
            Ok(status)
        })();
        let Ok(status) = moved else {
            continue;
        };
        if status == Placed::Conflict {
            result.conflicts.push(name.clone());
        }
        result.moved.push(name);
    }
    Ok(result)
}

/// `sweep_user_notes`: move every `type: user` note out of `src_dir` (repo or agent memory)
/// into `personal_mem`, never overwriting; the repo file is deleted only after the copy is
/// verified. Symlinks and non-regular files are never followed or touched.
pub fn sweep_user_notes(
    src_dir: &Path,
    personal_mem: &Path,
    today: Option<Date>,
    deps: &SyncDeps,
) -> SweepResult {
    try_sweep_user_notes(src_dir, personal_mem, today, deps).unwrap_or_default()
}

// ---------------------------------------------------------------------------------------------
// The SessionStart entry point
// ---------------------------------------------------------------------------------------------

/// Whether the user turned auto memory off. `approved`: the repo is approved for Anthology
/// memory, which is where a committed `autoMemoryEnabled: false` is Anthology's own switch.
fn auto_memory_off(root: Option<&Path>, config: &Path, env: &Env, approved: bool) -> bool {
    if let Some(v) = env.get_os("CLAUDE_CODE_DISABLE_AUTO_MEMORY") {
        let v = pystr::lower(pystr::strip_ws(&v.to_string_lossy()));
        if !["", "0", "false", "no"].contains(&v.as_str()) {
            return true;
        }
    }
    let mut paths = Vec::new();
    if let Some(r) = root {
        paths.push((r.join(".claude").join("settings.local.json"), false));
        paths.push((r.join(".claude").join("settings.json"), true));
    }
    paths.push((config.join("settings.json"), false));
    for (p, committed) in paths {
        if let Some(data) = load_settings(&p)
            && let Some(v) = data.get("autoMemoryEnabled")
        {
            // In an approved repo, `false` in the committed settings.json is what
            // `/anthology:setup` writes to switch Claude Code's own auto memory off in favour of
            // this memory; it is not an opt-out of Anthology memory. In any other repo it is the
            // team's opt-out, as before. A user's, the local or the environment's opt-out always is.
            if approved && committed && matches!(v, Value::Bool(false)) {
                continue;
            }
            return matches!(v, Value::Bool(false));
        }
    }
    false
}

/// Whether the repo's committed settings.json switches Claude Code's own auto memory off (as
/// `/anthology:setup` writes it) and settings.local.json does not switch it back on. Claude Code
/// then does not load the repo's memory index, so the SessionStart hook injects it every session.
fn auto_memory_off_by_repo(claude: &Path) -> bool {
    let flag = |name: &str| {
        load_settings(&claude.join(name)).and_then(|d| d.get("autoMemoryEnabled").cloned())
    };
    flag("settings.local.json").is_none()
        && matches!(flag("settings.json"), Some(Value::Bool(false)))
}

fn section(title: &str, directory: &Path, text: &str) -> String {
    let nl = if text.ends_with('\n') { "" } else { "\n" };
    format!("## {title} ({})\n\n{text}{nl}", s(directory))
}

/// `_context_index`: the index of `directory` within the context budget, with "N more"
/// pointers to `anthology-mem search`. They name no file: the on-disk `MEMORY.md` is capped too,
/// so it does not hold every note either.
fn context_index(directory: &Path, exclude: &[&str]) -> Result<String, PyExc> {
    if let Some(text) = render_index(directory, CONTEXT_SECTION_BUDGET, "", exclude)? {
        return Ok(if pystr::strip_ws(&text) == GENERATED {
            String::new()
        } else {
            text
        });
    }
    // Rust stricter (D-8, controller ruling): a hand-made index is read only when it is a
    // regular file, never through a symlink (which could pull any file into the session
    // context). Anything else is unreadable, as Python's `except OSError`.
    let index = directory.join("MEMORY.md");
    if lstat_kind(&index) != Kind::File {
        return Ok(String::new());
    }
    let Ok(raw) = fsx::read_regular(&index, None) else {
        return Ok(String::new());
    };
    let raw = String::from_utf8_lossy(&raw);
    let lines = splitlines(&raw);
    let head = lines[..lines.len().min(CONTEXT_SECTION_BUDGET.lines)].join("\n");
    Ok(format!(
        "{}\n",
        pystr::char_slice(&head, 0, CONTEXT_SECTION_BUDGET.bytes)
    ))
}

/// `_personal_unsafe`: true (and one message) when personal memory is a symlink or not a folder.
fn personal_unsafe(personal: &Path, messages: &mut Vec<String>) -> bool {
    if unsafe_dir(personal).is_none() {
        return false;
    }
    let msg = format!(
        "anthology memory: {} is a symlink or not a folder, so personal memory was not loaded, \
         user notes were not moved out of the repo and old notes were not copied; make it a real \
         folder to turn this back on",
        s(personal)
    );
    if !messages.contains(&msg) {
        messages.push(msg);
    }
    true
}

fn agent_dirs(agents: &Path) -> Result<Vec<PathBuf>, PyExc> {
    if !agents.is_dir() || is_symlink(agents) {
        return Ok(Vec::new());
    }
    let mut dirs: Vec<PathBuf> = fs::read_dir(agents)
        .map_err(|e| os_err(e, agents))?
        .flatten()
        .map(|e| e.path())
        .filter(|d| d.is_dir() && !is_symlink(d))
        .collect();
    dirs.sort();
    Ok(dirs)
}

/// `_sync_repo`: the approved-repo flow; returns the same-session cover section.
fn sync_repo(
    root: &Path,
    home: &Path,
    config: &Path,
    env: &Env,
    messages: &mut Vec<String>,
    deps: &SyncDeps<'_>,
) -> Result<Option<String>, PyExc> {
    let claude = root.join(".claude");
    let repo_mem = claude.join("memory");
    if is_symlink(&claude) {
        messages.push("anthology memory: .claude is a symlink; repo memory skipped".into());
        return Ok(None);
    }
    if is_symlink(&repo_mem) || (lexists(&repo_mem) && !repo_mem.is_dir()) {
        messages.push(
            "anthology memory: .claude/memory is a symlink or not a folder; repo memory skipped"
                .into(),
        );
        return Ok(None);
    }
    let personal = home.join("personal-memory");
    let personal_ok = !personal_unsafe(&personal, messages);
    let (mut copied, mut conflicts) = (0usize, 0usize);
    if personal_ok {
        let ledger = home.join("migrated.json");
        for old in legacy_dirs(root, config, env, deps.git) {
            // Never into the repo on its own: setup moves chosen notes there (B3).
            let res = try_migrate(&old, None, &personal, Some(&ledger), None, &[], None, deps)?;
            copied += res.copied.len();
            conflicts += res.side_by_side.len();
        }
    }
    if copied > 0 {
        messages.push(format!(
            "anthology memory: copied {copied} note(s) from the old per-path memory folder into \
             personal memory ({}); /anthology:setup can move chosen ones into this repo's memory",
            s(&personal)
        ));
    }
    if conflicts > 0 {
        messages.push(format!(
            "anthology memory: {conflicts} migrated note(s) differ from existing ones; kept side \
             by side as *.migrated-*.md"
        ));
    }
    let agents = agent_dirs(&claude.join("agent-memory"))?;
    let (mut moved, mut side) = (0usize, 0usize);
    if personal_ok {
        for d in std::iter::once(&repo_mem).chain(&agents) {
            let res = (deps.sweep_user_notes)(d, &personal, None, deps)?;
            moved += res.moved.len();
            side += res.conflicts.len();
            let index = d.join("MEMORY.md");
            if !res.moved.is_empty()
                && try_list_notes(d)?.is_empty()
                && lstat_kind(&index) == Kind::File
            {
                let text = format!("{GENERATED}\n");
                fsx::atomic_write(&index, text.as_bytes(), 0o644, ".tmp-", ".part")?;
            }
        }
    }
    if moved > 0 {
        let kept = if side > 0 {
            format!("; {side} kept side by side as *.migrated-*.md")
        } else {
            String::new()
        };
        messages.push(format!(
            "anthology memory: moved {moved} user note(s) out of the repo into personal memory \
             ({}){kept}; if they were committed, commit their removal",
            s(&personal)
        ));
    }
    let settings = claude.join("settings.local.json");
    let mut changed = false;
    if settings_tracked(root, env, deps) {
        messages.push(
            "anthology memory: .claude/settings.local.json is tracked by git, so anthology did \
             not write this clone's memory path into it; untrack it with `git rm --cached \
             .claude/settings.local.json`"
                .into(),
        );
    } else if load_settings(&settings).is_none() {
        messages.push(
            "anthology memory: .claude/settings.local.json is not a valid JSON object; left \
             untouched, so autoMemoryDirectory is not set"
                .into(),
        );
    } else {
        changed = ensure_memory_dir(root, &settings)?.1;
        match ensure_ignored(root, env, deps) {
            Ok(_) => {}
            Err(e @ PyExc::Other("UnsafeTarget", _)) => messages.push(format!(
                "anthology memory: could not git-ignore settings.local.json: {}",
                e.py_str()
            )),
            Err(e) => return Err(e),
        }
    }
    (deps.regenerate_index)(&repo_mem, DEFAULT_BUDGET, REPO_EXCLUDE)?;
    for d in &agents {
        (deps.regenerate_index)(d, DEFAULT_BUDGET, REPO_EXCLUDE)?;
    }
    if !changed && !auto_memory_off_by_repo(&claude) {
        return Ok(None);
    }
    if changed {
        messages.push(DIR_SET.into());
    }
    let text = context_index(&repo_mem, REPO_EXCLUDE)?;
    let text = if pystr::strip_ws(&text).is_empty() {
        "(no notes yet)\n".to_string()
    } else {
        text
    };
    Ok(Some(section("Repo memory", &repo_mem, &text)))
}

fn run_inner(cwd: &Path, env: &Env, deps: &SyncDeps<'_>) -> Result<(String, Vec<String>), PyExc> {
    let config = crate::repo::config_dir(env);
    let home = config.join("anthology");
    let root = (deps.repo_root)(cwd, env, deps.git);
    let approved = root
        .as_ref()
        .is_some_and(|r| is_approved(&repo_key(r, env, deps.git), env));
    if auto_memory_off(root.as_deref(), &config, env, approved) {
        return Ok((String::new(), Vec::new()));
    }
    let mut messages = Vec::new();
    let mut cover = None;
    if let Some(root) = &root {
        if is_approved(&repo_key(root, env, deps.git), env) {
            // `except Exception`: an error or a panic here still leaves the personal section.
            let step = catch_unwind(AssertUnwindSafe(|| {
                sync_repo(root, &home, &config, env, &mut messages, deps)
            }));
            let failed = match step {
                Ok(Ok(c)) => {
                    cover = c;
                    None
                }
                Ok(Err(e)) => Some(exc_text(&e)),
                Err(payload) => Some(panic_text(payload.as_ref())),
            };
            if let Some(why) = failed {
                messages.push(format!(
                    "anthology memory: repo memory step failed: {}",
                    cut120(&why)
                ));
            }
        } else if root.join(".claude").join("memory").is_dir()
            || root.join(".claude").join("agent-memory").is_dir()
        {
            messages.push(HINT.to_string());
        }
    }
    let personal = home.join("personal-memory");
    let mut text = String::new();
    let unsafe_personal = personal_unsafe(&personal, &mut messages);
    if !unsafe_personal && personal.is_dir() {
        let built = (deps.regenerate_index)(&personal, DEFAULT_BUDGET, &[])
            .and_then(|_| context_index(&personal, &[]));
        match built {
            Ok(t) => text = t,
            Err(e @ PyExc::OSError { .. }) => messages.push(format!(
                "anthology memory: personal memory index: {}",
                strerror(&e)
            )),
            Err(e) => return Err(e),
        }
    }
    let p = s(&personal);
    let mut sections = vec![if unsafe_personal {
        format!("## Personal memory\n{}\n", personal_unavailable(&p))
    } else {
        let body = if pystr::strip_ws(&text).is_empty() {
            "(no notes yet)\n"
        } else {
            text.as_str()
        };
        format!("## Personal memory ({p})\n{}\n\n{body}", personal_save(&p))
    }];
    if let Some(c) = cover {
        sections.push(c);
    }
    Ok((sections.join("\n"), messages))
}

/// `run`: the SessionStart memory step, `(context text, messages)`. Never panics outward: an
/// error or a panic is one `anthology memory: …` message (cut to 120 characters) and no context.
pub fn run(cwd: &Path, env: &Env, deps: &SyncDeps) -> (String, Vec<String>) {
    match catch_unwind(AssertUnwindSafe(|| run_inner(cwd, env, deps))) {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => (
            String::new(),
            vec![format!("anthology memory: {}", cut120(&exc_text(&e)))],
        ),
        Err(payload) => (
            String::new(),
            vec![format!(
                "anthology memory: {}",
                cut120(&panic_text(payload.as_ref()))
            )],
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitlines_matches_python() {
        assert_eq!(
            splitlines("a\nb\r\nc\rd\u{2028}e\x0cf"),
            ["a", "b", "c", "d", "e", "f"]
        );
        assert_eq!(splitlines("a\n"), ["a"]);
        assert_eq!(splitlines(""), Vec::<&str>::new());
        assert_eq!(splitlines("\n\n"), ["", ""]);
    }

    #[test]
    fn strerror_drops_the_errno_and_filename() {
        let e = PyExc::OSError {
            errno: 13,
            filename: Some("/x".into()),
        };
        assert_eq!(strerror(&e), "Permission denied");
        assert_eq!(strerror(&PyExc::Other("X", "m".into())), "m");
    }

    #[test]
    fn stem_drops_three_code_points() {
        assert_eq!(stem_of("a.md"), "a");
        assert_eq!(stem_of("é.md"), "é");
        assert_eq!(stem_of("md"), "");
    }
}
