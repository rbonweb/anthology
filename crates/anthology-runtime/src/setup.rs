//! `anthology setup preview|apply|remove` (RT4; port of
//! `plugins/anthology/scripts/anthology_setup.py`, GC-10).
//!
//! `preview` writes nothing: the environment checks, duplicate plugins, the rtk step, the
//! settings.json diff (with the sha256 of the text it came from), the memory steps and, new
//! (F-15), the located guardrails folder whose binary apply ensures. `apply` checks
//! `--expect-sha256` three times (before anything, after the downloads, right before the
//! write), records the ops in state.json before writing settings.json, ensures the
//! guardrails-pinned binary (`bootstrap.sh session --plugin guardrails --quiet --budget 60`,
//! D§8.8) and reports `guard_binary: {status}`. The preview's `subagent_model` says where
//! `CLAUDE_CODE_SUBAGENT_MODEL` is set; when that is the user settings.json it shows the
//! separate, opt-in removal, which `apply --remove-subagent-model` makes (recorded, so `remove`
//! puts the old value back). `remove` undoes the settings edits, deletes
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

/// An anthology-settings error outside setup's `except`: `main`'s `"<Class>: <msg>"`.
fn raise_text(msg: String) -> String {
    match classify(msg) {
        Refusal::Raised(m) => m,
        Refusal::Caught(m) => format!("ValueError: {m}"),
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

/// `_settings_plan(root, env, remove_model)`. `remove_model` adds the opt-in removal of
/// `env.CLAUDE_CODE_SUBAGENT_MODEL`, which stays out of the op version. Python's `setup_ops` raised FileNotFoundError when no
/// guardrails folder was located (it loaded the profile roots from it); the roots are compiled
/// in now (F-7), so the located folder is checked here, after the plugin-root validation, as
/// before (session start skips its ops note in that case too).
fn settings_plan(root: &Path, env: &Env, remove_model: bool) -> Result<Plan, Refusal> {
    let path = settings::settings_path(env);
    let text = read_settings(&path).map_err(Refusal::Raised)?;
    let ops = settings::setup_ops(root, env).map_err(classify)?;
    if guardrails_root(root, env).is_none() {
        return Err(Refusal::Caught(GUARD_NOT_FOUND.into()));
    }
    let previous = settings::stored_ops(state::read(env).get("settings_ops")).map_err(classify)?;
    let mut desired = ops.clone();
    if remove_model {
        desired.push(settings::subagent_model_op());
    }
    let (new_text, diff, stored) =
        settings::reconcile(&text, &desired, &previous).map_err(classify)?;
    Ok(Plan {
        path,
        text,
        new_text,
        diff,
        stored,
        ops_version: settings::ops_version(&ops, &SettingsLimits::default()),
    })
}

/// `_subagent_model(env, cwd, new_text)`: where `CLAUDE_CODE_SUBAGENT_MODEL` is set (never its
/// value). `removable` when setup can drop it from the user settings.json it edits (`new_text`
/// is that file after setup's other changes, or `None` when it cannot be edited); `diff` shows
/// that removal, redacted; `elsewhere` gives one line on removing it by hand for every other
/// place.
fn subagent_model(env: &Env, cwd: &Path, new_text: Option<&str>) -> Result<Value, String> {
    let found = compat::subagent_model_sources(env, cwd);
    let (mut removable, mut diff, mut elsewhere) = (false, String::new(), Vec::new());
    for label in &found {
        if *label == compat::USER_SETTINGS
            && let Some(text) = new_text
        {
            let (removed, d) =
                settings::apply(text, &[settings::subagent_model_op()]).map_err(raise_text)?;
            if removed != text {
                (removable, diff) = (true, d);
                continue;
            }
        }
        elsewhere.push(obj(vec![
            ("where", s(label)),
            ("how", s(compat::subagent_model_hint(label))),
        ]));
    }
    Ok(obj(vec![
        ("set_in", Value::Array(found.iter().map(|l| s(l)).collect())),
        ("removable", Value::Bool(removable)),
        ("diff", Value::Str(diff)),
        ("elsewhere", Value::Array(elsewhere)),
    ]))
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
    memory_repo::anthology_home(env).join("personal-memory")
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
    for old in memory_repo::legacy_dirs(root, &memory_repo::config_dir(env), env, &RealGit) {
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
            // Where `--move-old-notes` would put it (the confirmed, by-type move).
            let plan = sync::plan_auto_memory_note(&name, &data, m.is_some(), home, &sd);
            note.insert("destination".into(), s(plan.destination.label()));
            note.insert("target".into(), opt_str(plan.target.clone()));
            note.insert("findings".into(), findings_value(&findings));
            note.insert("movable".into(), Value::Bool(reason.is_none()));
            if let Some(r) = reason.or(plan.reason).filter(|r| !r.is_empty()) {
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
    let ledger = memory_repo::anthology_home(env).join("migrated.json");
    let sd = sync_deps(env, deps);
    let (mut repo, mut pers, mut skipped, mut blocked) = (vec![], vec![], vec![], vec![]);
    for old in memory_repo::legacy_dirs(root, &memory_repo::config_dir(env), env, &RealGit) {
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

/// What apply says about the old auto-memory folders after `--move-old-notes`.
pub const ORIGINALS_KEPT: &str = "the old notes were left in place and never deleted; Claude \
                                  Code's auto memory is off for this repo, so they are no \
                                  longer loaded";

/// `--move-old-notes`, after the user's confirmation: `(migrated, blocked)` as [`migrate_legacy`]
/// reports them, but by type: project, feedback and reference notes go into the repo's memory as
/// `<type>-<slug>.md`, user notes into personal memory. A note the privacy scan refuses, or one
/// without a type, is listed in `blocked` and moved nowhere.
fn move_old_notes(
    root: &Path,
    env: &Env,
    personal: &Path,
    deps: &SetupDeps<'_>,
) -> Result<(Value, Value), String> {
    let m = repo_mem(root);
    let ledger = memory_repo::anthology_home(env).join("migrated.json");
    let sd = sync_deps(env, deps);
    let (mut repo, mut pers, mut skipped, mut blocked) = (vec![], vec![], vec![], vec![]);
    for old in memory_repo::legacy_dirs(root, &memory_repo::config_dir(env), env, &RealGit) {
        let res = sync::try_move_auto_memory(
            &old,
            m.as_deref(),
            personal,
            Some(&ledger),
            None,
            env.get("HOME"),
            &sd,
        )
        .map_err(raised)?;
        for (i, (file, reason)) in res.blocked.iter().enumerate() {
            let findings = res.blocked_findings.get(i).map_or(&[][..], Vec::as_slice);
            blocked.push(obj(vec![
                ("file", s(file)),
                ("findings", findings_value(findings)),
                ("reason", s(reason)),
            ]));
        }
        repo.extend(res.repo);
        pers.extend(res.personal);
        skipped.extend(res.skipped);
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

/// Why `--move-old-notes` cannot be honoured (checked before anything is written).
fn move_notes_error(
    promote: &[String],
    skip_approve: bool,
    cwd: &Path,
    env: &Env,
    deps: &SetupDeps<'_>,
) -> Option<String> {
    if skip_approve {
        return Some(
            "--move-old-notes moves notes into this repo's memory, which needs the repo approved; \
             it cannot be combined with --skip-approve. Nothing was written."
                .into(),
        );
    }
    if !promote.is_empty() {
        return Some(
            "--move-old-notes already chooses where each note goes, by type; it cannot be \
             combined with --promote. Nothing was written."
                .into(),
        );
    }
    if repo_info(cwd, env, deps).is_none() {
        return Some(
            "--move-old-notes needs a git repo (this folder is not one). Nothing was written."
                .into(),
        );
    }
    None
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

// --- the repo's own auto memory switch ------------------------------------------------------

/// The one key setup adds to an approved repo's `.claude/settings.json`: with the repo's memory
/// (`.claude/memory/`) in force, Claude Code's built-in auto memory must not run beside it. The
/// user-wide settings.json is never used for this.
pub const AUTO_MEMORY_KEY: &str = "autoMemoryEnabled";
/// state.json's record of these edits, so `remove` can undo them:
/// `{<repo root>: {"ops": [<settings record>, ...], "created": <bool>}}`.
const REPO_SETTINGS_STATE: &str = "repo_settings";

fn auto_memory_op() -> settings::Op {
    settings::Op::Set {
        path: vec![AUTO_MEMORY_KEY.to_string()],
        value: Value::Bool(false),
        tag: None,
    }
}

fn repo_settings_path(root: &Path) -> PathBuf {
    root.join(".claude").join("settings.json")
}

/// What setup would write into one repo's `.claude/settings.json`.
struct RepoPlan {
    path: PathBuf,
    text: String,
    new_text: String,
    diff: String,
    stored: Vec<Value>,
}

/// The repo edits state.json holds, by repo root.
fn recorded_repo_settings(env: &Env) -> Map {
    state::read(env)
        .get(REPO_SETTINGS_STATE)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// The records of the ops earlier runs made in `root`'s settings.json.
fn recorded_ops(recorded: &Map, root: &Path) -> Result<Vec<Value>, String> {
    let ops = recorded
        .get(&pstr(root))
        .and_then(Value::as_object)
        .and_then(|entry| entry.get("ops"));
    settings::stored_ops(ops).map_err(raise_text)
}

/// Why a repo's settings.json is not touched, or `None` when it may be: `.claude` or the file
/// itself must not be a symlink (a repo could point it anywhere), and the file must be a plain
/// file.
fn repo_settings_unsafe(root: &Path) -> Option<String> {
    let claude = root.join(".claude");
    if unsafe_dir(&claude) {
        return Some(".claude is a symlink or not a folder; left untouched".into());
    }
    let path = repo_settings_path(root);
    if is_symlink(&path) {
        return Some(".claude/settings.json is a symlink; left untouched".into());
    }
    if lexists(&path) && !path.is_file() {
        return Some(".claude/settings.json is not a file; left untouched".into());
    }
    None
}

/// `_settings_plan` for a repo: the key merged into what the file holds. `Err` is the reason the
/// file is left alone (an unreadable or non-JSON file is never overwritten).
fn repo_settings_plan(root: &Path, env: &Env) -> Result<RepoPlan, String> {
    if let Some(why) = repo_settings_unsafe(root) {
        return Err(why);
    }
    let path = repo_settings_path(root);
    let text = settings::read_text(&path)
        .map(Option::unwrap_or_default)
        .map_err(|e| format!("{}; left untouched", exc_text(&e)))?;
    let previous = recorded_ops(&recorded_repo_settings(env), root)?;
    let (new_text, diff, stored) = settings::reconcile(&text, &[auto_memory_op()], &previous)
        .map_err(|m| match classify(m) {
            Refusal::Caught(t) | Refusal::Raised(t) => format!("{t}; left untouched"),
        })?;
    Ok(RepoPlan {
        path,
        text,
        new_text,
        diff,
        stored,
    })
}

fn repo_settings_skipped(root: &Path, why: &str) -> Value {
    obj(vec![
        ("path", s(&pstr(&repo_settings_path(root)))),
        ("skipped", s(why)),
    ])
}

/// The preview's `memory.repo.auto_memory`: what setup would change in the repo's settings.json.
fn repo_settings_preview(root: &Path, env: &Env) -> Value {
    match repo_settings_plan(root, env) {
        Ok(plan) => obj(vec![
            ("path", s(&pstr(&plan.path))),
            ("changed", Value::Bool(plan.new_text != plan.text)),
            ("diff", Value::Str(plan.diff)),
        ]),
        Err(why) => repo_settings_skipped(root, &why),
    }
}

/// Apply's step for an approved repo: record the edit in state.json first (so `remove` can always
/// undo it), then write the merged file.
fn apply_repo_settings(root: &Path, env: &Env, deps: &SetupDeps<'_>) -> Result<Value, String> {
    let plan = match repo_settings_plan(root, env) {
        Ok(p) => p,
        Err(why) => return Ok(repo_settings_skipped(root, &why)),
    };
    let changed = plan.new_text != plan.text;
    if !plan.stored.is_empty() {
        let mut recorded = recorded_repo_settings(env);
        let key = pstr(root);
        let was_created = recorded
            .get(&key)
            .and_then(Value::as_object)
            .and_then(|entry| entry.get("created"))
            .is_some_and(truthy);
        let created = was_created || !lexists(&plan.path);
        recorded.insert(
            key,
            obj(vec![
                ("ops", Value::Array(plan.stored.clone())),
                ("created", Value::Bool(created)),
            ]),
        );
        (deps.state_write)(
            env,
            vec![(REPO_SETTINGS_STATE.into(), Value::Object(recorded))],
        )
        .map_err(raised)?;
    }
    if changed {
        (deps.settings_write)(&plan.path, &plan.new_text).map_err(raised)?;
    }
    Ok(obj(vec![
        ("path", s(&pstr(&plan.path))),
        ("changed", Value::Bool(changed)),
        ("diff", Value::Str(plan.diff)),
    ]))
}

/// What `remove` does to one repo's settings.json.
struct RepoRevert {
    root: String,
    path: PathBuf,
    new_text: String,
    diff: String,
    /// Take the file away: setup created it and nothing else is left in it.
    delete: bool,
    /// Why the file is left alone (or has nothing to revert).
    note: Option<String>,
    changed: bool,
}

/// `remove`'s plan for every repo setup edited, from state.json. A repo whose file is gone has
/// nothing to revert; one that cannot be reverted is left alone and reported, and never stops
/// the rest of the removal.
fn repo_settings_reverts(env: &Env) -> Vec<RepoRevert> {
    let mut out = Vec::new();
    for (root, entry) in recorded_repo_settings(env).iter() {
        let path = repo_settings_path(Path::new(root));
        let mut revert = RepoRevert {
            root: root.to_string(),
            path: path.clone(),
            new_text: String::new(),
            diff: String::new(),
            delete: false,
            note: None,
            changed: false,
        };
        let plan = || -> Result<Option<(String, String, String)>, String> {
            if let Some(why) = repo_settings_unsafe(Path::new(root)) {
                return Err(why);
            }
            let Some(text) = settings::read_text(&path).map_err(|e| exc_text(&e))? else {
                return Ok(None);
            };
            let ops = settings::stored_ops(entry.as_object().and_then(|e| e.get("ops")))
                .map_err(raise_text)?;
            let (new_text, diff) = settings::revert_with_diff(&text, &ops).map_err(raise_text)?;
            Ok(Some((text, new_text, diff)))
        };
        match plan() {
            Ok(None) => revert.note = Some("nothing to revert: the file is gone".into()),
            Ok(Some((text, new_text, diff))) => {
                revert.changed = new_text != text;
                revert.delete = revert.changed
                    && settings::is_blank(&new_text)
                    && entry
                        .as_object()
                        .and_then(|e| e.get("created"))
                        .is_some_and(truthy);
                revert.new_text = new_text;
                revert.diff = diff;
            }
            Err(why) => revert.note = Some(format!("left in place: {why}")),
        }
        out.push(revert);
    }
    out
}

fn repo_reverts_value(reverts: &[RepoRevert]) -> Value {
    Value::Array(
        reverts
            .iter()
            .map(|r| {
                let mut m = vec![
                    ("root", s(&r.root)),
                    ("path", s(&pstr(&r.path))),
                    ("changed", Value::Bool(r.changed)),
                    ("diff", s(&r.diff)),
                ];
                if let Some(n) = &r.note {
                    m.push(("note", s(n)));
                }
                obj(m)
            })
            .collect(),
    )
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
    let mut new_text = None;
    match settings_plan(plugin_root, env, false) {
        Ok(plan) => {
            st.insert("changed".into(), Value::Bool(plan.new_text != text));
            st.insert("diff".into(), Value::Str(plan.diff));
            new_text = Some(plan.new_text);
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
            ("auto_memory", repo_settings_preview(root, env)),
        ]),
    };
    let legacy = info
        .map(|(root, _)| legacy_notes(&root, env, deps))
        .unwrap_or_default();
    let model = subagent_model(env, &cwd, new_text.as_deref())?;
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
        ("subagent_model", model),
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
    apply_with(
        plugin_root,
        expect_sha256,
        cwd,
        skip_approve,
        env,
        promote,
        false,
        deps,
    )
}

/// [`apply`] with `remove_subagent_model` (only when the user said yes): also drop
/// `env.CLAUDE_CODE_SUBAGENT_MODEL` from settings.json, recorded so `remove` restores it.
#[allow(clippy::too_many_arguments)] // Python's keyword arguments
pub fn apply_with(
    plugin_root: &Path,
    expect_sha256: &str,
    cwd: Option<&Path>,
    skip_approve: bool,
    env: &Env,
    promote: &[String],
    remove_subagent_model: bool,
    deps: &SetupDeps,
) -> Value {
    apply_full(
        plugin_root,
        expect_sha256,
        cwd,
        skip_approve,
        env,
        promote,
        remove_subagent_model,
        false,
        deps,
    )
}

/// [`apply_with`] with `move_old_notes` (only when the user said yes): move the notes Claude
/// Code's auto memory saved for this repo by type, instead of copying them all into personal
/// memory. It cannot be combined with `promote` or `skip_approve`.
#[allow(clippy::too_many_arguments)] // Python's keyword arguments
pub fn apply_full(
    plugin_root: &Path,
    expect_sha256: &str,
    cwd: Option<&Path>,
    skip_approve: bool,
    env: &Env,
    promote: &[String],
    remove_subagent_model: bool,
    move_old_notes: bool,
    deps: &SetupDeps,
) -> Value {
    result_or_error(try_apply(
        plugin_root,
        expect_sha256,
        cwd,
        skip_approve,
        env,
        promote,
        remove_subagent_model,
        move_old_notes,
        deps,
    ))
}

#[allow(clippy::too_many_arguments)] // Python's keyword arguments
fn try_apply(
    root: &Path,
    expect: &str,
    cwd: Option<&Path>,
    skip_approve: bool,
    env: &Env,
    promote: &[String],
    remove_model: bool,
    move_notes: bool,
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
    if move_notes && let Some(r) = move_notes_error(promote, skip_approve, &cwd, env, deps) {
        return Ok(refused(&r));
    }
    if let Some(r) = promote_error(promote, skip_approve, &cwd, env, deps) {
        return Ok(refused(&r));
    }
    let mut warnings: Vec<String> = Vec::new();
    let path = settings::settings_path(env);
    // Check 1 of 3: nothing is downloaded for a stale preview or a bad plan.
    if sha256(&read_settings(&path)?) != expect {
        return Ok(refused(CHANGED_SINCE_PREVIEW));
    }
    match settings_plan(root, env, remove_model) {
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
    let plan = match settings_plan(root, env, remove_model) {
        Ok(p) => p,
        Err(Refusal::Caught(m)) => return Ok(refused(&m)),
        Err(Refusal::Raised(m)) => return Err(m),
    };
    let model_removed = remove_model
        && !settings::plan(&plan.text, &[settings::subagent_model_op()])
            .map_err(raise_text)?
            .is_empty();
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
    let mut auto_memory = Value::Null;
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
        // Claude Code's own auto memory is switched off only where the repo's memory works.
        auto_memory = if approve_error.is_null() {
            apply_repo_settings(&repo_root, env, deps)?
        } else {
            repo_settings_skipped(
                &repo_root,
                "repo memory could not be set up (see approve_error), so Claude Code's auto \
                 memory stays on",
            )
        };
        (migrated, blocked) = if move_notes {
            move_old_notes(&repo_root, env, &personal, deps)?
        } else {
            migrate_legacy(&repo_root, env, &personal, promote, deps)?
        };
    }
    let memory = obj(vec![
        ("approved_key", approved_key),
        ("approve_error", approve_error),
        ("auto_memory", auto_memory),
        ("migrated", migrated),
        ("blocked", blocked),
        (
            "originals",
            if move_notes {
                s(ORIGINALS_KEPT)
            } else {
                Value::Null
            },
        ),
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
        (
            "subagent_model",
            obj(vec![("removed", Value::Bool(model_removed))]),
        ),
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
    let repo_reverts = repo_settings_reverts(env);
    let repo_warnings = repo_reverts.iter().filter_map(|r| {
        r.note
            .as_ref()
            .filter(|n| n.starts_with("left in place"))
            .map(|n| format!("{}: {n}", pstr(&r.path)))
    });
    let warnings: Vec<String> = rtk_warn
        .into_iter()
        .chain(history_warn)
        .chain(repo_warnings)
        .collect();
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
                    ("repo_settings", repo_reverts_value(&repo_reverts)),
                    ("this_repo_approved", opt_str(this_repo.clone())),
                    (
                        "approved_total",
                        Value::Int(json::Int::Small(
                            i64::try_from(approved.len()).unwrap_or(i64::MAX),
                        )),
                    ),
                    (
                        "unapprove",
                        // `[this_repo] if this_repo else []`: an empty key is falsy
                        strs(
                            &this_repo
                                .iter()
                                .filter(|k| !k.is_empty())
                                .cloned()
                                .collect::<Vec<_>>(),
                        ),
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
    for r in repo_reverts.iter().filter(|r| r.changed) {
        if r.delete {
            fs::remove_file(&r.path).map_err(|e| os_err(e, &r.path))?;
        } else {
            (deps.settings_write)(&r.path, &r.new_text).map_err(raised)?;
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
        && let Some(key) = this_repo.filter(|k| !k.is_empty())
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
                ("repo_settings", repo_reverts_value(&repo_reverts)),
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
        )
        .arg(
            Arg::option(&["--remove-subagent-model"])
                .store_true()
                .help("also remove env.CLAUDE_CODE_SUBAGENT_MODEL from settings.json"),
        )
        .arg(Arg::option(&["--move-old-notes"]).store_true().help(
            "move the notes Claude Code's auto memory saved for this repo by type: \
                 project, feedback and reference notes into the repo's memory, user notes \
                 into personal memory",
        ));
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
            apply_full(
                &root,
                &expect,
                cwd.as_deref(),
                parsed.flag("skip_approve"),
                env,
                &promote,
                parsed.flag("remove_subagent_model"),
                parsed.flag("move_old_notes"),
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
