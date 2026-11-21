//! `anthology setup preview|apply|remove` (RT4; port of
//! `plugins/anthology/scripts/anthology_setup.py`, GC-10).
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

use anthology_core::{Env, platform, state};
use anthology_pycompat::json::Value;
use anthology_pycompat::pyerr::PyExc;

use crate::bootstrap::{self, GuardBinaryStatus};
use crate::session_start::{RealRtk, RtkWiring, StateWrite};

/// Windows (setup refuses everything there).
pub const REFUSAL: &str = "Anthology supports macOS and Linux only; it is inactive on Windows.";
/// Seconds per read for the rtk download (overall about 4×): setup is where the first download
/// happens (GC-9: T = 60).
pub const RTK_TIMEOUT: f64 = 60.0;
/// Seconds the guardrails-pinned binary may take to install (D§8.8).
pub const GUARD_BUDGET: u64 = 60;

/// The memory functions Python's tests patch (`memory_sync.repo_root`, `repo_key`, `approve`).
pub struct MemoryDeps<'a> {
    /// `memory_sync.repo_root(cwd, env)`: the repo's top folder, or `None` outside a repo.
    pub repo_root: &'a dyn Fn(&Path, &Env) -> Option<PathBuf>,
    /// `memory_sync.repo_key(root, env)`.
    pub repo_key: &'a dyn Fn(&Path, &Env) -> String,
    /// `memory_sync.approve(root, env)`: the key. `PyExc::Other("UnsafeTarget", _)` is
    /// Python's `UnsafeTarget`, raised after the approval is recorded.
    pub approve: &'a dyn Fn(&Path, &Env) -> Result<String, PyExc>,
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

// SWAP(MS2): `|cwd, env| anthology_memory::repo::repo_root(cwd, env, &RealGit)`.
fn repo_root_default(_cwd: &Path, _env: &Env) -> Option<PathBuf> {
    None
}

// SWAP(MS2): `|root, env| anthology_memory::repo::repo_key(root, env, &RealGit)`.
fn repo_key_default(_root: &Path, _env: &Env) -> String {
    String::new()
}

// SWAP(MS2): `|root, env| anthology_memory::approve::approve(root, env, &RealGit)`.
fn approve_default(root: &Path, env: &Env) -> Result<String, PyExc> {
    Ok(repo_key_default(root, env))
}

// SWAP(RT1b): `doctor::check_env(env, root, cwd, &DoctorDeps { rtk_lock: Some(root.join("rtk.lock")), ..Default::default() })`
// mapped through `doctor::check_value`.
fn checks_default(_env: &Env, _root: &Path, _cwd: &Path) -> Value {
    Value::Array(Vec::new())
}

// SWAP(RT1b): `doctor::installed_plugins_state`.
fn installed_plugins_state_default(_env: &Env) -> (Option<Value>, &'static str) {
    (None, "missing")
}

// SWAP(RT1b): `doctor::find_duplicates_in` mapped through `doctor::duplicate_value`.
fn find_duplicates_default(_installed: Option<&Value>) -> Value {
    Value::Array(Vec::new())
}

fn settings_write_default(path: &Path, text: &str) -> Result<(), PyExc> {
    anthology_settings::write_text(path, text)
}

impl Default for MemoryDeps<'static> {
    fn default() -> MemoryDeps<'static> {
        MemoryDeps {
            repo_root: &repo_root_default,
            repo_key: &repo_key_default,
            approve: &approve_default,
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

/// `preview(plugin_root, cwd, env)`: what setup would do; writes nothing.
pub fn preview(plugin_root: &Path, cwd: Option<&Path>, env: &Env, deps: &SetupDeps) -> Value {
    let _ = (plugin_root, cwd, env, deps);
    todo!("RT4")
}

/// `apply(plugin_root, expect_sha256, cwd, skip_approve, env, promote)`.
pub fn apply(
    plugin_root: &Path,
    expect_sha256: &str,
    cwd: Option<&Path>,
    skip_approve: bool,
    env: &Env,
    promote: &[String],
    deps: &SetupDeps,
) -> Value {
    let _ = (plugin_root, expect_sha256, cwd, skip_approve, env, promote, deps);
    todo!("RT4")
}

/// `remove(plugin_root, cwd, unapprove, unapprove_all, env, preview)`.
pub fn remove(
    plugin_root: &Path,
    cwd: Option<&Path>,
    unapprove: bool,
    unapprove_all: bool,
    env: &Env,
    preview: bool,
    deps: &SetupDeps,
) -> Value {
    let _ = (plugin_root, cwd, unapprove, unapprove_all, env, preview, deps);
    todo!("RT4")
}

/// Python `main(argv)`: returns (exit code, stdout text exactly as Python prints it, stderr text).
pub fn main(argv: &[String], env: &Env, deps: &SetupDeps) -> (i32, String, String) {
    let _ = (argv, env, deps);
    todo!("RT4")
}
