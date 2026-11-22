//! The SessionStart hook (RT3; port of `plugins/anthology/scripts/session_start.py`, GC-7).
//!
//! [`run`] returns the hook's stdout JSON (one line, without the newline) and never panics:
//!
//! ```text
//! {"hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": <guide + memory>},
//!  "systemMessage": <warnings and memory messages, one per line>}
//! ```
//!
//! `additionalContext` stays within [`CONTEXT_BUDGET`] code points: the guide is never cut;
//! memory text is cut at a line boundary and ends with a pointer to `anthology-mem search`.
//! Every step is isolated: a failing step becomes `anthology: <label> failed (<Class>)` and
//! never costs the guide.
//!
//! Order (GC-7): Windows → refusal only; read the hook input; ensure the live guard binary
//! (new step); the guardrails self-test (F-4); state; the NAG, or the read-rule, ops-version and
//! rtk checks (the rtk check skipped when more than 10 s passed since `ANTHOLOGY_HOOK_T0`,
//! F-20); the data-folder, wrapper, agent-memory and subagent checks; the git-folder check (a
//! session folder that is not a repo but holds repos); memory sync; the guide; compose; prune
//! `<ah>/bin` (design §8.5).
//!
//! **Subagents.** When the hook input has a non-empty `agent_id` (a SessionStart inside a
//! subagent that compacted), only memory sync runs, and the output has no guide and no
//! `systemMessage` ([`subagent_output`]). Everything else is for the lead.
//!
//! **Time (I-7, GC-7).** Every elapsed check counts from `ANTHOLOGY_HOOK_T0` (epoch seconds,
//! set by `hooks/session-start.sh`). The live guard's binary is ensured only when
//! `ANTHOLOGY_BOOTSTRAP_DOWNLOADED` is exactly `0` and
//! `min(BOOTSTRAP_BUDGET, DEADLINE − elapsed − GUARD_TIMEOUT − 5) ≥ BOOTSTRAP_BUDGET`, with one
//! `bootstrap.sh session --plugin guardrails --quiet --budget 20` run; otherwise it is left to
//! the guardrails plugin's own SessionStart hook, the guard shim or the next session.
//!
//! **Self-test (F-4, m-R14).** `/bin/sh <guard>/hooks/guard.sh` gets the compiled-in sample on
//! stdin with `ANTHOLOGY_GUARD_NO_FETCH=1`. It passes only on exit 2 with stderr starting
//! `Blocked by anthology-guardrails (G1)`. A `(setup)` block (the shim failing closed) is
//! reported with the guard's own reason.
//!
//! Seams (GC-19): [`SessionDeps`] and [`SessionLimits`]. Python's `mock.patch` of a step
//! function with a `side_effect` maps to [`SessionDeps::fail`], keyed by the Python name.
#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anthology_core::lock::{self, ANTHOLOGY_TAG_PREFIX, Lock};
use anthology_core::{Env, paths, platform, state};
use anthology_guard::data;
use anthology_memory::sync::{self, SyncDeps};
use anthology_pycompat::errno_tables;
use anthology_pycompat::json::{self, Map, Value};
use anthology_pycompat::pyerr::PyExc;
use anthology_settings::{self as settings, SettingsDeps, SettingsLimits, TAG_PLUGIN_READ};

use crate::bootstrap::{self, GuardBinaryStatus};
use crate::compat;
use crate::guardrails_loc::{self, GUARD_REL, Problem};
use crate::rtk_install::{self, Reconciled, RtkDeps, RtkLimits};

/// Windows (the only output there).
pub const REFUSAL: &str = "Anthology supports macOS and Linux only; it is inactive on Windows.";
/// Setup never ran.
pub const NAG: &str =
    "anthology: run /anthology:setup to finish installing (rtk, permissions, memory)";
/// No `CLAUDE_PLUGIN_DATA`.
pub const NO_DATA: &str = "anthology: Claude Code did not pass CLAUDE_PLUGIN_DATA to the hook; rtk and the memory search index have no data folder (compatibility check)";
/// `CLAUDE_CODE_SUBAGENT_MODEL` is set somewhere.
pub const SUBAGENT_MODEL_SET: &str = "anthology: CLAUDE_CODE_SUBAGENT_MODEL is set, so every agent runs on that model and the roster's model tiers do not apply; run /anthology:setup to see where it is set";
/// The plugin-read rule names another folder.
pub const MOVED: &str =
    "anthology: the plugin moved; re-run /anthology:setup so agents can read their knowledge";
/// Setup's settings ops changed since setup last ran.
pub const OPS_CHANGED: &str =
    "anthology settings changed in this release — run /anthology:setup to review";
/// The rtk check was skipped (F-20).
pub const RTK_SKIPPED: &str =
    "anthology: skipped the rtk check this session (not enough time left)";
/// No rtk.lock in the plugin.
pub const RTK_NOT_WIRED: &str = "anthology: rtk wiring not present yet; skipped";
/// The session folder is not in a git repo but git repos sit directly below it.
pub const NOT_A_REPO: &str = "anthology: this folder is not a git repo, but git repos sit below it. Anthology's agents need worktrees, so start Claude Code inside the repo you are working on";
/// The most entries of the session folder looked at for [`NOT_A_REPO`].
pub const REPO_SCAN_CAP: usize = 200;
/// The guide file is missing or unreadable.
pub const GUIDE_MISSING: &str =
    "anthology: the routing guide is missing from the plugin (templates/routing-guide.md)";
/// The guardrails plugin is disabled in a settings file.
pub const GUARD_DISABLED: &str = "anthology: anthology-guardrails is disabled in your settings, so the guardrails are OFF; enable it (/plugin)";
/// anthology is installed without guardrails.
pub const GUARD_NOT_INSTALLED: &str = "anthology: anthology-guardrails is not installed, so the browser and secret guardrails are NOT active; install it (/plugin)";
/// The live guardrails folder has no guard (F-6).
pub const GUARD_LIVE_MISSING: &str = "anthology: the anthology-guardrails folder this session runs has no hooks/guard.sh \u{2014} the guardrails may be OFF (unchecked); reinstall or repair that folder";
/// Nothing found.
pub const GUARD_NOT_FOUND: &str = "anthology: the guardrails plugin was not found, so the browser and secret guardrails are NOT active; reinstall anthology-guardrails";
/// The self-test timed out.
pub const GUARD_TIMED_OUT: &str =
    "anthology: the guardrails self-test timed out; the guardrails may be OFF";
/// The pass prefix of the self-test (F-4).
pub const SELF_TEST_PASS: &[u8] = b"Blocked by anthology-guardrails (G1)";
/// The shim's fail-closed block (GC-14, m-R14).
pub const SETUP_BLOCK: &str = "Blocked by anthology-guardrails (setup): ";
/// The most characters of a fail-closed reason shown.
pub const FAILING_CLOSED_MAX: usize = 300;

/// Seconds for the guard self-test (GC-7).
pub const GUARD_TIMEOUT: u64 = 10;
/// rtk_install's per-read timeout at session start (GC-9: T = 8).
pub const RTK_TIMEOUT: f64 = 8.0;
/// Seconds of work before the 60 s hook timeout.
pub const DEADLINE: f64 = 50.0;
/// The rtk check is skipped when less than this is left of [`DEADLINE`].
pub const RTK_MIN_REMAINING: f64 = 40.0;
/// Code points of `additionalContext`.
pub const CONTEXT_BUDGET: usize = 9000;
/// Code points of `systemMessage`.
pub const MESSAGE_CAP: usize = 2000;
/// `BOOTSTRAP_BUDGET` (GC-7): seconds per download.
pub const BOOTSTRAP_BUDGET: u64 = 20;
/// The slack the budget formula keeps for the self-test's process start and the rest.
pub const ENSURE_SLACK: f64 = 5.0;
/// A hook process never lives longer than this; a T0 further back is not this hook's.
pub const HOOK_TIMEOUT: f64 = 60.0;
/// Version folders unused this long are pruned (design §8.5).
pub const PRUNE_VERSION_SECS: f64 = 30.0 * 86400.0;
/// `.tmp-*` and `.broken-*` older than this are pruned (GC-17).
pub const PRUNE_TEMP_SECS: f64 = 86400.0;
/// Most bytes kept of the self-test's stdout and stderr each.
const CAPTURE_MAX: usize = 1 << 20;

/// The constants tests patch (GC-19).
#[derive(Clone, Copy, Debug)]
pub struct SessionLimits {
    /// `GUARD_TIMEOUT`.
    pub guard_timeout: Duration,
}

impl Default for SessionLimits {
    fn default() -> SessionLimits {
        SessionLimits {
            guard_timeout: Duration::from_secs(GUARD_TIMEOUT),
        }
    }
}

/// A child process to run with a timeout (the self-test).
pub struct RunSpec<'a> {
    pub argv: Vec<PathBuf>,
    pub stdin: &'a [u8],
    pub env: &'a Env,
    pub timeout: Duration,
}

/// What [`RunSpec`] gave (`subprocess.run`'s outcomes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunResult {
    /// The exit code (`-N` for a signal, as Python's `returncode`).
    Exited {
        code: i32,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
    /// `TimeoutExpired`: the process group was killed.
    TimedOut,
    /// `OSError` starting it.
    Failed(PyExc),
}

/// The rtk interface session start calls (Python's `rtk_install` module).
pub trait RtkWiring {
    /// `rtk_install.load_lock(path)`.
    fn load_lock(&self, path: &Path) -> Result<Lock, PyExc>;
    /// `rtk_install.reconcile(data_dir, lock, None, timeout)`.
    fn reconcile(
        &self,
        env: &Env,
        data_dir: &Path,
        lock: &Lock,
        timeout: f64,
    ) -> Result<Reconciled, PyExc>;
}

/// The real [`RtkWiring`]: RT2's `rtk_install` with the production fetch.
pub struct RealRtk;

impl RtkWiring for RealRtk {
    fn load_lock(&self, path: &Path) -> Result<Lock, PyExc> {
        rtk_install::load_lock(path)
    }

    fn reconcile(
        &self,
        env: &Env,
        data_dir: &Path,
        lock: &Lock,
        timeout: f64,
    ) -> Result<Reconciled, PyExc> {
        let limits = RtkLimits::default();
        let fetch = rtk_install::default_fetch(env, &limits);
        let deps = RtkDeps::new(&fetch);
        Ok(rtk_install::reconcile(env, data_dir, lock, timeout, &deps))
    }
}

/// compat's checks (Python's `compat` module; `None` in [`SessionDeps::compat`] is a broken
/// import, so these three steps fail with `AttributeError`).
#[derive(Clone, Copy)]
pub struct CompatDeps {
    pub wrapper_problems: fn(&Path) -> Vec<String>,
    pub agent_memory_problems: fn(&Path) -> (Vec<String>, Vec<String>),
    pub repo_top: fn(&Path) -> Option<PathBuf>,
    pub agent_memory_hint: fn(&str) -> String,
    pub subagent_model_source: fn(&Env, &Path) -> Option<String>,
}

impl Default for CompatDeps {
    fn default() -> CompatDeps {
        CompatDeps {
            wrapper_problems: compat::wrapper_problems,
            agent_memory_problems: compat::agent_memory_problems,
            repo_top: compat::repo_top,
            agent_memory_hint: compat::agent_memory_hint,
            subagent_model_source: compat::subagent_model_source,
        }
    }
}

/// Memory sync: `(context text, messages)` (Python's `memory_sync.run(cwd, env)`).
pub type MemoryRun<'a> = &'a dyn Fn(&Path, &Env) -> Result<(String, Vec<String>), PyExc>;
/// `_guard_selftest(env, root, cwd)`: a warning or `None`.
pub type SelfTest<'a> = &'a dyn Fn(&Env, &Path, &Path) -> Result<Option<String>, PyExc>;
/// `state.write(**kv)`.
pub type StateWrite<'a> = &'a dyn Fn(&Env, Vec<(String, Value)>) -> Result<(), PyExc>;

/// The seams of session start (GC-19). `SessionDeps::default()` is production.
pub struct SessionDeps<'a> {
    /// `(platform.system(), platform.machine())`.
    pub uname: fn() -> (String, String),
    /// `_plugin_root(env)`: `CLAUDE_PLUGIN_ROOT`.
    pub plugin_root: &'a dyn Fn(&Env) -> Result<PathBuf, PyExc>,
    /// `os.getcwd()`, for a hook input without `cwd`.
    pub cwd: &'a dyn Fn() -> io::Result<PathBuf>,
    /// Seconds since `ANTHOLOGY_HOOK_T0` (`_elapsed`); `None` is the real clock.
    pub elapsed: Option<&'a dyn Fn() -> f64>,
    /// `_guard_selftest(env, root, cwd)`; `None` is the real one.
    pub self_test: Option<SelfTest<'a>>,
    /// The global `subprocess.run` (the self-test's child).
    pub run: &'a dyn Fn(&RunSpec<'_>) -> RunResult,
    /// `state.write(**kv)`.
    pub state_write: StateWrite<'a>,
    /// `settings_patch.setup_ops`.
    pub settings: SettingsDeps,
    /// `rtk_install`; `None` is a missing module (`rtk wiring not present yet`).
    pub rtk: Option<&'a dyn RtkWiring>,
    /// `pathlib.Path.exists`.
    pub path_exists: fn(&Path) -> bool,
    /// `compat`; `None` is a broken import.
    pub compat: Option<CompatDeps>,
    /// `memory_sync.run`.
    pub memory_run: MemoryRun<'a>,
    /// `ensure_live_guard_binary(guard_root, budget, env)` (RT2).
    pub ensure_guard_binary: &'a dyn Fn(&Path, u64, &Env) -> GuardBinaryStatus,
    /// `time.time()`, for the prune.
    pub now: fn() -> f64,
    /// A step whose Python function a test patched with `side_effect=<exc>`: given the Python
    /// name (`_setup_warning`, `_read_guide`, …), the exception it raises, if any.
    pub fail: &'a dyn Fn(&str) -> Option<PyExc>,
    pub limits: SessionLimits,
}

fn env_plugin_root(env: &Env) -> Result<PathBuf, PyExc> {
    match env.get_os("CLAUDE_PLUGIN_ROOT") {
        Some(v) if !v.is_empty() => Ok(PathBuf::from(v)),
        // The binary does not live in the plugin, so there is no folder to fall back to.
        _ => Err(PyExc::Other(
            "CLAUDE_PLUGIN_ROOT is not set",
            "CLAUDE_PLUGIN_ROOT is not set".into(),
        )),
    }
}

fn no_fault(_: &str) -> Option<PyExc> {
    None
}

/// `memory_sync.run(cwd, env)`: the hook's env reaches the privacy scan's `HOME` fallback, as
/// Python's `os.environ["HOME"]` does for the hook's own environment. Never fails: an error
/// becomes a message inside [`sync::run`].
fn memory_run_default(cwd: &Path, env: &Env) -> Result<(String, Vec<String>), PyExc> {
    Ok(sync::run(cwd, env, &SyncDeps::from_env(env)))
}

fn path_exists(p: &Path) -> bool {
    p.exists()
}

fn now_epoch() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

impl Default for SessionDeps<'static> {
    fn default() -> SessionDeps<'static> {
        SessionDeps {
            uname: platform::uname,
            plugin_root: &env_plugin_root,
            cwd: &std::env::current_dir,
            elapsed: None,
            self_test: None,
            run: &run_with_timeout,
            state_write: &state::write,
            settings: SettingsDeps::default(),
            rtk: Some(&RealRtk),
            path_exists,
            compat: Some(CompatDeps::default()),
            memory_run: &memory_run_default,
            ensure_guard_binary: &bootstrap::ensure_live_guard_binary,
            now: now_epoch,
            fail: &no_fault,
            limits: SessionLimits::default(),
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

/// `type(v).__name__` of a JSON value.
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

/// `s[:n]` in code points.
fn cut(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// `str.rstrip()` (Python whitespace).
fn py_rstrip(s: &str) -> &str {
    s.trim_end_matches(anthology_pycompat::str::is_space)
}

/// `(str(exc) or type(exc).__name__)`.
fn exc_text(exc: &PyExc) -> String {
    let s = exc.py_str();
    if s.is_empty() {
        exc.type_name().to_string()
    } else {
        s
    }
}

/// `exc.strerror` of an `OSError`; `None` (Python prints `None`) for anything else.
fn strerror(exc: &PyExc) -> String {
    match exc {
        PyExc::OSError { errno, .. } => errno_tables::strerror(*errno),
        _ => "None".to_string(),
    }
}

/// `str(path)` as pathlib prints it.
fn path_str(p: &Path) -> String {
    String::from_utf8_lossy(&compat::pathlib_str(p.as_os_str().as_bytes())).into_owned()
}

// --- steps ----------------------------------------------------------------------------------

/// The step labels (GC-7), in order.
pub mod label {
    pub const HOOK_INPUT: &str = "reading the hook input";
    pub const ENSURE: &str = "ensuring the guardrails binary";
    pub const SELF_TEST: &str = "the guardrails self-test";
    pub const STATE: &str = "reading state.json";
    pub const DATA_DIR: &str = "recording the plugin data folder";
    pub const READ_RULE: &str = "the plugin-read permission check";
    pub const OPS: &str = "the settings version check";
    pub const RTK: &str = "the rtk check";
    pub const DATA_CHECK: &str = "the plugin data folder check";
    pub const WRAPPERS: &str = "the bin wrapper check";
    pub const AGENT_MEMORY: &str = "the agent-memory path check";
    pub const SUBAGENT: &str = "the subagent model check";
    pub const GIT_FOLDER: &str = "the git folder check";
    pub const MEMORY: &str = "memory sync";
    pub const GUIDE: &str = "reading the routing guide";
    pub const COMPOSE: &str = "assembling the session context";
}

/// `_step`: runs `f`; an exception (or a panic) becomes `anthology: <label> failed (<Class>)`.
fn step<T>(
    warnings: &mut Vec<String>,
    label: &str,
    f: impl FnOnce(&mut Vec<String>) -> Result<T, PyExc>,
    default: T,
) -> T {
    let mut inner = Vec::new();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&mut inner)));
    warnings.append(&mut inner);
    match result {
        Ok(Ok(v)) => v,
        Ok(Err(exc)) => {
            warnings.push(format!("anthology: {label} failed ({})", exc.type_name()));
            default
        }
        Err(_) => {
            warnings.push(format!("anthology: {label} failed (panic)"));
            default
        }
    }
}

fn fault(deps: &SessionDeps<'_>, name: &str) -> Result<(), PyExc> {
    match (deps.fail)(name) {
        Some(exc) => Err(exc),
        None => Ok(()),
    }
}

fn attribute_error() -> PyExc {
    PyExc::Other(
        "AttributeError",
        "'NoneType' object has no attribute".into(),
    )
}

/// `_parse_stdin`: the hook input object, or `{}`.
fn parse_stdin(stdin: &str) -> Map {
    if anthology_pycompat::str::strip_ws(stdin).is_empty() {
        return Map::new();
    }
    json::loads(stdin)
        .ok()
        .and_then(Value::into_object)
        .unwrap_or_default()
}

/// `_is_subagent`: Claude Code puts `agent_id` in the hook input when the hook fires inside a
/// subagent or teammate (a SessionStart after it compacts). `agent_type` is not used: it is
/// also set for a main session started with `--agent`.
pub fn is_subagent(hook: &Map) -> bool {
    matches!(hook.get("agent_id").and_then(Value::as_str), Some(id) if !id.is_empty())
}

/// The real `_guard_selftest` (module docs, F-4).
pub fn guard_selftest(
    env: &Env,
    root: &Path,
    cwd: &Path,
    deps: &SessionDeps<'_>,
) -> Result<Option<String>, PyExc> {
    let loc = guardrails_loc::inspect(root, env, cwd);
    match loc.problem {
        Some(Problem::Disabled) => return Ok(Some(GUARD_DISABLED.into())),
        Some(Problem::NotInstalled) => return Ok(Some(GUARD_NOT_INSTALLED.into())),
        Some(Problem::LiveMissing) => return Ok(Some(GUARD_LIVE_MISSING.into())),
        _ => {}
    }
    let Some(base) = loc.root else {
        return Ok(Some(GUARD_NOT_FOUND.into()));
    };
    let sample = json::dumps(&data::self_test_sample(), &json::PY_DEFAULT);
    let child_env = env.clone().with("ANTHOLOGY_GUARD_NO_FETCH", "1");
    let spec = RunSpec {
        argv: vec![PathBuf::from("/bin/sh"), base.join(GUARD_REL)],
        stdin: sample.as_bytes(),
        env: &child_env,
        timeout: deps.limits.guard_timeout,
    };
    Ok(match (deps.run)(&spec) {
        RunResult::TimedOut => Some(GUARD_TIMED_OUT.into()),
        RunResult::Failed(exc) => Some(format!(
            "anthology: the guardrails self-test could not run ({}); the guardrails may be OFF",
            strerror(&exc)
        )),
        RunResult::Exited { code, stderr, .. } => self_test_verdict(code, &stderr),
    })
}

/// The F-4 verdict on the guard's exit code and stderr.
pub fn self_test_verdict(code: i32, stderr: &[u8]) -> Option<String> {
    if code == 2 && stderr.starts_with(SELF_TEST_PASS) {
        return None;
    }
    if code == 2 && stderr.starts_with(SETUP_BLOCK.as_bytes()) {
        let rest = String::from_utf8_lossy(&stderr[SETUP_BLOCK.len()..]).into_owned();
        let line = rest.split('\n').next().unwrap_or("").trim_end_matches('\r');
        let reason = if char_len(line) > FAILING_CLOSED_MAX {
            format!("{}…", py_rstrip(cut(line, FAILING_CLOSED_MAX - 1)))
        } else {
            line.to_string()
        };
        return Some(format!(
            "anthology: the guardrails are failing closed: {reason}"
        ));
    }
    Some(format!(
        "anthology: the guardrails self-test did NOT block a known-bad command (exit {code}); the guardrails may be OFF"
    ))
}

/// `_setup_warning`: the plugin-read rule check.
fn setup_warning(env: &Env, root: &Path) -> Option<String> {
    match settings::plugin_rule_status(env, root) {
        "missing" => Some(MOVED.into()),
        "unreadable" => Some(
            "anthology: could not read settings.json to check the plugin-read permission; fix the \
             file, then re-run /anthology:setup if agents cannot read their knowledge"
                .into(),
        ),
        _ => None,
    }
}

/// `_ops_warning`: setup's op set changed since setup ran and is not all in settings.json.
/// Python's `setup_ops` loaded the profile roots from the located guardrails plugin and raised
/// `FileNotFoundError` when there was none, which skipped the note; the roots are compiled in
/// now (F-7), so the located folder is checked here, keeping that rule (setup refuses then).
/// As Python, the folder is looked up from the process cwd (`os.getcwd()`), not the hook's.
fn ops_warning(env: &Env, root: &Path, current: &Map, deps: &SessionDeps<'_>) -> Option<String> {
    let cwd = (deps.cwd)().ok()?;
    guardrails_loc::inspect(root, env, &cwd).root?;
    let attempt = || -> Result<bool, ()> {
        let ops = (deps.settings.setup_ops)(root, env).map_err(drop)?;
        let version = settings::ops_version(&ops, &SettingsLimits::default());
        if current.get("settings_ops_version").and_then(Value::as_str) == Some(version.as_str()) {
            return Ok(false);
        }
        let ops: Vec<_> = ops
            .into_iter()
            .filter(|op| op.tag() != Some(TAG_PLUGIN_READ))
            .collect();
        let text = settings::read_text(&settings::settings_path(env)).map_err(drop)?;
        let pending = settings::plan(text.as_deref().unwrap_or(""), &ops).map_err(drop)?;
        Ok(!pending.is_empty())
    };
    match attempt() {
        Ok(true) => Some(OPS_CHANGED.into()),
        _ => None,
    }
}

/// `_reconcile_rtk(root, data_dir)`: a warning or `None`.
pub fn reconcile_rtk(
    env: &Env,
    root: &Path,
    data_dir: Option<&Value>,
    deps: &SessionDeps<'_>,
) -> Result<Option<String>, PyExc> {
    fault(deps, "_reconcile_rtk")?;
    let Some(rtk) = deps.rtk else {
        return Ok(Some(RTK_NOT_WIRED.into()));
    };
    let Some(data_dir) = data_dir.filter(|v| truthy(v)) else {
        return Ok(None);
    };
    let lock_path = root.join("rtk.lock");
    let lock = match rtk.load_lock(&lock_path) {
        Ok(lock) => lock,
        Err(exc) => {
            if !(deps.path_exists)(&lock_path) {
                return Ok(Some(RTK_NOT_WIRED.into()));
            }
            return Ok(Some(format!(
                "anthology: rtk.lock is not valid ({}); rtk skipped",
                cut(&exc_text(&exc), 150)
            )));
        }
    };
    let Some(dir) = data_dir.as_str() else {
        // `Path(data_dir)` raises inside the try around reconcile (CPython 3.12's text).
        return Ok(Some(format!(
            "anthology: rtk setup failed: {}",
            cut(
                &format!(
                    "argument should be a str or an os.PathLike object where __fspath__ returns \
                     a str, not '{}'",
                    py_type(data_dir)
                ),
                150
            )
        )));
    };
    match rtk.reconcile(env, Path::new(dir), &lock, RTK_TIMEOUT) {
        Ok(result) => Ok(result.warning.filter(|w| !w.is_empty())),
        Err(exc) => Ok(Some(format!(
            "anthology: rtk setup failed: {}",
            cut(&exc_text(&exc), 150)
        ))),
    }
}

/// `_read_guide(root)`: the guide as `Path.read_text` gives it (universal newlines), with one
/// final newline; `None` when it cannot be read or is not UTF-8.
fn read_guide(root: &Path) -> Option<String> {
    let raw = fs::read(root.join("templates").join("routing-guide.md")).ok()?;
    let text = String::from_utf8(raw).ok()?;
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    Some(format!("{}\n", text.trim_end_matches('\n')))
}

/// `_compose(guide, memory, home)`: the guide, then memory cut to fit [`CONTEXT_BUDGET`].
pub fn compose(guide: &str, memory: &str, home: &Path) -> String {
    let memory = memory.trim_end_matches('\n');
    if memory.is_empty() {
        return guide.trim_end_matches('\n').to_string();
    }
    let head = if guide.is_empty() {
        String::new()
    } else {
        format!("{guide}\n")
    };
    let room = CONTEXT_BUDGET as i64 - char_len(&head) as i64;
    if char_len(memory) as i64 <= room {
        return head + memory;
    }
    let pointer = format!(
        "(memory index truncated to fit the session budget; use `anthology-mem search <query>`; \
         full indexes: {}, and .claude/memory/MEMORY.md in the repo)",
        path_str(&home.join("personal-memory").join("MEMORY.md"))
    );
    let allowed = room - char_len(&pointer) as i64 - 1;
    if allowed <= 0 {
        return head.trim_end_matches('\n').to_string();
    }
    let piece = cut(memory, allowed as usize + 1);
    let piece = match piece.rfind('\n') {
        Some(i) => &piece[..i],
        None => "",
    };
    if piece.is_empty() {
        head + &pointer
    } else {
        format!("{head}{piece}\n{pointer}")
    }
}

/// The real elapsed clock: seconds since `ANTHOLOGY_HOOK_T0`, or since this run started when T0
/// is missing, not a number, in the future, or older than a hook can live.
struct Clock {
    started: Instant,
    t0: Option<f64>,
}

impl Clock {
    fn new(env: &Env, now: f64) -> Clock {
        let t0 = env
            .get("ANTHOLOGY_HOOK_T0")
            .filter(|s| !s.is_empty() && s.len() <= 12 && s.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|t0| {
                let back = now - t0;
                // `date +%s` is whole seconds, so T0 may read up to 1 s ahead of `now`.
                (-1.0..=HOOK_TIMEOUT).contains(&back)
            });
        Clock {
            started: Instant::now(),
            t0: t0.map(|t0| t0 - now),
        }
    }

    fn elapsed(&self) -> f64 {
        let own = self.started.elapsed().as_secs_f64();
        match self.t0 {
            // `t0` holds T0 − now at the start; elapsed = own − (T0 − start).
            Some(offset) => (own - offset).max(own),
            None => own,
        }
    }
}

/// The GC-7 budget for ensuring the guard binary now, or `None` when it must wait.
pub fn ensure_budget(env: &Env, elapsed: f64, guard_timeout: Duration) -> Option<u64> {
    if env.get("ANTHOLOGY_BOOTSTRAP_DOWNLOADED") != Some("0") {
        return None;
    }
    let left = DEADLINE - elapsed - guard_timeout.as_secs_f64() - ENSURE_SLACK;
    let budget = left.min(BOOTSTRAP_BUDGET as f64);
    if budget >= BOOTSTRAP_BUDGET as f64 {
        Some(budget as u64)
    } else {
        None
    }
}

/// `run(env, stdin)`: the hook's stdout JSON (without the newline). Never panics.
pub fn run(env: &Env, stdin: &str, deps: &SessionDeps<'_>) -> String {
    let attempt =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_inner(env, stdin, deps)));
    let why = match attempt {
        Ok(Ok(out)) => return out,
        Ok(Err(exc)) => exc.type_name().to_string(),
        Err(_) => "panic".to_string(),
    };
    system_message_json(&format!(
        "anthology: session start did not complete ({why}); the routing guide may be missing"
    ))
}

/// `json.dumps({"systemMessage": msg})`.
pub fn system_message_json(msg: &str) -> String {
    let mut out = Map::new();
    out.insert("systemMessage".into(), Value::Str(msg.into()));
    json::dumps(&Value::Object(out), &json::PY_DEFAULT)
}

fn run_inner(env: &Env, stdin: &str, deps: &SessionDeps<'_>) -> Result<String, PyExc> {
    if (deps.uname)().0 == "Windows" {
        return Ok(system_message_json(REFUSAL));
    }
    let clock = Clock::new(env, (deps.now)());
    let elapsed = || match deps.elapsed {
        Some(f) => f(),
        None => clock.elapsed(),
    };
    let mut warnings: Vec<String> = Vec::new();
    let hook = step(
        &mut warnings,
        label::HOOK_INPUT,
        |_| {
            fault(deps, "_parse_stdin")?;
            Ok(parse_stdin(stdin))
        },
        Map::new(),
    );
    let cwd = match hook.get("cwd").and_then(Value::as_str) {
        Some(c) if !c.is_empty() => PathBuf::from(c),
        _ => (deps.cwd)().map_err(PyExc::from)?,
    };
    if is_subagent(&hook) {
        return Ok(subagent_output(env, &cwd, deps));
    }
    let root = (deps.plugin_root)(env)?;

    let guard_root = step(
        &mut warnings,
        label::ENSURE,
        |_| {
            fault(deps, "ensure")?;
            let loc = guardrails_loc::inspect(&root, env, &cwd);
            if loc.problem.is_none()
                && let Some(guard) = &loc.root
                && let Some(budget) = ensure_budget(env, elapsed(), deps.limits.guard_timeout)
            {
                // The outcome has no line of its own: the self-test below reports a guard
                // that fails closed, and the guardrails' own hook names the reason.
                let _ = (deps.ensure_guard_binary)(guard, budget, env);
            }
            Ok(loc.root)
        },
        None,
    );

    let self_test = step(
        &mut warnings,
        label::SELF_TEST,
        |_| {
            fault(deps, "_guard_selftest")?;
            match deps.self_test {
                Some(f) => f(env, &root, &cwd),
                None => guard_selftest(env, &root, &cwd, deps),
            }
        },
        None,
    );
    warnings.extend(self_test);

    let data_dir = env
        .get_os("CLAUDE_PLUGIN_DATA")
        .filter(|v| !v.is_empty())
        .map(|v| v.to_string_lossy().into_owned());
    let current = step(
        &mut warnings,
        label::STATE,
        |_| Ok(state::read(env)),
        Map::new(),
    );

    step(
        &mut warnings,
        label::DATA_DIR,
        |inner| {
            if let Some(dir) = &data_dir
                && current.get("data_dir").and_then(Value::as_str) != Some(dir.as_str())
                && let Err(exc) =
                    (deps.state_write)(env, vec![("data_dir".into(), Value::Str(dir.clone()))])
            {
                match exc {
                    PyExc::OSError { .. } => inner.push(format!(
                        "anthology: could not write state.json ({})",
                        strerror(&exc)
                    )),
                    other => return Err(other),
                }
            }
            Ok(())
        },
        (),
    );

    if current.get("setup_done").is_none() {
        warnings.push(NAG.into());
    } else {
        let w = step(
            &mut warnings,
            label::READ_RULE,
            |_| {
                fault(deps, "_setup_warning")?;
                Ok(setup_warning(env, &root))
            },
            None,
        );
        warnings.extend(w);
        let w = step(
            &mut warnings,
            label::OPS,
            |_| {
                fault(deps, "_ops_warning")?;
                Ok(ops_warning(env, &root, &current, deps))
            },
            None,
        );
        warnings.extend(w);
        if elapsed() > DEADLINE - RTK_MIN_REMAINING {
            warnings.push(RTK_SKIPPED.into());
        } else {
            let from_env = data_dir.clone().map(Value::Str);
            let dir = from_env.as_ref().or_else(|| current.get("data_dir"));
            let w = step(
                &mut warnings,
                label::RTK,
                |_| reconcile_rtk(env, &root, dir, deps),
                None,
            );
            warnings.extend(w);
        }
    }

    let w = step(
        &mut warnings,
        label::DATA_CHECK,
        |_| {
            fault(deps, "_data_dir_warning")?;
            Ok(data_dir.is_none().then(|| NO_DATA.to_string()))
        },
        None,
    );
    warnings.extend(w);
    let w = step(
        &mut warnings,
        label::WRAPPERS,
        |_| {
            fault(deps, "_wrappers_warning")?;
            let c = deps.compat.ok_or_else(attribute_error)?;
            let bad = (c.wrapper_problems)(&root);
            if bad.is_empty() {
                return Ok(None);
            }
            let names: Vec<String> = bad.iter().map(|n| format!("bin/{n}")).collect();
            Ok(Some(format!(
                "anthology: the plugin's bin/ wrappers are missing or not executable: {}; agents \
                 cannot run them (reinstall the plugin)",
                names.join(", ")
            )))
        },
        None,
    );
    warnings.extend(w);
    let w = step(
        &mut warnings,
        label::AGENT_MEMORY,
        |_| {
            fault(deps, "_agent_memory_warning")?;
            let c = deps.compat.ok_or_else(attribute_error)?;
            agent_memory_warning(env, &cwd, &current, &c, deps)
        },
        None,
    );
    warnings.extend(w);
    let w = step(
        &mut warnings,
        label::SUBAGENT,
        |_| {
            fault(deps, "_subagent_model_warning")?;
            let c = deps.compat.ok_or_else(attribute_error)?;
            Ok((c.subagent_model_source)(env, &cwd).map(|_| SUBAGENT_MODEL_SET.to_string()))
        },
        None,
    );
    warnings.extend(w);

    let w = step(
        &mut warnings,
        label::GIT_FOLDER,
        |_| {
            fault(deps, "_not_a_repo_warning")?;
            Ok(not_a_repo_warning(&cwd))
        },
        None,
    );
    warnings.extend(w);

    let (memory, messages) = memory_step(&mut warnings, &cwd, env, deps);
    // A compact that runs in a git worktree the session moved into (or an agent's worktree)
    // really writes that worktree's setting, but the notice is noise mid-session: keep it quiet.
    let mut messages = messages;
    if hook.get("source").and_then(Value::as_str) == Some("compact") {
        messages.retain(|m| m != anthology_memory::sync::DIR_SET);
    }

    let guide = step(
        &mut warnings,
        label::GUIDE,
        |_| {
            fault(deps, "_read_guide")?;
            Ok(read_guide(&root))
        },
        None,
    );
    if guide.is_none() {
        warnings.push(GUIDE_MISSING.into());
    }
    let guide_text = guide.unwrap_or_default();
    let context = step(
        &mut warnings,
        label::COMPOSE,
        |_| {
            fault(deps, "_compose")?;
            Ok(compose(
                &guide_text,
                &memory,
                &anthology_memory::repo::anthology_home(env),
            ))
        },
        guide_text.clone(),
    );

    // Step 10: prune <ah>/bin; silent, and it never costs the output.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        prune_bin(env, &root, guard_root.as_deref(), (deps.now)())
    }));

    let mut hook_out = Map::new();
    hook_out.insert("hookEventName".into(), Value::Str("SessionStart".into()));
    hook_out.insert("additionalContext".into(), Value::Str(context));
    let mut out = Map::new();
    out.insert("hookSpecificOutput".into(), Value::Object(hook_out));
    let mut all = warnings;
    all.extend(messages);
    let message = all.join("\n");
    if !message.is_empty() {
        out.insert("systemMessage".into(), Value::Str(cap_message(&message)));
    }
    Ok(json::dumps(&Value::Object(out), &json::PY_DEFAULT))
}

/// The memory sync step: `(context text, messages)`; a failure becomes a message.
fn memory_step(
    warnings: &mut Vec<String>,
    cwd: &Path,
    env: &Env,
    deps: &SessionDeps<'_>,
) -> (String, Vec<String>) {
    step(
        warnings,
        label::MEMORY,
        |_| {
            Ok(match (deps.memory_run)(cwd, env) {
                Ok(v) => v,
                Err(exc) => (
                    String::new(),
                    vec![format!("anthology memory: {}", cut(&exc_text(&exc), 120))],
                ),
            })
        },
        (String::new(), Vec::new()),
    )
}

/// What a subagent or teammate gets (a SessionStart that fires inside one after it compacts):
/// the memory index and nothing else. The routing guide and every warning are for the lead,
/// who got them when the session started; the checks behind them (guard self-test, rtk, state
/// writes, the once-per-repo agent-memory note) would only repeat them, cost time, or use up
/// a note the lead has not seen yet. In a subagent a project-scoped guardrails install is
/// judged against the subagent's folder, which can be outside that project, so the warning
/// that guardrails is "not installed" would also be false.
fn subagent_output(env: &Env, cwd: &Path, deps: &SessionDeps<'_>) -> String {
    let (memory, _messages) = memory_step(&mut Vec::new(), cwd, env, deps);
    let context = compose("", &memory, &anthology_memory::repo::anthology_home(env));
    let mut hook_out = Map::new();
    hook_out.insert("hookEventName".into(), Value::Str("SessionStart".into()));
    hook_out.insert("additionalContext".into(), Value::Str(context));
    let mut out = Map::new();
    out.insert("hookSpecificOutput".into(), Value::Object(hook_out));
    json::dumps(&Value::Object(out), &json::PY_DEFAULT)
}

/// [`NOT_A_REPO`] when `cwd` is not in a git repo but some folder directly below it is one (it
/// holds a `.git` folder or file). Only the first [`REPO_SCAN_CAP`] entries are looked at.
fn not_a_repo_warning(cwd: &Path) -> Option<String> {
    (compat::repo_top(cwd).is_none() && repo_below(cwd, REPO_SCAN_CAP))
        .then(|| NOT_A_REPO.to_string())
}

/// Whether one of the first `cap` entries of `dir` holds a `.git`.
fn repo_below(dir: &Path, cap: usize) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    entries
        .take(cap)
        .flatten()
        .any(|entry| entry.path().join(".git").exists())
}

/// `msg if len ≤ 2000 else msg[:1999].rstrip() + "…"`.
pub fn cap_message(message: &str) -> String {
    if char_len(message) <= MESSAGE_CAP {
        message.to_string()
    } else {
        format!("{}…", py_rstrip(cut(message, MESSAGE_CAP - 1)))
    }
}

/// `_agent_memory_warning(cwd, current)`: a soft, once-per-repo line.
fn agent_memory_warning(
    env: &Env,
    cwd: &Path,
    current: &Map,
    c: &CompatDeps,
    deps: &SessionDeps<'_>,
) -> Result<Option<String>, PyExc> {
    let (bad, _good) = (c.agent_memory_problems)(cwd);
    let top = (c.repo_top)(cwd);
    let (Some(first), Some(top)) = (bad.first(), top) else {
        return Ok(None);
    };
    let key = path_str(&top);
    let mut seen = match current.get("agent_memory_warned") {
        Some(Value::Object(m)) => m.clone(),
        _ => Map::new(),
    };
    if seen.get(&key).is_some_and(truthy) {
        return Ok(None);
    }
    seen.insert(key, Value::Bool(true));
    match (deps.state_write)(
        env,
        vec![("agent_memory_warned".into(), Value::Object(seen))],
    ) {
        Ok(()) | Err(PyExc::OSError { .. }) => {} // cannot record it: still say it this once
        Err(other) => return Err(other),
    }
    Ok(Some(format!("anthology: {}", (c.agent_memory_hint)(first))))
}

// --- prune <ah>/bin (design §8.5, GC-17) ----------------------------------------------------

/// Whether `name` is a version folder name (GC-17's version form).
fn is_version(name: &str) -> bool {
    let (core, pre) = match name.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (name, None),
    };
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        && pre.is_none_or(|p| {
            !p.is_empty() && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.')
        })
}

fn decimal(s: &str) -> Option<f64> {
    if s.is_empty() || s.len() > 12 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// Every epoch a `.broken-<version>-<epoch>-<pid>[-<n>]` name can be read with (a prerelease
/// version adds one `-` part, so a name may read two ways).
pub fn broken_epochs(name: &str) -> Vec<f64> {
    let Some(rest) = name.strip_prefix(".broken-") else {
        return Vec::new();
    };
    let parts: Vec<&str> = rest.split('-').collect();
    let mut out = Vec::new();
    // parts = the version (1 or 2 parts), then <epoch>-<pid> or <epoch>-<pid>-<n>.
    for version_parts in [1usize, 2] {
        for tail in [2usize, 3] {
            if parts.len() != version_parts + tail {
                continue;
            }
            let version = parts[..version_parts].join("-");
            let rest = &parts[version_parts..];
            if !is_version(&version) || rest.iter().any(|p| decimal(p).is_none()) {
                continue;
            }
            if let Some(e) = decimal(rest[0]) {
                out.push(e);
            }
        }
    }
    out
}

fn mtime_secs(p: &Path) -> Option<f64> {
    let m = fs::symlink_metadata(p).ok()?.modified().ok()?;
    Some(m.duration_since(UNIX_EPOCH).ok()?.as_secs_f64())
}

/// The version an `anthology.lock` pins, `Err` when it is missing or not valid.
fn pinned_version(root: &Path) -> Result<String, ()> {
    let text = fs::read_to_string(root.join("anthology.lock")).map_err(drop)?;
    let lock = lock::parse(&text, ANTHOLOGY_TAG_PREFIX).map_err(drop)?;
    lock::check_anthology_layout(&text, &lock).map_err(drop)?;
    Ok(lock.version)
}

/// Prunes `<ah>/bin`: `.tmp-*` folders older than 24 h; `.broken-*` folders whose name epoch is
/// older than 24 h; version folders pinned by neither live lock and unused (`.last-used`, else
/// `.sha256`, else the folder) for 30 days. Version folders are pruned only when every live
/// lock reads. Symlinks and anything else are never touched. Returns the names removed.
pub fn prune_bin(
    env: &Env,
    plugin_root: &Path,
    guard_root: Option<&Path>,
    now: f64,
) -> Vec<String> {
    let bin = paths::anthology_home(env).join("bin");
    let mut removed = Vec::new();
    if !fs::symlink_metadata(&bin).is_ok_and(|m| m.is_dir()) {
        return removed;
    }
    let mut pinned = BTreeSet::new();
    let mut versions_ok = true;
    for root in std::iter::once(plugin_root).chain(guard_root) {
        match pinned_version(root) {
            Ok(v) => {
                pinned.insert(v);
            }
            Err(()) => versions_ok = false,
        }
    }
    let Ok(entries) = fs::read_dir(&bin) else {
        return removed;
    };
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let path = entry.path();
        if !fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
            continue;
        }
        let old = if name.starts_with(".tmp-") {
            mtime_secs(&path).is_some_and(|t| now - t > PRUNE_TEMP_SECS)
        } else if name.starts_with(".broken-") {
            let epochs = broken_epochs(&name);
            !epochs.is_empty() && epochs.iter().all(|e| now - e > PRUNE_TEMP_SECS)
        } else if versions_ok && is_version(&name) && !pinned.contains(&name) {
            let used = mtime_secs(&path.join(".last-used"))
                .or_else(|| mtime_secs(&path.join(".sha256")))
                .or_else(|| mtime_secs(&path));
            used.is_some_and(|t| now - t > PRUNE_VERSION_SECS)
        } else {
            false
        };
        if old && fs::remove_dir_all(&path).is_ok() {
            removed.push(name);
        }
    }
    removed.sort();
    removed
}

// --- the self-test's child process ----------------------------------------------------------

/// `subprocess.run(argv, input=stdin, timeout=…, env=env)`: the child runs in a process group
/// of its own, which is killed on the timeout. Each stream keeps at most 1 MiB.
pub fn run_with_timeout(spec: &RunSpec<'_>) -> RunResult {
    let Some((program, args)) = spec.argv.split_first() else {
        return RunResult::Failed(PyExc::OSError {
            errno: 2,
            filename: None,
        });
    };
    let mut cmd = Command::new(program);
    cmd.args(args)
        .env_clear()
        .envs(spec.env.iter())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return RunResult::Failed(PyExc::from(e)),
    };
    let pgid = child.id();
    if let Some(mut stdin) = child.stdin.take() {
        let data = spec.stdin.to_vec();
        thread::spawn(move || {
            let _ = stdin.write_all(&data);
        });
    }
    let collect = |stream: Option<Box<dyn Read + Send>>| {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let done = Arc::new(Mutex::new(stream.is_none()));
        if let Some(mut s) = stream {
            let (buf, done) = (Arc::clone(&buf), Arc::clone(&done));
            thread::spawn(move || {
                let mut chunk = [0u8; 8192];
                while let Ok(n) = s.read(&mut chunk) {
                    if n == 0 {
                        break;
                    }
                    let mut b = buf.lock().unwrap_or_else(|e| e.into_inner());
                    let room = CAPTURE_MAX.saturating_sub(b.len());
                    b.extend_from_slice(&chunk[..n.min(room)]);
                }
                *done.lock().unwrap_or_else(|e| e.into_inner()) = true;
            });
        }
        (buf, done)
    };
    let (out, out_done) = collect(
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    );
    let (err, err_done) = collect(
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    );
    let deadline = Instant::now() + spec.timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(_) => break None,
        }
        if Instant::now() >= deadline {
            kill_group(pgid);
            let _ = child.kill();
            let _ = child.wait();
            return RunResult::TimedOut;
        }
        thread::sleep(Duration::from_millis(5));
    };
    // The streams close when the last holder exits; something that outlives the shell must not
    // hold the hook, so wait at most a moment, then clean up the group.
    let until = Instant::now() + Duration::from_millis(500);
    let finished = || {
        *out_done.lock().unwrap_or_else(|e| e.into_inner())
            && *err_done.lock().unwrap_or_else(|e| e.into_inner())
    };
    while !finished() && Instant::now() < until {
        thread::sleep(Duration::from_millis(5));
    }
    if !finished() {
        kill_group(pgid);
    }
    let code = match status {
        Some(s) => {
            use std::os::unix::process::ExitStatusExt;
            s.code().or_else(|| s.signal().map(|n| -n)).unwrap_or(-1)
        }
        None => -1,
    };
    let stdout = out.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let stderr = err.lock().unwrap_or_else(|e| e.into_inner()).clone();
    RunResult::Exited {
        code,
        stdout,
        stderr,
    }
}

/// `kill -KILL -<pgid>` with the POSIX `kill` utility (safe code: no libc).
fn kill_group(pgid: u32) {
    let group = format!("-{pgid}");
    let _ = Command::new("/bin/sh")
        .args([
            "-c",
            "kill -s KILL -- \"$1\" 2>/dev/null || kill -s KILL \"$1\" 2>/dev/null",
            "sh",
            &group,
        ])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_names() {
        for ok in ["0.1.0", "1.2.3-rc.1", "10.20.30-abc"] {
            assert!(is_version(ok), "{ok}");
        }
        for bad in ["1.2", "1.2.3-", "1.2.3-a-b", ".tmp-x", "v1.2.3", "1..3"] {
            assert!(!is_version(bad), "{bad}");
        }
    }

    #[test]
    fn broken_names_read_every_way() {
        assert_eq!(broken_epochs(".broken-0.1.0-100-7"), vec![100.0]);
        // `0.1.0-100` is a version too, so the pid reads as an epoch: every reading must be old
        // before the folder is pruned, and a pid is never a recent epoch.
        assert_eq!(broken_epochs(".broken-0.1.0-100-7-2"), vec![100.0, 7.0]);
        // A prerelease with a digits-only tag reads two ways.
        assert_eq!(broken_epochs(".broken-0.1.0-5-100-7"), vec![5.0, 100.0]);
        assert!(broken_epochs(".broken-0.1.0-x-7").is_empty());
        assert!(broken_epochs(".broken-junk").is_empty());
    }

    #[test]
    fn the_repo_scan_looks_at_no_more_than_its_cap() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("only/.git")).unwrap();
        assert!(repo_below(tmp.path(), 1));
        assert!(!repo_below(tmp.path(), 0));
        assert!(!repo_below(&tmp.path().join("missing"), REPO_SCAN_CAP));
    }

    #[test]
    fn cap_and_cut_count_code_points() {
        let msg = "é".repeat(2500);
        let capped = cap_message(&msg);
        assert_eq!(capped.chars().count(), 2000);
        assert!(capped.ends_with('…'));
        assert_eq!(cap_message("short"), "short");
    }
}
