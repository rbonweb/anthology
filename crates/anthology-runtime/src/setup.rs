//! `anthology setup preview|apply|remove` (RT4; port of
//! `plugins/anthology/scripts/anthology_setup.py`, GC-10).
//!
//! `preview` writes nothing: the environment checks, duplicate plugins, the rtk step, the
//! settings.json diff (with the sha256 of the text it came from), the memory steps and, new
//! (F-15), the located guardrails folder whose binary apply ensures. `apply` checks
//! `--expect-sha256` three times (before anything, after the downloads, right before the
//! write), records the ops in state.json before writing settings.json, ensures the
//! guardrails-pinned binary (`bootstrap.sh session --plugin guardrails --quiet --budget 60`,
//! D§8.8) and reports `guard_binary: {status}`. `remove` undoes the settings edits, deletes
//! state.json, the downloaded rtk, rtk's history folder (`state::rtk_data_dir`), `<ah>/bin`
//! (only when `installed_plugins.json` has no `anthology-guardrails@*` entry, D§8.8), and
//! optionally drops approvals.
//!
//! Every result is a JSON object; [`main`] prints it with `indent=2` and exits 0, 1 (`ok` is
//! false) or 2 (unsupported platform). An exception Python would let reach `main` is the
//! result `{"ok": false, "error": "<Class>: <msg>"}` (GC-5).
//!
//! Seams (GC-19): [`SetupDeps`]. The memory functions are `anthology_memory`'s; the three
//! Python's tests patch (`repo_root`, `repo_key`, `approve`) and the host name are in
//! [`MemoryDeps`].
#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anthology_core::git::RealGit;
use anthology_core::{Env, clock, fsx, paths, platform, state};
use anthology_memory::sync::{self, Placement, SyncDeps};
use anthology_memory::{approve as memory_approve, notes, privacy, render, repo as memory_repo};
use anthology_pycompat::argparse::{self, Arg, Spec, Subcommands};
use anthology_pycompat::difflib;
use anthology_pycompat::json::{self, Map, Value};
use anthology_pycompat::path as pypath;
use anthology_pycompat::pyerr::PyExc;
use anthology_pycompat::str as pystr;
use anthology_settings::{self as settings, SettingsLimits};
use sha2::{Digest, Sha256};

use crate::bootstrap::{self, GuardBinaryStatus};
use crate::compat;
use crate::doctor;
use crate::guardrails_loc::{self, GUARDRAILS_PREFIX};
use crate::rtk_install;
use crate::session_start::{RealRtk, RtkWiring, StateWrite};

/// Windows (setup refuses everything there).
pub const REFUSAL: &str = "Anthology supports macOS and Linux only; it is inactive on Windows.";
/// Seconds per read for the rtk download (overall about 4×): setup is where the first download
/// happens (GC-9: T = 60).
pub const RTK_TIMEOUT: f64 = 60.0;
/// Seconds the guardrails-pinned binary may take to install (D§8.8).
pub const GUARD_BUDGET: u64 = 60;
/// `_settings_error(FileNotFoundError)` (F-6: `hooks/guard.sh`).
pub const GUARD_NOT_FOUND: &str = "the anthology-guardrails guard (hooks/guard.sh) was not found \
                                   where Claude Code runs it; reinstall or repair the \
                                   anthology-guardrails plugin";
/// The first `--expect-sha256` check failed.
pub const CHANGED_SINCE_PREVIEW: &str =
    "settings.json changed since the preview; run the preview again";
/// The second or third check failed.
pub const CHANGED_DURING_SETUP: &str =
    "settings.json changed during setup; it was left untouched. Run the preview again.";
/// remove never deletes notes.
pub const NOTES_KEPT: &str = "kept (memory notes are never deleted)";
/// remove keeps `<ah>/bin` while the guardrails plugin is installed (GC-14).
pub const BIN_KEPT: &str = "kept (anthology-guardrails still uses it)";

/// The memory functions Python's tests patch (`memory_sync.repo_root`, `repo_key`, `approve`,
/// and the privacy scan's `socket.gethostname`). `Default` is production.
pub struct MemoryDeps<'a> {
    /// `memory_sync.repo_root(cwd, env)`: the repo's top folder, or `None` outside a repo.
    pub repo_root: &'a dyn Fn(&Path, &Env) -> Option<PathBuf>,
    /// `memory_sync.repo_key(root, env)`.
    pub repo_key: &'a dyn Fn(&Path, &Env) -> String,
    /// `memory_sync.approve(root, env)`: the key. `PyExc::Other("UnsafeTarget", _)` is
    /// Python's `UnsafeTarget`, raised after the approval is recorded.
    pub approve: &'a dyn Fn(&Path, &Env) -> Result<String, PyExc>,
    /// `socket.gethostname()` for the privacy scan; `None` is the machine's host name.
    pub hostname: Option<fn() -> Option<String>>,
}

/// doctor's functions setup reports (RT1b). Values are the JSON setup prints.
pub struct SetupDoctor<'a> {
    /// `doctor.check_env(env, lock=<plugin root>/rtk.lock, plugin_root, cwd)`, as the JSON list
    /// of `{"name", "ok", "detail"}` (args: env, plugin root, cwd).
    pub checks: &'a dyn Fn(&Env, &Path, &Path) -> Value,
    /// `doctor.installed_plugins_state(env)`: the parsed `installed_plugins.json` and `"ok"`,
    /// `"missing"` or `"unreadable"`.
    pub installed_plugins_state: &'a dyn Fn(&Env) -> (Option<Value>, &'static str),
    /// `doctor.find_duplicates(installed)`, as the JSON list of `{"plugin", "scope", "command"}`.
    pub find_duplicates: &'a dyn Fn(Option<&Value>) -> Value,
}

/// The seams of setup (GC-19). `SetupDeps::default()` is production.
pub struct SetupDeps<'a> {
    /// `(platform.system(), platform.machine())`.
    pub uname: fn() -> (String, String),
    /// `state.write(**kv)`.
    pub state_write: StateWrite<'a>,
    /// `settings_patch.write_text(path, text)`.
    pub settings_write: &'a dyn Fn(&Path, &str) -> Result<(), PyExc>,
    pub memory: MemoryDeps<'a>,
    pub doctor: SetupDoctor<'a>,
    /// `rtk_install`; `None` is a missing module (`rtk wiring not present`).
    pub rtk: Option<&'a dyn RtkWiring>,
    /// `ensure_live_guard_binary(guard_root, budget, env)` (RT2, D§8.8).
    pub ensure_guard_binary: &'a dyn Fn(&Path, u64, &Env) -> GuardBinaryStatus,
}

fn repo_root_default(cwd: &Path, env: &Env) -> Option<PathBuf> {
    memory_repo::repo_root(cwd, env, &RealGit)
}

fn repo_key_default(root: &Path, env: &Env) -> String {
    memory_repo::repo_key(root, env, &RealGit)
}

fn approve_default(root: &Path, env: &Env) -> Result<String, PyExc> {
    memory_approve::approve(root, env, &RealGit)
}

/// `doctor.check_env(env, lock=_lock_path(root), plugin_root=root, cwd=cwd)`.
fn checks_default(env: &Env, root: &Path, cwd: &Path) -> Value {
    let deps = doctor::DoctorDeps {
        rtk_lock: Some(root.join("rtk.lock")),
        ..Default::default()
    };
    Value::Array(
        doctor::check_env(env, root, cwd, &deps)
            .iter()
            .map(doctor::check_value)
            .collect(),
    )
}

fn installed_plugins_state_default(env: &Env) -> (Option<Value>, &'static str) {
    doctor::installed_plugins_state(env)
}

fn find_duplicates_default(installed: Option<&Value>) -> Value {
    Value::Array(
        doctor::find_duplicates_in(installed)
            .iter()
            .map(doctor::duplicate_value)
            .collect(),
    )
}

fn settings_write_default(path: &Path, text: &str) -> Result<(), PyExc> {
    settings::write_text(path, text)
}

impl Default for MemoryDeps<'static> {
    fn default() -> MemoryDeps<'static> {
        MemoryDeps {
            repo_root: &repo_root_default,
            repo_key: &repo_key_default,
            approve: &approve_default,
            hostname: None,
        }
    }
}

impl Default for SetupDoctor<'static> {
    fn default() -> SetupDoctor<'static> {
        SetupDoctor {
            checks: &checks_default,
            installed_plugins_state: &installed_plugins_state_default,
            find_duplicates: &find_duplicates_default,
        }
    }
}

impl Default for SetupDeps<'static> {
    fn default() -> SetupDeps<'static> {
        SetupDeps {
            uname: platform::uname,
            state_write: &state::write,
            settings_write: &settings_write_default,
            memory: MemoryDeps::default(),
            doctor: SetupDoctor::default(),
            rtk: Some(&RealRtk),
            ensure_guard_binary: &bootstrap::ensure_live_guard_binary,
        }
    }
}

// --- small Python semantics -----------------------------------------------------------------

/// Python truthiness of a JSON value.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Int(json::Int::Small(n)) => *n != 0,
        Value::Int(json::Int::Big(_)) => true,
        Value::Float(f) => *f != 0.0,
        Value::Str(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `str(v)` of a JSON value: a str as is, `True`/`False`/`None`, numbers as JSON prints them
/// (Python's repr); containers as JSON (Python's repr quotes differ; only a hand-made file
/// holds one there).
fn py_str(v: &Value) -> String {
    match v {
        Value::Str(s) => s.clone(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Null => "None".into(),
        other => json::dumps(other, &json::PY_DEFAULT),
    }
}

/// `str(path)` as pathlib prints it.
fn pstr(p: &Path) -> String {
    String::from_utf8_lossy(&compat::pathlib_str(p.as_os_str().as_bytes())).into_owned()
}

fn s(v: &str) -> Value {
    Value::Str(v.into())
}

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    let mut m = Map::new();
    for (k, v) in pairs {
        m.insert(k.into(), v);
    }
    Value::Object(m)
}

fn strs(items: &[String]) -> Value {
    Value::Array(items.iter().map(|i| s(i)).collect())
}

fn opt_str(v: Option<String>) -> Value {
    v.map_or(Value::Null, Value::Str)
}

fn refused(error: &str) -> Value {
    obj(vec![("ok", Value::Bool(false)), ("error", s(error))])
}

/// `hashlib.sha256(text.encode("utf-8")).hexdigest()`.
fn sha256(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// An exception that reaches `main`: `"<Class>: <str(exc)>"` (GC-5).
fn raised(e: PyExc) -> String {
    format!("{}: {}", e.type_name(), e.py_str())
}

/// An `OSError` for `path` from an `io::Error`.
fn os_err(e: io::Error, path: &Path) -> String {
    let mut exc = PyExc::from(e);
    if let PyExc::OSError { filename, .. } = &mut exc {
        *filename = Some(pstr(path));
    }
    raised(exc)
}

/// `(str(exc) or type(exc).__name__)`.
fn exc_text(e: &PyExc) -> String {
    let t = e.py_str();
    if t.is_empty() {
        e.type_name().to_string()
    } else {
        t
    }
}

/// `os.path.lexists`.
fn lexists(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok()
}

fn is_symlink(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink())
}

/// The classes anthology-settings reports as `"<Class>: …"` that setup does not catch (it
/// catches ValueError and FileNotFoundError), so they reach `main`.
const RAISED_CLASSES: [&str; 4] = [
    "TypeError: ",
    "KeyError: ",
    "AttributeError: ",
    "RecursionError: ",
];

/// What setup's `except (ValueError, FileNotFoundError)` makes of an error.
enum Refusal {
    /// Caught: the message setup shows.
    Caught(String),
    /// Not caught: `main`'s `"<Class>: <msg>"`.
    Raised(String),
}

/// Sorts an anthology-settings error (its convention: a ValueError is `str(exc)`, any other
/// class starts with the class name). A `JSONDecodeError`, a ValueError subclass, is shown
/// without the class prefix the scanner gives it, and so is a `UnicodeDecodeError` (also one).
fn classify(msg: String) -> Refusal {
    if RAISED_CLASSES.iter().any(|c| msg.starts_with(c)) {
        return Refusal::Raised(msg);
    }
    match msg
        .strip_prefix("JSONDecodeError: ")
        .or_else(|| msg.strip_prefix("UnicodeDecodeError: "))
    {
        Some(rest) => Refusal::Caught(rest.to_string()),
        None => Refusal::Caught(msg),
    }
}

// --- plugin, data folder, rtk ---------------------------------------------------------------

fn unsupported(deps: &SetupDeps<'_>) -> bool {
    (deps.uname)().0 == "Windows"
}

/// `cwd or os.getcwd()`.
fn effective_cwd(cwd: Option<&Path>) -> Result<PathBuf, String> {
    match cwd {
        Some(c) if !c.as_os_str().is_empty() => Ok(c.to_path_buf()),
        _ => std::env::current_dir().map_err(|e| raised(PyExc::from(e))),
    }
}

/// `_plugin_version(root)`: `str(plugin.json["version"])`, or `unknown`.
fn plugin_version(root: &Path) -> String {
    let read = || -> Option<String> {
        let raw = fs::read(root.join(".claude-plugin").join("plugin.json")).ok()?;
        let text = String::from_utf8(raw).ok()?;
        let data = json::loads(&text).ok()?;
        data.as_object()?.get("version").map(py_str)
    };
    read().unwrap_or_else(|| "unknown".into())
}

/// `_data_dir(env)`: `CLAUDE_PLUGIN_DATA`, else what session start recorded.
fn data_dir(env: &Env) -> Option<String> {
    if let Some(v) = env.get_os("CLAUDE_PLUGIN_DATA").filter(|v| !v.is_empty()) {
        return Some(v.to_string_lossy().into_owned());
    }
    state::read(env)
        .get("data_dir")
        .filter(|v| truthy(v))
        .map(py_str)
}

/// The located guardrails folder. Python's `setup_ops` asked `guardrails_loc` without a cwd
/// (the process's working folder), so this does too.
fn guardrails_root(root: &Path, env: &Env) -> Option<PathBuf> {
    guardrails_loc::inspect(root, env, Path::new("")).root
}

/// `_rtk_preview(root, data_dir)`: the rtk step setup would take. The target is
/// `rtk_install.target` for `deps.uname`.
pub fn rtk_preview(root: &Path, data_dir: Option<&str>, deps: &SetupDeps<'_>) -> Value {
    let Some(rtk) = deps.rtk else {
        return obj(vec![("skipped", s("rtk wiring not present"))]);
    };
    let Ok(lock) = rtk.load_lock(&root.join("rtk.lock")) else {
        return obj(vec![("skipped", s("rtk wiring not present (no rtk.lock)"))]);
    };
    let (system, machine) = (deps.uname)();
    let target = match rtk_install::target(&system, &machine) {
        Ok(t) => t,
        Err(e) => return obj(vec![("error", s(&exc_text(&e)))]),
    };
    let dest = data_dir
        .filter(|d| !d.is_empty())
        .map(|d| pstr(&Path::new(d).join("rtk").join(&lock.version).join("rtk")));
    obj(vec![
        ("version", s(&lock.version)),
        ("target", s(target.triple())),
        ("dest", opt_str(dest)),
    ])
}

/// `_rtk_apply(root, data_dir)`: `(status, warning)` after the reconcile (download and verify
/// when missing or stale).
pub fn rtk_apply(
    root: &Path,
    data_dir: Option<&str>,
    env: &Env,
    deps: &SetupDeps<'_>,
) -> (String, Option<String>) {
    let Some(rtk) = deps.rtk else {
        return ("skipped: rtk wiring not present".into(), None);
    };
    let Some(dir) = data_dir.filter(|d| !d.is_empty()) else {
        return (
            "skipped: plugin data folder unknown".into(),
            Some(
                "rtk was not installed: the plugin data folder is not known yet; it is installed \
                 at the next session start"
                    .into(),
            ),
        );
    };
    let failed = |e: &PyExc| {
        (
            "failed".to_string(),
            Some(format!(
                "rtk install failed: {}",
                pystr::char_slice(&exc_text(e), 0, 200)
            )),
        )
    };
    let lock = match rtk.load_lock(&root.join("rtk.lock")) {
        Ok(l) => l,
        Err(e) => return failed(&e),
    };
    match rtk.reconcile(env, Path::new(dir), &lock, RTK_TIMEOUT) {
        Err(e) => failed(&e),
        Ok(r) => match r.warning.filter(|w| !w.is_empty()) {
            Some(w) => ("warning".into(), Some(w)),
            None if r.installed => ("installed".into(), None),
            None => ("up to date".into(), None),
        },
    }
}

// --- settings -------------------------------------------------------------------------------

struct Plan {
    path: PathBuf,
    text: String,
    new_text: String,
    diff: String,
    stored: Vec<Value>,
    ops_version: String,
}

/// `read_text(path)`: `""` when it does not exist; any other error reaches `main`.
fn read_settings(path: &Path) -> Result<String, String> {
    settings::read_text(path)
        .map(Option::unwrap_or_default)
        .map_err(raised)
}

/// `_settings_plan(root, env)`. Python's `setup_ops` raised FileNotFoundError when no
/// guardrails folder was located (it loaded the profile roots from it); the roots are compiled
/// in now (F-7), so the located folder is checked here, after the plugin-root validation, as
/// before (session start skips its ops note in that case too).
fn settings_plan(root: &Path, env: &Env) -> Result<Plan, Refusal> {
    let path = settings::settings_path(env);
    let text = read_settings(&path).map_err(Refusal::Raised)?;
    let ops = settings::setup_ops(root, env).map_err(classify)?;
    if guardrails_root(root, env).is_none() {
        return Err(Refusal::Caught(GUARD_NOT_FOUND.into()));
    }
    let previous = settings::stored_ops(state::read(env).get("settings_ops")).map_err(classify)?;
    let (new_text, diff, stored) = settings::reconcile(&text, &ops, &previous).map_err(classify)?;
    Ok(Plan {
        path,
        text,
        new_text,
        diff,
        stored,
        ops_version: settings::ops_version(&ops, &SettingsLimits::default()),
    })
}

// --- memory ---------------------------------------------------------------------------------

/// Production sync deps: privacy falls back to the hook's `HOME`, and the host name is the
/// patched one when a test gave it (`socket.gethostname`).
fn sync_deps(env: &Env, deps: &SetupDeps<'_>) -> SyncDeps<'static> {
    let mut sd = SyncDeps::from_env(env);
    if let Some(hostname) = deps.memory.hostname {
        sd.privacy.hostname = hostname;
    }
    sd
}

/// `_unsafe_dir(path)`: a symlink or not a folder (an absent path is fine).
fn unsafe_dir(path: &Path) -> bool {
    memory_repo::unsafe_dir(path).is_some()
}

/// `_personal_dir(env)`.
fn personal_dir(env: &Env) -> PathBuf {
    paths::anthology_home(env).join("personal-memory")
}

/// `_repo_info(cwd, env)`: the repo's root and key.
fn repo_info(cwd: &Path, env: &Env, deps: &SetupDeps<'_>) -> Option<(PathBuf, String)> {
    let root = (deps.memory.repo_root)(cwd, env)?;
    let key = (deps.memory.repo_key)(&root, env);
    Some((root, key))
}

/// `_repo_mem(root)`: `<root>/.claude/memory`, or `None` if it or `.claude` is a symlink or
/// not a folder.
fn repo_mem(root: &Path) -> Option<PathBuf> {
    let claude = root.join(".claude");
    let m = claude.join("memory");
    (!unsafe_dir(&claude) && !unsafe_dir(&m)).then_some(m)
}

/// `str.splitlines(True)`.
fn splitlines_keep(t: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = t.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        if matches!(
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
        ) {
            let mut end = i + c.len_utf8();
            if c == '\r' && matches!(it.peek(), Some((_, '\n'))) {
                it.next();
                end += 1;
            }
            out.push(&t[start..end]);
            start = end;
        }
    }
    if start < t.len() {
        out.push(&t[start..]);
    }
    out
}

/// `_diff_lines(data)`: lossy UTF-8 lines, each ending in `\n`.
fn diff_lines(data: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(data);
    splitlines_keep(&text)
        .into_iter()
        .map(|l| {
            if l.ends_with('\n') {
                l.to_string()
            } else {
                format!("{l}\n")
            }
        })
        .collect()
}

/// `"".join(difflib.unified_diff(a, b, fromfile, tofile))`.
fn note_diff(a: &[String], b: &[String], from: &str, to: &str) -> String {
    let a: Vec<&str> = a.iter().map(String::as_str).collect();
    let b: Vec<&str> = b.iter().map(String::as_str).collect();
    difflib::unified_diff_lineterm(&a, &b, from, to, 3, "\n").concat()
}

fn findings_value(findings: &[privacy::Finding]) -> Value {
    Value::Array(
        findings
            .iter()
            .map(|f| {
                obj(vec![
                    ("kind", s(&f.kind)),
                    (
                        "line",
                        Value::Int(json::Int::Small(i64::try_from(f.line).unwrap_or(i64::MAX))),
                    ),
                ])
            })
            .collect(),
    )
}

/// `legacy_notes(root, env)`: the repo's old path-keyed notes and what moving each into
/// `<root>/.claude/memory/` would do. Writes nothing.
fn legacy_notes(root: &Path, env: &Env, deps: &SetupDeps<'_>) -> Vec<Value> {
    let m = repo_mem(root);
    let home = env.get("HOME");
    let sd = sync_deps(env, deps);
    let mut out = Vec::new();
    for old in memory_repo::legacy_dirs(root, &paths::config_dir(env), env, &RealGit) {
        for (name, data) in sync::legacy_entries(&old) {
            let info = notes::note_info(&name, &data);
            let (findings, mut reason) = sync::promotion_check(&info, &data, home, None, &sd);
            if reason.is_none() && m.is_none() {
                reason = Some("repo memory is a symlink or not a folder".into());
            }
            let mut note = Map::new();
            note.insert("file".into(), s(&name));
            note.insert("name".into(), s(&info.name));
            note.insert("description".into(), s(&info.description));
            note.insert("type".into(), opt_str(info.kind));
            note.insert("source".into(), s(&pstr(&old)));
            note.insert("default".into(), s("personal"));
            note.insert("findings".into(), findings_value(&findings));
            note.insert("movable".into(), Value::Bool(reason.is_none()));
            if let Some(r) = reason.filter(|r| !r.is_empty()) {
                note.insert("reason".into(), Value::Str(r));
            }
            let status = match &m {
                Some(dir) => sync::placement_status(dir, &name, &data),
                None => Placement::New,
            };
            let label = match status {
                Placement::New => "new",
                Placement::Identical => "identical",
                Placement::Conflict => "conflict",
            };
            note.insert("status".into(), s(label));
            let diff = match (status, &m) {
                (Placement::New, _) => note_diff(
                    &[],
                    &diff_lines(&data),
                    "/dev/null",
                    &format!("b/.claude/memory/{name}"),
                ),
                (Placement::Conflict, Some(dir)) => {
                    let would = sync::side_by_side_name(dir, &name, &clock::today_local());
                    // `UnsafeTarget` reads as nothing there.
                    let existing = fsx::read_regular(&dir.join(&name), None).unwrap_or_default();
                    note.insert("would_create".into(), s(&would));
                    note_diff(
                        &diff_lines(&existing),
                        &diff_lines(&data),
                        &format!("a/.claude/memory/{name}"),
                        &format!("b/.claude/memory/{would}"),
                    )
                }
                _ => String::new(),
            };
            note.insert("diff".into(), Value::Str(diff));
            out.push(Value::Object(note));
        }
    }
    out
}

/// `_migrate_legacy(root, env, personal, promote)`: `(migrated, blocked)`.
fn migrate_legacy(
    root: &Path,
    env: &Env,
    personal: &Path,
    promote: &[String],
    deps: &SetupDeps<'_>,
) -> Result<(Value, Value), String> {
    let m = repo_mem(root);
    let ledger = paths::anthology_home(env).join("migrated.json");
    let sd = sync_deps(env, deps);
    let (mut repo, mut pers, mut skipped, mut blocked) = (vec![], vec![], vec![], vec![]);
    for old in memory_repo::legacy_dirs(root, &paths::config_dir(env), env, &RealGit) {
        let res = sync::try_migrate(
            &old,
            m.as_deref(),
            personal,
            Some(&ledger),
            None,
            promote,
            env.get("HOME"),
            &sd,
        )
        .map_err(raised)?;
        let promoted = &res.promoted;
        pers.extend(
            res.copied
                .iter()
                .chain(&res.side_by_side)
                .filter(|n| !promoted.contains(n))
                .cloned(),
        );
        skipped.extend(
            res.skipped
                .iter()
                .filter(|n| !promoted.contains(n))
                .cloned(),
        );
        for (i, (file, reason)) in res.blocked.iter().enumerate() {
            let findings = res.blocked_findings.get(i).map_or(&[][..], Vec::as_slice);
            blocked.push(obj(vec![
                ("file", s(file)),
                ("findings", findings_value(findings)),
                ("reason", s(reason)),
            ]));
        }
        repo.extend(res.promoted);
    }
    if !repo.is_empty()
        && let Some(dir) = &m
    {
        render::regenerate_index(dir, render::DEFAULT_BUDGET, notes::REPO_EXCLUDE)
            .map_err(raised)?;
    }
    let migrated = obj(vec![
        ("repo", strs(&repo)),
        ("personal", strs(&pers)),
        ("skipped", strs(&skipped)),
    ]);
    Ok((migrated, Value::Array(blocked)))
}

/// `_promote_error(promote, skip_approve, cwd, env)`: why `--promote` cannot be honoured
/// (checked before anything is written).
fn promote_error(
    promote: &[String],
    skip_approve: bool,
    cwd: &Path,
    env: &Env,
    deps: &SetupDeps<'_>,
) -> Option<String> {
    if promote.is_empty() {
        return None;
    }
    if skip_approve {
        return Some(
            "--promote moves notes into this repo's memory, which needs the repo approved; it \
             cannot be combined with --skip-approve. Nothing was written."
                .into(),
        );
    }
    let Some((root, _)) = repo_info(cwd, env, deps) else {
        return Some(
            "--promote needs a git repo (this folder is not one). Nothing was written.".into(),
        );
    };
    let listed: BTreeSet<String> = legacy_notes(&root, env, deps)
        .iter()
        .filter_map(|n| n.as_object()?.get("file")?.as_str().map(String::from))
        .collect();
    let unknown: BTreeSet<&str> = promote
        .iter()
        .map(String::as_str)
        .filter(|p| !listed.contains(*p))
        .collect();
    if unknown.is_empty() {
        return None;
    }
    let names: Vec<&str> = unknown.into_iter().collect();
    Some(format!(
        "--promote takes the file name of one of this repo's legacy notes \
         (memory.legacy_notes in the preview); not one: {}. Nothing was written.",
        names.join(", ")
    ))
}

// --- commands -------------------------------------------------------------------------------

/// The result for an exception that reached the top: what `main` prints.
fn result_or_error(r: Result<Value, String>) -> Value {
    r.unwrap_or_else(|e| refused(&e))
}

/// `preview(plugin_root, cwd, env)`: what setup would do; writes nothing.
pub fn preview(plugin_root: &Path, cwd: Option<&Path>, env: &Env, deps: &SetupDeps) -> Value {
    result_or_error(try_preview(plugin_root, cwd, env, deps))
}

fn try_preview(
    plugin_root: &Path,
    cwd: Option<&Path>,
    env: &Env,
    deps: &SetupDeps<'_>,
) -> Result<Value, String> {
    if unsupported(deps) {
        return Ok(obj(vec![
            ("supported", Value::Bool(false)),
            ("message", s(REFUSAL)),
        ]));
    }
    let cwd = effective_cwd(cwd)?;
    let data_dir = data_dir(env);
    let path = settings::settings_path(env);
    let text = read_settings(&path)?;
    let mut st = Map::new();
    st.insert("path".into(), s(&pstr(&path)));
    st.insert("sha256".into(), s(&sha256(&text)));
    st.insert("bypass_note".into(), s(settings::BYPASS_NOTE));
    match settings_plan(plugin_root, env) {
        Ok(plan) => {
            st.insert("changed".into(), Value::Bool(plan.new_text != text));
            st.insert("diff".into(), Value::Str(plan.diff));
        }
        Err(Refusal::Caught(m)) => {
            st.insert("changed".into(), Value::Bool(false));
            st.insert("diff".into(), s(""));
            st.insert("error".into(), Value::Str(m));
        }
        Err(Refusal::Raised(m)) => return Err(m),
    }
    let info = repo_info(&cwd, env, deps);
    let repo = match &info {
        None => obj(vec![("skipped", s("not a git repo"))]),
        Some((root, key)) => obj(vec![
            ("root", s(&pstr(root))),
            ("key", s(key)),
            (
                "approved",
                Value::Bool(memory_approve::is_approved(key, env)),
            ),
        ]),
    };
    let legacy = info
        .map(|(root, _)| legacy_notes(&root, env, deps))
        .unwrap_or_default();
    let personal = personal_dir(env);
    let (installed, installed_state) = (deps.doctor.installed_plugins_state)(env);
    let guard = guardrails_root(plugin_root, env);
    Ok(obj(vec![
        ("supported", Value::Bool(true)),
        (
            "doctor",
            obj(vec![(
                "checks",
                (deps.doctor.checks)(env, plugin_root, &cwd),
            )]),
        ),
        (
            "duplicates",
            (deps.doctor.find_duplicates)(installed.as_ref()),
        ),
        ("installed_plugins", s(installed_state)),
        ("rtk", rtk_preview(plugin_root, data_dir.as_deref(), deps)),
        ("settings", Value::Object(st)),
        (
            "memory",
            obj(vec![
                ("repo", repo),
                ("personal_dir", s(&pstr(&personal))),
                ("personal_exists", Value::Bool(personal.is_dir())),
                ("legacy_notes", Value::Array(legacy)),
            ]),
        ),
        (
            "guard_binary",
            obj(vec![("root", opt_str(guard.map(|g| pstr(&g))))]),
        ),
    ]))
}

/// `apply(plugin_root, expect_sha256, cwd, skip_approve, env, promote)`: apply the previewed
/// setup. `promote` names legacy notes (bare file names from the preview's
/// `memory.legacy_notes`) to move into the repo's memory instead of personal memory.
pub fn apply(
    plugin_root: &Path,
    expect_sha256: &str,
    cwd: Option<&Path>,
    skip_approve: bool,
    env: &Env,
    promote: &[String],
    deps: &SetupDeps,
) -> Value {
    result_or_error(try_apply(
        plugin_root,
        expect_sha256,
        cwd,
        skip_approve,
        env,
        promote,
        deps,
    ))
}

fn try_apply(
    root: &Path,
    expect: &str,
    cwd: Option<&Path>,
    skip_approve: bool,
    env: &Env,
    promote: &[String],
    deps: &SetupDeps<'_>,
) -> Result<Value, String> {
    if unsupported(deps) {
        return Ok(obj(vec![
            ("ok", Value::Bool(false)),
            ("unsupported", Value::Bool(true)),
            ("error", s(REFUSAL)),
        ]));
    }
    let cwd = effective_cwd(cwd)?;
    if let Some(r) = promote_error(promote, skip_approve, &cwd, env, deps) {
        return Ok(refused(&r));
    }
    let mut warnings: Vec<String> = Vec::new();
    let path = settings::settings_path(env);
    // Check 1 of 3: nothing is downloaded for a stale preview or a bad plan.
    if sha256(&read_settings(&path)?) != expect {
        return Ok(refused(CHANGED_SINCE_PREVIEW));
    }
    match settings_plan(root, env) {
        Ok(_) => {}
        Err(Refusal::Caught(m)) => return Ok(refused(&m)),
        Err(Refusal::Raised(m)) => return Err(m),
    }

    let data_dir = data_dir(env);
    let (rtk_status, warn) = rtk_apply(root, data_dir.as_deref(), env, deps);
    warnings.extend(warn);
    let guard = match guardrails_root(root, env) {
        Some(g) => (deps.ensure_guard_binary)(&g, GUARD_BUDGET, env),
        None => GuardBinaryStatus::Skipped("guardrails not found"),
    };
    let guard_value = obj(vec![("status", s(&guard.status_text()))]);

    // Check 2 of 3: the downloads can take a while. Re-read settings.json and recompute from
    // what is on disk, so an edit made meanwhile is either kept (we refuse) or never
    // overwritten.
    let stale = |warnings: &[String]| {
        obj(vec![
            ("ok", Value::Bool(false)),
            ("rtk", obj(vec![("status", s(&rtk_status))])),
            ("warnings", strs(warnings)),
            ("error", s(CHANGED_DURING_SETUP)),
            ("guard_binary", guard_value.clone()),
        ])
    };
    if sha256(&read_settings(&path)?) != expect {
        return Ok(stale(&warnings));
    }
    let plan = match settings_plan(root, env) {
        Ok(p) => p,
        Err(Refusal::Caught(m)) => return Ok(refused(&m)),
        Err(Refusal::Raised(m)) => return Err(m),
    };
    let before = state::read(env);
    let was_created = before.get("settings_created").is_some_and(truthy);
    let created = was_created || !lexists(&plan.path);
    let changed = plan.new_text != plan.text;
    // Record what we are about to do before doing it, so remove can always undo it.
    (deps.state_write)(
        env,
        vec![
            ("settings_ops".into(), Value::Array(plan.stored.clone())),
            ("settings_created".into(), Value::Bool(created)),
        ],
    )
    .map_err(raised)?;
    if changed {
        // Check 3 of 3: right before the write.
        if sha256(&read_settings(&path)?) != expect {
            let previous = before
                .get("settings_ops")
                .filter(|v| truthy(v))
                .cloned()
                .unwrap_or(Value::Array(Vec::new()));
            (deps.state_write)(
                env,
                vec![
                    ("settings_ops".into(), previous),
                    ("settings_created".into(), Value::Bool(was_created)),
                ],
            )
            .map_err(raised)?;
            return Ok(stale(&warnings));
        }
        (deps.settings_write)(&plan.path, &plan.new_text).map_err(raised)?;
    }

    let info = if skip_approve {
        None
    } else {
        repo_info(&cwd, env, deps)
    };
    let personal = personal_dir(env);
    let fresh = !personal.is_dir();
    fs::create_dir_all(&personal).map_err(|e| os_err(e, &personal))?;
    if fresh {
        fs::set_permissions(&personal, fs::Permissions::from_mode(0o700))
            .map_err(|e| os_err(e, &personal))?;
    }
    let (mut approved_key, mut approve_error) = (Value::Null, Value::Null);
    let (mut migrated, mut blocked) = (Value::Null, Value::Array(Vec::new()));
    if let Some((repo_root, key)) = info {
        approved_key = Value::Str(key);
        match (deps.memory.approve)(&repo_root, env) {
            Ok(k) => approved_key = Value::Str(k),
            // The approval is already recorded.
            Err(PyExc::Other("UnsafeTarget", msg)) => {
                warnings.push(format!("repo memory is approved, but: {msg}"));
                approve_error = Value::Str(msg);
            }
            Err(e) => return Err(raised(e)),
        }
        (migrated, blocked) = migrate_legacy(&repo_root, env, &personal, promote, deps)?;
    }
    let memory = obj(vec![
        ("approved_key", approved_key),
        ("approve_error", approve_error),
        ("migrated", migrated),
        ("blocked", blocked),
        ("personal_dir", s(&pstr(&personal))),
    ]);

    // The op set this run applied: session start compares it with the current release's.
    let mut done = vec![
        ("setup_done".to_string(), s(&plugin_version(root))),
        ("settings_ops_version".to_string(), s(&plan.ops_version)),
    ];
    if let Some(d) = &data_dir {
        done.push(("data_dir".into(), s(d)));
    }
    (deps.state_write)(env, done).map_err(raised)?;
    Ok(obj(vec![
        ("ok", Value::Bool(true)),
        (
            "settings",
            obj(vec![
                ("path", s(&pstr(&plan.path))),
                ("changed", Value::Bool(changed)),
                ("diff", Value::Str(plan.diff)),
            ]),
        ),
        ("rtk", obj(vec![("status", s(&rtk_status))])),
        ("memory", memory),
        ("warnings", strs(&warnings)),
        ("guard_binary", guard_value),
    ]))
}

/// `_rtk_target(env)`: `(real path to delete or None, status, warning or None)` for the
/// downloaded rtk folder. Only `realpath(data_dir)/rtk` is ever deleted, and only when
/// data_dir sits under `<config>/plugins/data/` and the folder is not a symlink.
fn rtk_target(env: &Env) -> (Option<String>, &'static str, Option<String>) {
    let Some(d) = data_dir(env) else {
        return (None, "no plugin data folder recorded", None);
    };
    let rtk_dir = Path::new(&d).join("rtk");
    if !lexists(&rtk_dir) {
        return (None, "nothing to delete", None);
    }
    if is_symlink(&rtk_dir) {
        return (
            None,
            "left in place",
            Some("the rtk folder is a symlink; left in place".into()),
        );
    }
    let text = |p: PathBuf| p.to_string_lossy().into_owned();
    let data_real = text(pypath::realpath(Path::new(&d)));
    let data_root = pypath::join(
        &pypath::join(&text(pypath::realpath(&paths::config_dir(env))), "plugins"),
        "data",
    );
    let real = text(pypath::realpath(Path::new(&pstr(&rtk_dir))));
    if !data_real.starts_with(&format!("{data_root}/")) || real != pypath::join(&data_real, "rtk") {
        return (
            None,
            "left in place",
            Some("the rtk folder is not under the plugin data folder; left in place".into()),
        );
    }
    (Some(real), "delete", None)
}

/// `shutil.rmtree(path, onerror=_ignore_vanished)`: a file that disappears mid-delete (a
/// SQLite -wal/-shm) is fine. std's `remove_dir_all` already skips entries that vanish under
/// it; this also accepts the folder itself being gone.
fn rmtree_ignoring_vanished(path: &Path) -> Result<(), String> {
    match fs::remove_dir_all(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(os_err(e, path)),
        _ => Ok(()),
    }
}

/// `_rtk_history_target()`: `(path to delete or None, status, warning or None)` for rtk's
/// history folder, [`state::rtk_data_dir`]. Deleted only when it is a real folder, not a
/// symlink.
fn rtk_history_target(env: &Env) -> (Option<PathBuf>, &'static str, Option<String>) {
    let path = state::rtk_data_dir(env);
    if !lexists(&path) {
        return (None, "nothing to delete", None);
    }
    if is_symlink(&path) || !path.is_dir() {
        let warn = format!(
            "the rtk history folder {} is not a plain folder; left in place",
            pstr(&path)
        );
        return (None, "left in place", Some(warn));
    }
    (Some(path), "delete", None)
}

/// Whether `installed_plugins.json` has an `anthology-guardrails@*` entry; a file that exists
/// but cannot be read, or has no `plugins` object, counts as one (keep the binary).
fn guardrails_installed(env: &Env) -> bool {
    let p = compat::config_dir(env)
        .join("plugins")
        .join("installed_plugins.json");
    if !lexists(&p) {
        return false;
    }
    let Some(data) = compat::read_json(&p) else {
        return true;
    };
    data.as_object()
        .and_then(|m| m.get("plugins"))
        .and_then(Value::as_object)
        .is_none_or(|plugins| {
            plugins
                .iter()
                .any(|(k, _)| k.starts_with(GUARDRAILS_PREFIX))
        })
}

/// `<ah>/bin` (D§8.8): `(folder to delete or None, status)`.
fn bin_target(env: &Env) -> (Option<PathBuf>, &'static str) {
    let bin = paths::anthology_home(env).join("bin");
    if !lexists(&bin) {
        return (None, "nothing to delete");
    }
    if guardrails_installed(env) {
        return (None, BIN_KEPT);
    }
    (Some(bin), "delete")
}

/// `remove(plugin_root, cwd, unapprove, unapprove_all, env, preview)`: undo setup. With
/// `preview` nothing is written: it reports the settings diff, the rtk folder, `<ah>/bin` and
/// state.json that would be deleted, and what each approval flag would drop.
pub fn remove(
    plugin_root: &Path,
    cwd: Option<&Path>,
    unapprove: bool,
    unapprove_all: bool,
    env: &Env,
    preview: bool,
    deps: &SetupDeps,
) -> Value {
    let _ = plugin_root; // Python takes it and never reads it.
    result_or_error(try_remove(
        cwd,
        unapprove,
        unapprove_all,
        env,
        preview,
        deps,
    ))
}

fn try_remove(
    cwd: Option<&Path>,
    unapprove: bool,
    unapprove_all: bool,
    env: &Env,
    preview: bool,
    deps: &SetupDeps<'_>,
) -> Result<Value, String> {
    if unsupported(deps) {
        return Ok(obj(vec![
            ("ok", Value::Bool(false)),
            ("unsupported", Value::Bool(true)),
            ("error", s(REFUSAL)),
        ]));
    }
    let cwd = effective_cwd(cwd)?;
    // `_remove_plan(env)`.
    let st = state::read(env);
    let path = settings::settings_path(env);
    let text = read_settings(&path)?;
    let (mut new_text, mut diff) = (text.clone(), String::new());
    if st.get("settings_ops").is_some_and(truthy) {
        // Python's `revert` returns on a blank file and parses the text before `list(ops)`, so
        // a malformed `settings_ops` matters only for a settings file that parses.
        let reverted = if settings::is_blank(&text) {
            Ok((text.clone(), String::new()))
        } else {
            settings::revert_with_diff(&text, &[]).and_then(|_| {
                settings::stored_ops(st.get("settings_ops"))
                    .and_then(|ops| settings::revert_with_diff(&text, &ops))
            })
        };
        match reverted.map_err(classify) {
            Ok((n, d)) => (new_text, diff) = (n, d),
            Err(Refusal::Caught(m)) => {
                return Ok(refused(&format!(
                    "{m}; left untouched. Fix the file and run remove again."
                )));
            }
            Err(Refusal::Raised(m)) => return Err(m),
        }
    }
    let (rtk_real, mut rtk_status, rtk_warn) = rtk_target(env);
    let (history_real, mut history_status, history_warn) = rtk_history_target(env);

    let changed = new_text != text;
    let warnings: Vec<String> = rtk_warn.into_iter().chain(history_warn).collect();
    let approved: Vec<String> = {
        let mut keys: Vec<String> = match memory_approve::load_approved(env).get("repos") {
            Some(Value::Object(repos)) => repos.iter().map(|(k, _)| k.to_string()).collect(),
            _ => Vec::new(),
        };
        keys.sort();
        keys
    };
    let this_repo = repo_info(&cwd, env, deps)
        .map(|(_, key)| key)
        .filter(|key| approved.contains(key));
    let (bin, mut bin_status) = bin_target(env);
    let settings_value = obj(vec![
        ("path", s(&pstr(&path))),
        ("changed", Value::Bool(changed)),
        ("diff", s(&diff)),
    ]);

    if preview {
        let state_path = state::path(env);
        return Ok(obj(vec![
            ("ok", Value::Bool(true)),
            ("preview", Value::Bool(true)),
            ("settings", settings_value),
            (
                "rtk",
                obj(vec![("dir", opt_str(rtk_real)), ("status", s(rtk_status))]),
            ),
            (
                "rtk_history",
                obj(vec![
                    ("dir", s(&pstr(&state::rtk_data_dir(env)))),
                    ("status", s(history_status)),
                ]),
            ),
            (
                "state",
                obj(vec![
                    ("path", s(&pstr(&state_path))),
                    ("exists", Value::Bool(state_path.exists())),
                ]),
            ),
            (
                "memory",
                obj(vec![
                    ("notes", s(NOTES_KEPT)),
                    ("this_repo_approved", opt_str(this_repo.clone())),
                    (
                        "approved_total",
                        Value::Int(json::Int::Small(
                            i64::try_from(approved.len()).unwrap_or(i64::MAX),
                        )),
                    ),
                    (
                        "unapprove",
                        strs(&this_repo.into_iter().collect::<Vec<_>>()),
                    ),
                    ("unapprove_all", strs(&approved)),
                ]),
            ),
            ("warnings", strs(&warnings)),
            (
                "guard_binary",
                obj(vec![
                    ("dir", opt_str(bin.map(|b| pstr(&b)))),
                    ("status", s(bin_status)),
                ]),
            ),
        ]));
    }

    if changed {
        if new_text.is_empty() && st.get("settings_created").is_some_and(truthy) {
            fs::remove_file(&path).map_err(|e| os_err(e, &path))?;
        } else {
            (deps.settings_write)(&path, &new_text).map_err(raised)?;
        }
    }
    // state.json goes first: without its rtk_path, bin/rtk-hook exits before running rtk, so
    // no hook in another session recreates rtk's history while it is being deleted.
    let state_path = state::path(env);
    match fs::remove_file(&state_path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(os_err(e, &state_path)),
        _ => {}
    }
    if let Some(real) = rtk_real {
        rmtree_ignoring_vanished(Path::new(&real))?;
        rtk_status = "deleted";
    }
    if let Some(history) = history_real {
        rmtree_ignoring_vanished(&history)?;
        history_status = "deleted";
    }
    if let Some(bin) = bin {
        // A symlinked `<ah>/bin` loses only the link, never what it points to.
        let removed = match fs::symlink_metadata(&bin) {
            Ok(m) if m.is_dir() => fs::remove_dir_all(&bin),
            _ => fs::remove_file(&bin),
        };
        removed.map_err(|e| os_err(e, &bin))?;
        bin_status = "deleted";
    }
    let mut unapproved = Vec::new();
    if unapprove_all {
        for key in &approved {
            if memory_approve::unapprove(key, env).map_err(raised)? {
                unapproved.push(key.clone());
            }
        }
    } else if unapprove
        && let Some(key) = this_repo
        && memory_approve::unapprove(&key, env).map_err(raised)?
    {
        unapproved.push(key);
    }
    Ok(obj(vec![
        ("ok", Value::Bool(true)),
        ("settings", settings_value),
        ("rtk", obj(vec![("status", s(rtk_status))])),
        ("rtk_history", obj(vec![("status", s(history_status))])),
        (
            "memory",
            obj(vec![
                ("unapproved", strs(&unapproved)),
                ("notes", s(NOTES_KEPT)),
            ]),
        ),
        ("warnings", strs(&warnings)),
        ("guard_binary", obj(vec![("status", s(bin_status))])),
    ]))
}

// --- main -----------------------------------------------------------------------------------

/// Python's `ArgumentParser` (its prog is `anthology setup`; Python's was the script name).
fn parser() -> Spec {
    let common = |spec: Spec| {
        spec.arg(Arg::option(&["--plugin-root"]).required())
            .arg(Arg::option(&["--cwd"]))
            .arg(
                Arg::option(&["--json"])
                    .store_true()
                    .help("accepted for clarity; output is always JSON"),
            )
    };
    let apply = common(Spec::command())
        .arg(Arg::option(&["--expect-sha256"]).required())
        .arg(Arg::option(&["--skip-approve"]).store_true())
        .arg(
            Arg::option(&["--promote"])
                .append()
                .default(argparse::Value::List(Vec::new()))
                .metavar("FILE")
                .help("move this legacy note into the repo's memory (repeatable)"),
        );
    let remove = common(Spec::command())
        .arg(Arg::option(&["--unapprove"]).store_true())
        .arg(Arg::option(&["--unapprove-all"]).store_true())
        .arg(
            Arg::option(&["--preview"])
                .store_true()
                .help("write nothing; show what remove would change"),
        );
    Spec::new("anthology setup")
        .description("/anthology:setup: preview, apply and remove")
        .subcommands(
            Subcommands::new()
                .dest("command")
                .required()
                .parser("preview", None, common(Spec::command()))
                .parser("apply", None, apply)
                .parser("remove", None, remove),
        )
}

/// `main`'s exit code for a result: 2 unsupported, 1 when `ok` is false, else 0 (GC-5).
fn exit_code(result: &Value) -> i32 {
    let m = result.as_object();
    let get = |k: &str| m.and_then(|m| m.get(k));
    if matches!(get("supported"), Some(Value::Bool(false)))
        || get("unsupported").is_some_and(truthy)
    {
        return 2;
    }
    match get("ok") {
        Some(ok) if !truthy(ok) => 1,
        _ => 0,
    }
}

/// Python `main(argv)`: returns (exit code, stdout text exactly as Python prints it, stderr text).
pub fn main(argv: &[String], env: &Env, deps: &SetupDeps) -> (i32, String, String) {
    let args: Vec<OsString> = argv.iter().map(OsString::from).collect();
    let parsed = match argparse::parse(&parser(), &args) {
        Ok(p) => p,
        Err(u) => return (u.code, u.stdout, u.stderr),
    };
    let os = |dest: &str| parsed.os_str(dest).map(PathBuf::from);
    let root = os("plugin_root").unwrap_or_default();
    let cwd = os("cwd");
    let result = match parsed.str("command") {
        Some("preview") => preview(&root, cwd.as_deref(), env, deps),
        Some("apply") => {
            let expect = parsed
                .os_str("expect_sha256")
                .map(|v| v.to_string_lossy().into_owned())
                .unwrap_or_default();
            let promote: Vec<String> = parsed
                .list("promote")
                .iter()
                .filter_map(|v| v.as_os_str().map(|o| o.to_string_lossy().into_owned()))
                .collect();
            apply(
                &root,
                &expect,
                cwd.as_deref(),
                parsed.flag("skip_approve"),
                env,
                &promote,
                deps,
            )
        }
        _ => remove(
            &root,
            cwd.as_deref(),
            parsed.flag("unapprove"),
            parsed.flag("unapprove_all"),
            env,
            parsed.flag("preview"),
            deps,
        ),
    };
    let out = json::dumps(&result, &json::INDENT2) + "\n";
    (exit_code(&result), out, String::new())
}
