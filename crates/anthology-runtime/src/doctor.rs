//! `anthology doctor` (RT1b): the environment checks `/anthology:setup` runs first. A port of
//! `plugins/anthology/scripts/doctor.py`.
//!
//! [`check_env`] returns the thirteen [`Check`]s in GC-11's order; `ok` is `Some(true)`,
//! `Some(false)` or `None` ("cannot tell yet"). [`find_duplicates`] lists installs of the
//! official `superpowers` and `design-skills` plugins, which Anthology already bundles. Nothing
//! here prints a URL, a token or a path out of the user's MCP configuration: only server names,
//! escaped like `repr` ([`safe`]).
//!
//! Differences from the Python, all forced:
//! - F-5: the first check is `anthology_binary` ([`bootstrap::anthology_binary_check`], GC-14)
//!   instead of `python3`. It checks the binary of the guardrails plugin that really runs
//!   ([`guardrails_loc::inspect`]) whenever that finds a folder, a disabled one included; no
//!   folder is "anthology-guardrails not found; nothing to check".
//! - There is no script folder to fall back on, so the plugin root is the `plugin_root`
//!   argument, else `$CLAUDE_PLUGIN_ROOT`. When neither is set, the checks that need it
//!   (`anthology_binary`, `atl` without an explicit lock, `plugin_read_rule`, `path_wrappers`)
//!   say so with `ok = None` ([`UNKNOWN_ROOT`]); nothing is looked up relative to the working
//!   folder.
//! - Files a cloned repo controls are read bounded (2 MiB, regular files only) but parsed to
//!   CPython 3.12's nesting limit, as Python reads them: doctor only reports.
//! - Python's `check failed: <exception>` comes from an `Err` a check returns (an `OSError`
//!   Python would raise) or from a panic inside a check (a seam in tests).
//! - `shutil.which` reads only the `Env` (GC-18): without `PATH` it searches `CS_PATH`, where
//!   Python would read the process environment first. `os.access(X_OK)` is "some execute bit
//!   is set", as in compat (RT1a ruling).
//! - `docker info` runs with stdin from `/dev/null` (Python inherits it).
//!
//! Limitation (as in Python): the BrowserOS tool-name check reads configuration files only; it
//! never connects to the server, so it cannot list the tools the server really offers.
#![forbid(unsafe_code)]

use std::collections::{BTreeSet, HashSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anthology_core::Env;
use anthology_core::lock::Lock;
use anthology_core::paths;
use anthology_core::platform::{self, Target};
use anthology_pycompat::json::{self, Int, Map, Value};
use anthology_pycompat::pyerr::PyExc;
use anthology_pycompat::str as pystr;

use crate::compat::{self, Opened, pathlib_child, pathlib_parent, pathlib_str, realpath};
use crate::{atl_install, bootstrap, guardrails_loc};

/// `SYSTEM_APPLICATIONS` (a [`DoctorLimits`] field in tests).
pub const SYSTEM_APPLICATIONS: &str = "/Applications";
/// `MANAGED_MCP_PATHS` (a [`DoctorLimits`] field in tests).
pub const MANAGED_MCP_PATHS: [&str; 2] = [
    "/Library/Application Support/ClaudeCode/managed-mcp.json",
    "/etc/claude-code/managed-mcp.json",
];
/// Names a BrowserOS binary has on `PATH`.
pub const BROWSEROS_BINARIES: [&str; 3] = ["browseros", "browseros-neo", "BrowserOS"];
/// The server name agents call (`mcp__browseros-neo__*`).
pub const EXPECTED_TOOL_SERVER: &str = "browseros-neo";
/// `docker info`'s timeout (GC-11).
pub const DOCKER_TIMEOUT: Duration = Duration::from_secs(10);
/// `platform.system()` values Anthology supports.
pub const SUPPORTED_SYSTEMS: [&str; 2] = ["Darwin", "Linux"];
/// Plugins Anthology bundles; installing them as well is a duplicate.
pub const DUPLICATE_NAMES: [&str; 2] = ["superpowers", "design-skills"];
/// The scopes `claude plugin uninstall -s` accepts.
pub const UNINSTALL_SCOPES: [&str; 3] = ["user", "project", "local"];
/// The longest name shown, in code points, before `...`.
pub const SHOWN_NAME_LIMIT: usize = 80;
/// How many servers a detail lists before `and N more`.
pub const SHOWN_SERVERS: usize = 5;
/// The checks, in order (GC-11; F-5: `anthology_binary` replaces `python3`).
pub const CHECKS: [&str; 13] = [
    "anthology_binary",
    "node",
    "docker",
    "browseros",
    "atl",
    "platform_supported",
    "plugin_read_rule",
    "browseros_tools",
    "plugin_data",
    "path_wrappers",
    "agent_memory_path",
    "subagent_model",
    "atl_savings",
];
/// The detail of a check that needs the plugin folder when none is known (module docs).
pub const UNKNOWN_ROOT: &str =
    "the anthology plugin folder is not known (CLAUDE_PLUGIN_ROOT is not set)";
/// The `check failed: ` detail is cut to this many code points of the error text.
pub const CHECK_FAILED_LIMIT: usize = 120;

/// One check's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub name: &'static str,
    pub ok: Option<bool>,
    pub detail: String,
}

/// An install of a bundled plugin. `plugin` and `scope` are escaped for printing ([`safe`]);
/// `command` is `None` when the scope or the key is odd (no runnable command is offered).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Duplicate {
    pub plugin: String,
    pub scope: String,
    pub command: Option<String>,
}

/// An MCP server Claude Code would see. `approved` is set only for the `project` scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServer {
    pub name: String,
    pub scope: &'static str,
    pub tool_prefix: String,
    pub approved: Option<bool>,
}

/// What `subprocess.run` gave: an exit status (negative for a signal, as `returncode`), or
/// `Failed` for `OSError` and `TimeoutExpired`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunResult {
    Exited(i32),
    Failed,
}

/// `shutil.which(name, path=env["PATH"])`.
pub type WhichFn = dyn Fn(&str, &Env) -> Option<PathBuf>;
/// `subprocess.run(argv, env=env, timeout=t)` with stdout and stderr discarded.
pub type RunFn = dyn Fn(&[&OsStr], &Env, Duration) -> RunResult;
/// `(platform.system(), platform.machine())`.
pub type UnameFn = dyn Fn() -> (String, String);
/// `atl_install.check_installed(data_dir, lock, target)`.
pub type AtlInstalledFn = dyn Fn(&Path, &Lock, Target) -> bool;

/// The constants tests patch (GC-19).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorLimits {
    /// `SYSTEM_APPLICATIONS`: where a macOS app is looked for (before `~/Applications`).
    pub system_applications: Vec<PathBuf>,
    /// `MANAGED_MCP_PATHS`.
    pub managed_mcp_paths: Vec<PathBuf>,
    /// `compat.MANAGED_SETTINGS_PATHS`.
    pub managed_settings_paths: Vec<PathBuf>,
}

impl Default for DoctorLimits {
    fn default() -> DoctorLimits {
        DoctorLimits {
            system_applications: vec![PathBuf::from(SYSTEM_APPLICATIONS)],
            managed_mcp_paths: MANAGED_MCP_PATHS.iter().map(PathBuf::from).collect(),
            managed_settings_paths: compat::MANAGED_SETTINGS_PATHS
                .iter()
                .map(PathBuf::from)
                .collect(),
        }
    }
}

/// The collaborators tests replace (GC-19); `Default` is production.
pub struct DoctorDeps {
    pub which: Box<WhichFn>,
    pub run: Box<RunFn>,
    pub uname: Box<UnameFn>,
    pub atl_installed: Box<AtlInstalledFn>,
    /// `check_env(lock=…)`: the atl.lock to check against; `None` is `<plugin root>/atl.lock`.
    pub atl_lock: Option<PathBuf>,
    pub limits: DoctorLimits,
}

impl Default for DoctorDeps {
    fn default() -> DoctorDeps {
        DoctorDeps {
            which: Box::new(which),
            run: Box::new(run_command),
            uname: Box::new(platform::uname),
            atl_installed: Box::new(|data_dir, lock, target| {
                atl_install::check_installed_for(data_dir, lock, target).is_ok()
            }),
            atl_lock: None,
            limits: DoctorLimits::default(),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Production seams

/// `os.confstr("CS_PATH")`, the search path `shutil.which` uses without `PATH`.
#[cfg(target_os = "macos")]
const CS_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";
#[cfg(not(target_os = "macos"))]
const CS_PATH: &str = "/bin:/usr/bin";

fn has_nul(b: &[u8]) -> bool {
    b.contains(&0)
}

/// `os.path.exists(p)`: any error (a NUL included) is `False`.
fn py_exists(p: &[u8]) -> bool {
    !has_nul(p) && fs::metadata(OsStr::from_bytes(p)).is_ok()
}

/// `shutil._access_check(fn, F_OK | X_OK)`: exists, executable (some execute bit set), and not
/// a folder.
fn access_check(p: &[u8]) -> bool {
    !has_nul(p)
        && fs::metadata(OsStr::from_bytes(p))
            .is_ok_and(|m| !m.is_dir() && m.permissions().mode() & 0o111 != 0)
}

/// `os.path.join(a, b)` for a relative `b`.
fn py_join(a: &[u8], b: &[u8]) -> Vec<u8> {
    let mut out = a.to_vec();
    if !out.is_empty() && !out.ends_with(b"/") {
        out.push(b'/');
    }
    out.extend_from_slice(b);
    out
}

/// `shutil.which(name, path=env.get("PATH"))` (CPython 3.12, POSIX), with `CS_PATH` when the
/// `Env` has no `PATH`.
pub fn which(name: &str, env: &Env) -> Option<PathBuf> {
    let to_path = |b: Vec<u8>| PathBuf::from(OsString::from_vec(b));
    let (dirname, cmd) = match name.rfind('/') {
        Some(i) => {
            let head = name[..=i].trim_end_matches('/');
            (if head.is_empty() { "/" } else { head }, &name[i + 1..])
        }
        None => ("", name),
    };
    if !dirname.is_empty() {
        let candidate = py_join(dirname.as_bytes(), cmd.as_bytes());
        return access_check(&candidate).then(|| to_path(candidate));
    }
    let path = env
        .get_os("PATH")
        .map(|p| p.as_bytes().to_vec())
        .unwrap_or_else(|| CS_PATH.as_bytes().to_vec());
    if path.is_empty() {
        return None;
    }
    let mut seen = HashSet::new();
    for dir in path.split(|&b| b == b':') {
        if !seen.insert(dir) {
            continue;
        }
        let candidate = py_join(dir, cmd.as_bytes());
        if access_check(&candidate) {
            return Some(to_path(candidate));
        }
    }
    None
}

/// True when `execve` would answer `ENOEXEC` for `program` (Python's `OSError`). On macOS, Rust
/// spawns through `posix_spawn`, which silently falls back to running such a file with `/bin/sh`;
/// Python's `execve` does not, so a file that is neither a `#!` script nor a Mach-O image is
/// treated as a failed spawn there. Linux's `posix_spawn` already reports `ENOEXEC`.
#[cfg(target_os = "macos")]
fn lacks_exec_format(program: &OsStr) -> bool {
    use std::io::Read;
    let mut head = [0u8; 4];
    let Ok(mut f) = fs::File::open(program) else {
        return false;
    };
    let Ok(n) = f.read(&mut head) else {
        return false;
    };
    let head = &head[..n];
    const MACHO: [[u8; 4]; 5] = [
        [0xfe, 0xed, 0xfa, 0xce],
        [0xfe, 0xed, 0xfa, 0xcf],
        [0xce, 0xfa, 0xed, 0xfe],
        [0xcf, 0xfa, 0xed, 0xfe],
        [0xca, 0xfe, 0xba, 0xbe],
    ];
    !(head.starts_with(b"#!") || MACHO.iter().any(|m| head == m))
}

#[cfg(not(target_os = "macos"))]
fn lacks_exec_format(_program: &OsStr) -> bool {
    false
}

/// `subprocess.run(argv, stdout=DEVNULL, stderr=DEVNULL, timeout=…, env=env)`: the child gets
/// exactly `env` and stdin from `/dev/null`, and is killed at the timeout.
pub fn run_command(argv: &[&OsStr], env: &Env, timeout: Duration) -> RunResult {
    let Some((program, args)) = argv.split_first() else {
        return RunResult::Failed;
    };
    if lacks_exec_format(program) {
        return RunResult::Failed;
    }
    let mut cmd = Command::new(program);
    cmd.args(args)
        .env_clear()
        .envs(env.iter())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let Ok(mut child) = cmd.spawn() else {
        return RunResult::Failed;
    };
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return RunResult::Exited(
                    status
                        .code()
                        .or_else(|| status.signal().map(|s| -s))
                        .unwrap_or(-1),
                );
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return RunResult::Failed;
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Python's str() and repr() of JSON values

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Int(Int::Small(i)) => *i != 0,
        Value::Int(Int::Big(_)) => true,
        Value::Float(f) => *f != 0.0,
        Value::Str(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `repr(float)`.
fn float_repr(f: f64) -> String {
    if f.is_nan() {
        "nan".into()
    } else if f.is_infinite() {
        if f > 0.0 { "inf".into() } else { "-inf".into() }
    } else {
        json::dumps(&Value::Float(f), &json::PY_DEFAULT)
    }
}

/// `str(v)` for a parsed JSON value: a str is itself, anything else its `repr`. Iterative, so
/// any depth works (Python raises `RecursionError` on a very deep container: D-1).
pub fn py_str(v: &Value) -> String {
    if let Value::Str(s) = v {
        return s.clone();
    }
    enum Step<'a> {
        Value(&'a Value),
        Text(&'static str),
        Key(&'a str),
    }
    let mut out = String::new();
    let mut stack = vec![Step::Value(v)];
    while let Some(step) = stack.pop() {
        match step {
            Step::Text(t) => out.push_str(t),
            Step::Key(k) => {
                out.push_str(&pystr::repr(k));
                out.push_str(": ");
            }
            Step::Value(v) => match v {
                Value::Null => out.push_str("None"),
                Value::Bool(true) => out.push_str("True"),
                Value::Bool(false) => out.push_str("False"),
                Value::Int(Int::Small(i)) => out.push_str(&i.to_string()),
                Value::Int(Int::Big(s)) => out.push_str(s),
                Value::Float(f) => out.push_str(&float_repr(*f)),
                Value::Str(s) => out.push_str(&pystr::repr(s)),
                Value::Array(a) => {
                    out.push('[');
                    stack.push(Step::Text("]"));
                    for (i, item) in a.iter().enumerate().rev() {
                        stack.push(Step::Value(item));
                        if i > 0 {
                            stack.push(Step::Text(", "));
                        }
                    }
                }
                Value::Object(o) => {
                    out.push('{');
                    stack.push(Step::Text("}"));
                    let items: Vec<_> = o.iter().collect();
                    for (i, (k, item)) in items.into_iter().enumerate().rev() {
                        stack.push(Step::Value(item));
                        stack.push(Step::Key(k));
                        if i > 0 {
                            stack.push(Step::Text(", "));
                        }
                    }
                }
            },
        }
    }
    out
}

/// `_safe(value)`: `repr(str(value))[1:-1]`, cut to [`SHOWN_NAME_LIMIT`] code points plus
/// `...`. Control and non-printable characters (ANSI escapes, newlines, bidi marks) become
/// backslash escapes.
pub fn safe(text: &str) -> String {
    let quoted = pystr::repr(text);
    // The quotes are one ASCII byte each.
    let inner = &quoted[1..quoted.len() - 1];
    if pystr::char_len(inner) <= SHOWN_NAME_LIMIT {
        inner.to_string()
    } else {
        format!("{}...", pystr::char_slice(inner, 0, SHOWN_NAME_LIMIT))
    }
}

// ---------------------------------------------------------------------------------------------
// Small helpers

fn obj(v: Option<&Value>) -> Option<&Map> {
    v.and_then(Value::as_object)
}

/// `[str(n) for n in data] if isinstance(data, dict) else []`: names only, never values.
fn servers_in(data: Option<&Value>) -> Vec<String> {
    obj(data).map_or_else(Vec::new, |m| m.iter().map(|(k, _)| k.to_string()).collect())
}

/// `{n for n in value if isinstance(n, str)} if isinstance(value, list) else set()`.
fn names(value: Option<&Value>) -> HashSet<String> {
    value
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

/// `(here, *here.parents)` for an absolute, normalised path.
fn self_and_parents(here: &Path) -> Vec<&Path> {
    let mut out = Vec::new();
    let mut folder = Some(here);
    while let Some(f) = folder {
        out.push(f);
        folder = f.parent();
    }
    out
}

/// `os.path.realpath(str(cwd))`; the path as given when the walk fails (Python's non-strict
/// realpath never fails on a link loop).
fn real(cwd: &Path) -> PathBuf {
    realpath(cwd).unwrap_or_else(|_| cwd.to_path_buf())
}

/// `compat.read_json(path, REPO_READ_LIMIT)` for a file a cloned repo controls: a regular file
/// (a link to one is fine) of at most 2 MiB, else `None`. It is parsed to CPython 3.12's own
/// nesting limit, as Python parses it (compat's 512-level cap is for the fail-closed guard
/// lookup; doctor only reports). JSON `null` is `None`, as in Python.
fn read_repo_json(path: &Path) -> Option<Value> {
    match compat::open_read(path, Some(compat::REPO_READ_LIMIT)) {
        Opened::Bytes(raw) => compat::loads_user(&compat::decode_text(&raw)),
        Opened::Absent | Opened::Refused => None,
    }
    .filter(|v| !v.is_null())
}

fn state(env: &Env) -> Option<Value> {
    compat::read_json(&compat::config_dir(env).join("anthology").join("state.json"))
        .filter(|v| v.as_object().is_some())
}

/// `env.get(k)` when it is a non-empty value.
fn env_bytes(env: &Env, k: &str) -> Option<Vec<u8>> {
    env.get_os(k)
        .filter(|v| !v.is_empty())
        .map(|v| v.as_bytes().to_vec())
}

fn path_of(b: &[u8]) -> PathBuf {
    PathBuf::from(OsStr::from_bytes(b))
}

/// The plugin folder: `plugin_root`, else `$CLAUDE_PLUGIN_ROOT`, else unknown (module docs).
fn resolve_root(env: &Env, given: &Path) -> Option<PathBuf> {
    if !given.as_os_str().is_empty() {
        return Some(given.to_path_buf());
    }
    env_bytes(env, "CLAUDE_PLUGIN_ROOT").map(|b| path_of(&b))
}

/// `pathlib.Path.is_dir()` (3.12): `ENOENT`, `ENOTDIR`, `EBADF` and `ELOOP` (and a NUL) are
/// `False`; any other error is raised.
fn pathlib_is_dir(p: &[u8]) -> Result<bool, PyExc> {
    if has_nul(p) {
        return Ok(false);
    }
    match fs::metadata(OsStr::from_bytes(p)) {
        Ok(m) => Ok(m.is_dir()),
        Err(e) if ignorable(&e) => Ok(false),
        Err(e) => Err(os_error(e, &pathlib_str(p))),
    }
}

/// pathlib's `_ignore_error`: `ENOENT`, `ENOTDIR`, `EBADF`, `ELOOP`.
fn ignorable(e: &io::Error) -> bool {
    matches!(
        e.raw_os_error(),
        Some(code) if code == libc_errno::ENOENT
            || code == libc_errno::ENOTDIR
            || code == libc_errno::EBADF
            || code == libc_errno::ELOOP
    )
}

/// The errno values pathlib ignores (fixed ABI per OS; no libc dependency here).
mod libc_errno {
    pub const ENOENT: i32 = 2;
    pub const EBADF: i32 = 9;
    pub const ENOTDIR: i32 = 20;
    #[cfg(target_os = "macos")]
    pub const ELOOP: i32 = 62;
    #[cfg(not(target_os = "macos"))]
    pub const ELOOP: i32 = 40;
}

/// An `OSError` for `path`, as Python's `str(exc)` shows it: `[Errno N] <text>: '<path>'`.
fn os_error(e: io::Error, path: &[u8]) -> PyExc {
    let mut exc = PyExc::from(e);
    if let PyExc::OSError { filename, .. } = &mut exc {
        *filename = Some(String::from_utf8_lossy(path).into_owned());
    }
    exc
}

/// `os.path.dirname(p)`.
fn py_dirname(p: &[u8]) -> Vec<u8> {
    let i = p.iter().rposition(|&b| b == b'/').map_or(0, |i| i + 1);
    let head = &p[..i];
    if !head.is_empty() && head.iter().any(|&b| b != b'/') {
        let end = head.iter().rposition(|&b| b != b'/').map_or(0, |j| j + 1);
        head[..end].to_vec()
    } else {
        head.to_vec()
    }
}

/// `Path(p).parents`, as normalised strings.
fn pathlib_parents(p: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut cur = pathlib_str(p);
    loop {
        let parent = pathlib_parent(&cur);
        if parent == cur {
            return out;
        }
        out.push(parent.clone());
        cur = parent;
    }
}

// ---------------------------------------------------------------------------------------------
// MCP servers

/// `_project_approval`: who approved or disabled which project server (module docs of the
/// Python, ruling m14).
struct Approval {
    everything: bool,
    enabled: HashSet<String>,
    disabled: HashSet<String>,
}

impl Approval {
    fn approved(&self, name: &str) -> bool {
        !self.disabled.contains(name) && (self.everything || self.enabled.contains(name))
    }
}

/// `_project_approval(env, claude_json, here)`: Claude Code loads a server of a project
/// `.mcp.json` only when it is approved and not disabled.
///
/// - `disabledMcpjsonServers` from ANY source (user settings.json, `.claude.json`, and the
///   working folder's `.claude/settings.json` and `settings.local.json`) rejects the server.
/// - `enableAllProjectMcpServers: true` or the name in `enabledMcpjsonServers` approves it when
///   it comes from the user's own settings.json or the project's entries in `.claude.json`.
/// - The same keys in `<cwd>/.claude/settings.json` and `settings.local.json` approve it only
///   when the workspace is trusted: `.claude.json` `projects[<key>].hasTrustDialogAccepted` is
///   `true`, `<key>` being the git root of the working folder, else the folder itself.
/// - Repo files are read only up to `REPO_READ_LIMIT`; anything unreadable or oddly shaped
///   approves nothing and disables nothing.
fn project_approval(env: &Env, claude_json: &Map, here: &Path) -> Approval {
    let empty = Map::new();
    let projects = obj(claude_json.get("projects")).unwrap_or(&empty);
    let key = compat::repo_top(here).unwrap_or_else(|| here.to_path_buf());
    let trusted = key
        .to_str()
        .and_then(|k| obj(projects.get(k)))
        .is_some_and(|e| matches!(e.get("hasTrustDialogAccepted"), Some(Value::Bool(true))));
    let user = compat::read_json(&compat::config_dir(env).join("settings.json"));
    let mut own: Vec<&Map> = obj(user.as_ref()).into_iter().collect();
    for folder in self_and_parents(here) {
        if let Some(entry) = folder.to_str().and_then(|k| obj(projects.get(k))) {
            own.push(entry);
        }
    }
    let repo_values: Vec<Option<Value>> = ["settings.json", "settings.local.json"]
        .iter()
        .map(|name| read_repo_json(&here.join(".claude").join(name)))
        .collect();
    let repo: Vec<&Map> = repo_values.iter().filter_map(|v| obj(v.as_ref())).collect();
    let approving: Vec<&Map> = if trusted {
        own.iter().chain(repo.iter()).copied().collect()
    } else {
        own.clone()
    };
    let everything = approving
        .iter()
        .any(|d| matches!(d.get("enableAllProjectMcpServers"), Some(Value::Bool(true))));
    let enabled = approving
        .iter()
        .flat_map(|d| names(d.get("enabledMcpjsonServers")))
        .collect();
    let disabled = own
        .iter()
        .chain(repo.iter())
        .flat_map(|d| names(d.get("disabledMcpjsonServers")))
        .collect();
    Approval {
        everything,
        enabled,
        disabled,
    }
}

/// `find_mcp_servers(env, cwd)`: every MCP server Claude Code would see from `cwd`.
///
/// Scopes: user and local (`.claude.json`), project (`.mcp.json`, walking up from the working
/// folder to its git root, the first one found), plugin (installed plugins' `.mcp.json` and
/// `plugin.json`), managed. Reads names only, never a URL, command, argument, header or token.
/// Missing or malformed files are skipped. Never connects to a server. A project server carries
/// `approved` ([`project_approval`]); the other scopes always load.
pub fn find_mcp_servers(env: &Env, cwd: &Path, limits: &DoctorLimits) -> Vec<McpServer> {
    let mut found = Vec::new();
    let base = match env_bytes(env, "CLAUDE_CONFIG_DIR") {
        Some(b) => path_of(&b),
        None => paths::home_raw(env),
    };
    let data = compat::read_json(&base.join(".claude.json"));
    let empty = Map::new();
    let data = obj(data.as_ref()).unwrap_or(&empty);
    add(
        &mut found,
        servers_in(data.get("mcpServers")),
        "user",
        "mcp__",
    );

    let here = real(cwd);
    if let Some(projects) = obj(data.get("projects")) {
        for folder in self_and_parents(&here) {
            if let Some(entry) = folder.to_str().and_then(|k| obj(projects.get(k))) {
                add(
                    &mut found,
                    servers_in(entry.get("mcpServers")),
                    "local",
                    "mcp__",
                );
            }
        }
    }

    let top = compat::repo_top(cwd);
    for folder in self_and_parents(&here) {
        let mcp = read_repo_json(&folder.join(".mcp.json"));
        if let Some(mcp) = mcp {
            let names = servers_in(obj(Some(&mcp)).and_then(|m| m.get("mcpServers")));
            let approval = project_approval(env, data, &here);
            for name in names {
                found.push(McpServer {
                    tool_prefix: format!("mcp__{name}__"),
                    approved: Some(approval.approved(&name)),
                    name,
                    scope: "project",
                });
            }
            break;
        }
        if top.as_deref() == Some(folder) {
            break;
        }
    }

    let installed = compat::read_json(
        &compat::config_dir(env)
            .join("plugins")
            .join("installed_plugins.json"),
    );
    if let Some(plugins) = obj(obj(installed.as_ref()).and_then(|m| m.get("plugins"))) {
        let mut keys: Vec<(&str, &Value)> = plugins.iter().collect();
        keys.sort_by(|a, b| a.0.cmp(b.0));
        for (key, entries) in keys {
            let plugin = key.split('@').next().unwrap_or(key);
            for entry in entries.as_array().map_or(&[][..], Vec::as_slice) {
                let Some(path) = obj(Some(entry))
                    .and_then(|e| e.get("installPath"))
                    .and_then(Value::as_str)
                    .filter(|p| !p.is_empty())
                else {
                    continue;
                };
                add(
                    &mut found,
                    plugin_server_names(Path::new(path)),
                    "plugin",
                    &format!("mcp__plugin_{plugin}_"),
                );
            }
        }
    }

    for path in &limits.managed_mcp_paths {
        let managed = compat::read_json(path);
        if let Some(m) = obj(managed.as_ref()) {
            add(
                &mut found,
                servers_in(m.get("mcpServers")),
                "managed",
                "mcp__",
            );
        }
    }
    found
}

/// `add(names, scope, head)`: one server per name, tool prefix `<head><name>__`.
fn add(found: &mut Vec<McpServer>, names: Vec<String>, scope: &'static str, head: &str) {
    for name in names {
        found.push(McpServer {
            tool_prefix: format!("{head}{name}__"),
            name,
            scope,
            approved: None,
        });
    }
}

/// The servers an installed plugin declares, sorted and unique: its `.mcp.json` (the
/// `mcpServers` object, or else the top-level object itself) and the `mcpServers` of
/// `.claude-plugin/plugin.json` and `plugin.json`, all read bounded.
fn plugin_server_names(path: &Path) -> Vec<String> {
    let mut names = Vec::new();
    let mcp = read_repo_json(&path.join(".mcp.json"));
    if let Some(m) = obj(mcp.as_ref()) {
        match m.get("mcpServers") {
            Some(inner @ Value::Object(_)) => names.extend(servers_in(Some(inner))),
            _ => names.extend(servers_in(mcp.as_ref())),
        }
    }
    for manifest in [
        path.join(".claude-plugin").join("plugin.json"),
        path.join("plugin.json"),
    ] {
        let meta = read_repo_json(&manifest);
        if let Some(m) = obj(meta.as_ref()) {
            names.extend(servers_in(m.get("mcpServers")));
        }
    }
    let unique: BTreeSet<String> = names.into_iter().collect();
    unique.into_iter().collect()
}

/// `_browseros_servers(env, cwd, loaded)`: BrowserOS servers Claude Code loads (`loaded`), or
/// those in a project file nobody approved.
fn browseros_servers(env: &Env, cwd: &Path, limits: &DoctorLimits, loaded: bool) -> Vec<McpServer> {
    find_mcp_servers(env, cwd, limits)
        .into_iter()
        .filter(|s| {
            pystr::lower(&s.name).contains("browseros") && s.approved.unwrap_or(true) == loaded
        })
        .collect()
}

fn unapproved_note(env: &Env, cwd: &Path, limits: &DoctorLimits) -> String {
    let names: Vec<String> = browseros_servers(env, cwd, limits, false)
        .iter()
        .map(|s| safe(&s.name))
        .collect();
    if names.is_empty() {
        return String::new();
    }
    let mut shown = names[..names.len().min(SHOWN_SERVERS)].join(", ");
    if names.len() > SHOWN_SERVERS {
        shown.push_str(&format!(" and {} more", names.len() - SHOWN_SERVERS));
    }
    format!(
        "found in an unapproved project .mcp.json ({shown}), so it is not counted; approve it in \
         Claude Code or add the server at user scope"
    )
}

// ---------------------------------------------------------------------------------------------
// The checks

type CheckResult = Result<(Option<bool>, String), String>;

fn ok(v: Option<bool>, detail: impl Into<String>) -> CheckResult {
    Ok((v, detail.into()))
}

fn check_node(env: &Env, deps: &DoctorDeps) -> CheckResult {
    if (deps.which)("node", env).is_some() {
        return ok(Some(true), "Node is on PATH");
    }
    ok(
        Some(false),
        "Node not found on PATH; install Node.js (project tooling and some tests need it)",
    )
}

fn check_docker(env: &Env, deps: &DoctorDeps) -> CheckResult {
    let Some(docker) = (deps.which)("docker", env) else {
        return ok(
            Some(false),
            "Docker not found on PATH; install Docker (OrbStack or Docker Desktop)",
        );
    };
    match (deps.run)(
        &[docker.as_os_str(), OsStr::new("info")],
        env,
        DOCKER_TIMEOUT,
    ) {
        RunResult::Failed => ok(
            Some(false),
            "Docker is on PATH but did not answer; start Docker and try again",
        ),
        RunResult::Exited(0) => ok(Some(true), "Docker is running"),
        RunResult::Exited(_) => ok(
            Some(false),
            "Docker is on PATH but not running; start Docker and try again",
        ),
    }
}

/// `any(test(p) for p in root.glob(<prefix>*<suffix>))`, inside doctor's `except OSError`:
/// glob lists the folder (any error: no match), then tests each matching entry in listing
/// order; an error pathlib does not ignore ends the search of this root.
fn glob_any(root: &Path, prefix: &[u8], suffix: &[u8], want_dir: bool) -> bool {
    let Ok(entries) = fs::read_dir(root) else {
        return false;
    };
    let Ok(entries) = entries.collect::<Result<Vec<_>, _>>() else {
        return false;
    };
    for entry in entries {
        let name = entry.file_name();
        let name = name.as_bytes();
        if name.len() < prefix.len() + suffix.len()
            || !name.starts_with(prefix)
            || !name.ends_with(suffix)
        {
            continue;
        }
        match fs::metadata(entry.path()) {
            Ok(m) if (want_dir && m.is_dir()) || (!want_dir && m.is_file()) => return true,
            Ok(_) => {}
            Err(e) if ignorable(&e) => {}
            Err(_) => return false,
        }
    }
    false
}

/// `_browseros_installed(env)`: a BrowserOS app (macOS), binary on `PATH` or AppImage (Linux).
fn browseros_installed(env: &Env, deps: &DoctorDeps) -> bool {
    let home = paths::home_raw(env);
    let apps = home.join("Applications");
    let roots = deps.limits.system_applications.iter().chain([&apps]);
    for root in roots {
        if glob_any(root, b"BrowserOS", b".app", true) {
            return true;
        }
    }
    for root in [apps.clone(), home.join(".local").join("bin")] {
        if glob_any(&root, b"BrowserOS", b".AppImage", false) {
            return true;
        }
    }
    BROWSEROS_BINARIES
        .iter()
        .any(|name| (deps.which)(name, env).is_some())
}

fn check_browseros(env: &Env, cwd: &Path, deps: &DoctorDeps) -> CheckResult {
    if !browseros_servers(env, cwd, &deps.limits, true).is_empty() {
        return ok(Some(true), "a BrowserOS MCP server is configured");
    }
    let note = unapproved_note(env, cwd, &deps.limits);
    let tail = if note.is_empty() {
        String::new()
    } else {
        format!("; a BrowserOS MCP server was {note}")
    };
    if browseros_installed(env, deps) {
        return ok(Some(true), format!("BrowserOS is installed{tail}"));
    }
    ok(
        Some(false),
        format!(
            "BrowserOS Neo not found. It is the only browser Anthology uses; install it yourself \
             (agents never install a browser) and add its MCP server to Claude Code{tail}"
        ),
    )
}

fn check_browseros_tools(env: &Env, cwd: &Path, deps: &DoctorDeps) -> CheckResult {
    let servers = browseros_servers(env, cwd, &deps.limits, true);
    if servers.is_empty() {
        let note = unapproved_note(env, cwd, &deps.limits);
        if !note.is_empty() {
            return ok(
                None,
                format!("no BrowserOS MCP server is loaded: one was {note}"),
            );
        }
        return ok(
            None,
            "no BrowserOS MCP server is configured (see the browseros check)",
        );
    }
    if servers
        .iter()
        .any(|s| s.name == EXPECTED_TOOL_SERVER && s.scope != "plugin")
    {
        return ok(
            Some(true),
            "the BrowserOS MCP server is named browseros-neo (tools: mcp__browseros-neo__*)",
        );
    }
    let mut found = servers
        .iter()
        .take(SHOWN_SERVERS)
        .map(|s| {
            format!(
                "{} ({} scope, tools {}*)",
                safe(&s.name),
                s.scope,
                safe(&s.tool_prefix)
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    if servers.len() > SHOWN_SERVERS {
        found.push_str(&format!("; and {} more", servers.len() - SHOWN_SERVERS));
    }
    ok(
        Some(false),
        format!(
            "agents call mcp__browseros-neo__* tools, but the BrowserOS server is configured as: \
             {found}. Name the server browseros-neo (user, local, project or managed scope)"
        ),
    )
}

/// What the old install left behind, in the legacy name's words.
const LEGACY_STATE_KEY: &str = "the rtk_path key in state.json"; // legacy
const LEGACY_DOWNLOAD: &str = "the old rtk download folder"; // legacy
const LEGACY_HISTORY: &str = "the old rtk history folder"; // legacy

/// Whether anything, a symlink included, is at `path`.
fn lexists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// The legacy rtk install's leftovers: the `rtk_path` key in state.json, `<data>/rtk` and
/// `<config>/anthology/rtk` (legacy names). Session start never removes them; setup does.
fn legacy_rtk_leftovers(env: &Env) -> Vec<&'static str> {
    let state = state(env);
    let state = obj(state.as_ref());
    let mut found = Vec::new();
    let legacy_key = state.is_some_and(|s| s.get("rtk_path").is_some()); // legacy key
    if legacy_key {
        found.push(LEGACY_STATE_KEY);
    }
    let recorded = state
        .and_then(|s| s.get("data_dir"))
        .and_then(Value::as_str)
        .filter(|d| !d.is_empty())
        .map(|d| d.as_bytes().to_vec());
    let legacy_download = env_bytes(env, "CLAUDE_PLUGIN_DATA")
        .or(recorded)
        .map(|data| path_of(&data).join("rtk")); // legacy folder
    if legacy_download.is_some_and(|dir| lexists(&dir)) {
        found.push(LEGACY_DOWNLOAD);
    }
    let history = compat::config_dir(env).join("anthology").join("rtk"); // legacy folder
    if lexists(&history) {
        found.push(LEGACY_HISTORY);
    }
    found
}

/// `_check_atl(env, lock)` plus the legacy rtk leftovers, which only add a note to the detail.
fn check_atl(env: &Env, root: Option<&Path>, deps: &DoctorDeps) -> CheckResult {
    let (verdict, detail) = check_atl_install(env, root, deps)?;
    let left = legacy_rtk_leftovers(env);
    if left.is_empty() {
        return ok(verdict, detail);
    }
    ok(
        verdict,
        format!(
            "{detail}; legacy rtk leftovers: {} (run /anthology:setup to remove them)", // legacy
            left.join(", ")
        ),
    )
}

/// `_check_atl(env, lock)`: state.json's `atl_path` is `<data>/atl/<lock version>/atl`, and
/// that version folder holds an executable atl whose recorded tarball hash is the lock's hash
/// for this platform (`atl_install.check_installed`).
fn check_atl_install(env: &Env, root: Option<&Path>, deps: &DoctorDeps) -> CheckResult {
    let lock_path = match (&deps.atl_lock, root) {
        (Some(lock), _) => lock.clone(),
        (None, Some(root)) => root.join("atl.lock"),
        (None, None) => return ok(None, UNKNOWN_ROOT),
    };
    let Ok(lock) = atl_install::load_lock(&lock_path) else {
        return ok(None, "atl is not wired yet (no atl.lock)");
    };
    let (system, machine) = (deps.uname)();
    let target = match atl_install::target(&system, &machine) {
        Ok(t) => t,
        Err(e) => {
            let text = e.py_str();
            return ok(
                Some(false),
                if text.is_empty() {
                    "this platform is not supported".to_string()
                } else {
                    text
                },
            );
        }
    };
    let state = state(env);
    let state = obj(state.as_ref());
    let path = state.and_then(|s| s.get("atl_path"));
    let Some(path) = path.filter(|p| truthy(p)).map(py_str) else {
        return ok(Some(false), "atl is not installed; run /anthology:setup");
    };
    if !py_exists(path.as_bytes()) {
        return ok(Some(false), "atl is not installed; run /anthology:setup");
    }
    // session start records data_dir; failing that, the folder three levels above the binary.
    // `path_like`: `Path(data_dir)` works (a str or a Path); for another JSON type it raises
    // inside check_installed, which then says False.
    let parents = pathlib_parents(path.as_bytes());
    let recorded = state.and_then(|s| s.get("data_dir")).filter(|v| truthy(v));
    let (data_dir, path_like) = if let Some(b) = env_bytes(env, "CLAUDE_PLUGIN_DATA") {
        (b, true)
    } else if let Some(v) = recorded {
        (py_str(v).into_bytes(), matches!(v, Value::Str(_)))
    } else if parents.len() > 2 {
        (parents[2].clone(), true)
    } else {
        (Vec::new(), true)
    };
    let mut expected = pathlib_str(&data_dir);
    for part in [b"atl".as_slice(), lock.version.as_bytes(), b"atl"] {
        expected = pathlib_child(&expected, part);
    }
    if pathlib_str(path.as_bytes()) == expected
        && path_like
        && (deps.atl_installed)(&path_of(&data_dir), &lock, target)
    {
        return ok(Some(true), "atl matches atl.lock");
    }
    ok(
        Some(false),
        "atl does not match atl.lock; run /anthology:setup",
    )
}

/// `_thousands(n)`: `"{:,}".format(n)`.
fn thousands(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::from(if n < 0 { "-" } else { "" });
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `_per_cent(part, whole)`: one decimal, rounded half up in integers; `0.0%` for no `whole`.
fn per_cent(part: i64, whole: i64) -> String {
    let (part, whole) = (i128::from(part), i128::from(whole));
    let tenths = if whole > 0 {
        (part * 1000 + whole / 2).div_euclid(whole)
    } else {
        0
    };
    format!("{}.{}%", tenths.div_euclid(10), tenths.rem_euclid(10))
}

/// `quote(s)` (`urllib.parse`, `safe="/"`): every UTF-8 byte but `A-Za-z0-9_.-~/` as `%XX`.
fn url_quote(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' | b'/' => {
                char::from(b).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// atl's history as `[seen, ran, tokens, saved]`; `Ok(None)` when there is no file, `Err` for
/// anything Python's `except Exception` catches.
fn atl_history(env: &Env) -> Result<Option<[i64; 4]>, ()> {
    let db = compat::config_dir(env)
        .join("anthology")
        .join("atl")
        .join("history.db");
    let db = pathlib_str(db.as_os_str().as_bytes());
    match fs::metadata(OsStr::from_bytes(&db)) {
        Ok(m) if m.is_file() => {}
        Err(e) if !ignorable(&e) => return Err(()),
        _ => return Ok(None),
    }
    let db = std::str::from_utf8(&db).map_err(drop)?;
    let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
        | rusqlite::OpenFlags::SQLITE_OPEN_URI
        | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let uri = format!("file:{}?mode=ro&immutable=1", url_quote(db));
    let con = rusqlite::Connection::open_with_flags(uri, flags).map_err(drop)?;
    let seen = con
        .query_row("SELECT COUNT(*) FROM hook_decisions", [], |r| r.get(0))
        .map_err(drop)?;
    let (ran, tokens, saved) = con
        .query_row(
            "SELECT COUNT(*), CAST(TOTAL(input_tokens) AS INTEGER), \
             CAST(TOTAL(saved_tokens) AS INTEGER) FROM commands",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(drop)?;
    Ok(Some([seen, ran, tokens, saved]))
}

/// `_check_atl_savings(env)`: what atl did, from its own history (`bin/atl` and `bin/atl-hook`
/// keep it at `<config>/anthology/atl/history.db`): the commands the hook saw, the ones atl ran,
/// and the tokens it saved on their output. It only informs: never `Some(false)`, `None` until
/// there is history. atl keeps the file in WAL mode, which a read-only reader cannot open, so it
/// is read as immutable: no lock, no write, and what atl last checkpointed.
fn check_atl_savings(env: &Env) -> CheckResult {
    const NO_HISTORY: &str = "atl has no history yet";
    let [seen, ran, tokens, saved] = match atl_history(env) {
        Ok(Some([0, 0, ..]) | None) => return ok(None, NO_HISTORY),
        Ok(Some(counts)) => counts,
        Err(()) => return ok(None, "atl's history cannot be read"),
    };
    let (tokens, saved) = (tokens.max(0), saved.max(0));
    ok(
        Some(true),
        format!(
            "atl ran {} of the {} commands the hook saw ({}) and saved {} of their {} output \
             tokens ({}), from atl's own history",
            thousands(ran),
            thousands(seen),
            per_cent(ran, seen),
            thousands(saved),
            thousands(tokens),
            per_cent(saved, tokens)
        ),
    )
}

fn check_platform(deps: &DoctorDeps) -> CheckResult {
    let (system, _) = (deps.uname)();
    if SUPPORTED_SYSTEMS.contains(&system.as_str()) {
        return ok(Some(true), format!("{system} is supported"));
    }
    ok(Some(false), "Anthology supports macOS and Linux only")
}

/// The plugin-read allow rule (same helper as session start). `None` until setup has run.
fn check_plugin_rule(env: &Env, root: Option<&Path>) -> CheckResult {
    let state = state(env);
    if !obj(state.as_ref()).is_some_and(|s| s.contains_key("setup_done")) {
        return ok(None, "setup has not run yet");
    }
    let Some(root) = root else {
        return ok(None, UNKNOWN_ROOT);
    };
    match anthology_settings::plugin_rule_status(env, root) {
        "ok" => ok(
            Some(true),
            "the plugin-read permission matches this install",
        ),
        "unreadable" => ok(
            Some(false),
            "settings.json cannot be read; fix it, then re-run /anthology:setup",
        ),
        _ => ok(
            Some(false),
            "the plugin moved or the rule is gone; re-run /anthology:setup",
        ),
    }
}

/// `${CLAUDE_PLUGIN_DATA}` only exists in hook environments, so the folder session start
/// recorded in state.json stands in for it.
fn check_plugin_data(env: &Env) -> CheckResult {
    let recorded = match env_bytes(env, "CLAUDE_PLUGIN_DATA") {
        Some(b) => Some(b),
        None => {
            let state = state(env);
            obj(state.as_ref())
                .and_then(|s| s.get("data_dir"))
                .filter(|v| truthy(v))
                .map(|v| py_str(v).into_bytes())
        }
    };
    let Some(recorded) = recorded else {
        return ok(
            None,
            "session start has not run since install, so no plugin data folder is recorded yet",
        );
    };
    if pathlib_is_dir(&recorded).map_err(|e| e.py_str())? {
        return ok(Some(true), "the plugin data folder exists");
    }
    ok(
        Some(false),
        "session start recorded a data folder that is gone; start a new session, and if it stays \
         gone Claude Code changed where it keeps plugin data (report it)",
    )
}

fn check_path_wrappers(env: &Env, root: Option<&Path>, deps: &DoctorDeps) -> CheckResult {
    let Some(root) = root else {
        return ok(None, UNKNOWN_ROOT);
    };
    let bad = compat::wrapper_problems(root);
    if !bad.is_empty() {
        let shown: Vec<String> = bad.iter().map(|n| format!("bin/{n}")).collect();
        return ok(
            Some(false),
            format!(
                "the plugin's bin/ is missing or not executable: {} (reinstall the plugin)",
                shown.join(", ")
            ),
        );
    }
    if env_bytes(env, "CLAUDECODE").is_some() {
        let in_bin = |found: PathBuf| {
            let dir = py_dirname(found.as_os_str().as_bytes());
            let dir = if dir.is_empty() { b".".to_vec() } else { dir };
            match (realpath(&path_of(&dir)), realpath(&root.join("bin"))) {
                (Ok(a), Ok(b)) => a == b,
                _ => false,
            }
        };
        if !(deps.which)("anthology-mem", env).is_some_and(in_bin) {
            return ok(
                Some(false),
                "the plugin's bin/ is not on PATH; agents cannot run anthology-mem",
            );
        }
        // Another `atl` found first on PATH would run instead of the plugin's shim.
        if (deps.which)("atl", env).is_some_and(|found| !in_bin(found)) {
            return ok(
                Some(false),
                "the atl on PATH is not the plugin's bin/atl shim; atl commands would run another \
                 program (put the plugin's bin/ first on PATH)",
            );
        }
        return ok(
            Some(true),
            "the plugin's bin/ wrappers exist and bin/ is on PATH",
        );
    }
    ok(
        Some(true),
        "the plugin's bin/ wrappers exist and are executable",
    )
}

fn check_agent_memory(cwd: &Path) -> CheckResult {
    let (bad, good) = compat::agent_memory_problems(cwd);
    if let Some(first) = bad.first() {
        return ok(None, compat::agent_memory_hint(first));
    }
    if !good.is_empty() {
        return ok(
            Some(true),
            "agent notes are under .claude/agent-memory/anthology-<agent>/ (agents save them with \
             anthology-mem save; Claude Code preloads them only while its auto memory is on)",
        );
    }
    ok(None, "no agent memory folders yet")
}

fn check_subagent_model(env: &Env, cwd: &Path, limits: &DoctorLimits) -> CheckResult {
    let found = compat::subagent_model_sources_with(env, cwd, &limits.managed_settings_paths);
    if found.is_empty() {
        return ok(Some(true), "CLAUDE_CODE_SUBAGENT_MODEL is not set");
    }
    let source = found.join(", ");
    ok(
        Some(false),
        format!(
            "CLAUDE_CODE_SUBAGENT_MODEL is set ({source}): it overrides every agent's model, so \
             the roster's opus/sonnet/haiku tiers do not apply. Run /anthology:setup: it offers \
             to remove it from user settings.json and says how to remove it anywhere else."
        ),
    )
}

/// F-5: the guardrails plugin's anthology binary (GC-14), for the guardrails folder
/// `guardrails_loc` finds (a disabled one too: a repo's settings file can mark it disabled).
/// Without a plugin root nothing is looked up (the sibling lookup would be relative to the
/// working folder, which a cloned repo controls).
fn check_anthology_binary(
    env: &Env,
    root: Option<&Path>,
    cwd: &Path,
    deps: &DoctorDeps,
) -> CheckResult {
    let Some(root) = root else {
        return ok(None, UNKNOWN_ROOT);
    };
    let loc = guardrails_loc::inspect(root, env, cwd);
    let (system, machine) = (deps.uname)();
    Ok(bootstrap::anthology_binary_check_with(
        loc.root.as_deref(),
        env,
        (&system, &machine),
    ))
}

/// A panic's message, as Python's `str(exc) or type(exc).__name__`.
fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    let text = payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_default();
    if text.is_empty() {
        "panic".into()
    } else {
        text
    }
}

/// `check_env(env, lock, plugin_root, cwd)`: the thirteen checks in GC-11's order. Never fails:
/// a check that errs or panics is `ok = Some(false)` with `check failed: <error>` (cut to 120
/// code points). An empty `cwd` is the process's working folder.
pub fn check_env(env: &Env, plugin_root: &Path, cwd: &Path, deps: &DoctorDeps) -> Vec<Check> {
    let cwd = if cwd.as_os_str().is_empty() {
        std::env::current_dir().unwrap_or_default()
    } else {
        cwd.to_path_buf()
    };
    let cwd = cwd.as_path();
    let root = resolve_root(env, plugin_root);
    let root = root.as_deref();
    let runners: [(&'static str, &dyn Fn() -> CheckResult); 13] = [
        ("anthology_binary", &|| {
            check_anthology_binary(env, root, cwd, deps)
        }),
        ("node", &|| check_node(env, deps)),
        ("docker", &|| check_docker(env, deps)),
        ("browseros", &|| check_browseros(env, cwd, deps)),
        ("atl", &|| check_atl(env, root, deps)),
        ("platform_supported", &|| check_platform(deps)),
        ("plugin_read_rule", &|| check_plugin_rule(env, root)),
        ("browseros_tools", &|| check_browseros_tools(env, cwd, deps)),
        ("plugin_data", &|| check_plugin_data(env)),
        ("path_wrappers", &|| check_path_wrappers(env, root, deps)),
        ("agent_memory_path", &|| check_agent_memory(cwd)),
        ("subagent_model", &|| {
            check_subagent_model(env, cwd, &deps.limits)
        }),
        ("atl_savings", &|| check_atl_savings(env)),
    ];
    runners
        .iter()
        .map(|(name, run)| {
            let result = panic::catch_unwind(AssertUnwindSafe(run))
                .unwrap_or_else(|payload| Err(panic_text(payload.as_ref())));
            let (ok, detail) = result.unwrap_or_else(|text| {
                (
                    Some(false),
                    format!(
                        "check failed: {}",
                        pystr::char_slice(&text, 0, CHECK_FAILED_LIMIT)
                    ),
                )
            });
            Check { name, ok, detail }
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Duplicates and installed_plugins.json

/// `SAFE_PLUGIN_KEY.fullmatch(key)`: `[A-Za-z0-9][A-Za-z0-9._-]*(@[A-Za-z0-9][A-Za-z0-9._-]*)?`.
fn safe_plugin_key(key: &str) -> bool {
    let part = |p: &str| {
        let b = p.as_bytes();
        !b.is_empty()
            && b[0].is_ascii_alphanumeric()
            && b[1..]
                .iter()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
    };
    match key.split_once('@') {
        None => part(key),
        Some((name, market)) => part(name) && part(market),
    }
}

/// `find_duplicates(installed)` for a parsed `installed_plugins.json` (version 2): installs of
/// the official `superpowers` / `design-skills` plugins, with U4 uninstall commands. `command`
/// is only built from a scope in [`UNINSTALL_SCOPES`] (a missing scope means `user`) and a
/// plugin key of plain name characters; otherwise it is `None`.
pub fn find_duplicates_in(installed: Option<&Value>) -> Vec<Duplicate> {
    let mut found = Vec::new();
    let Some(plugins) = obj(obj(installed).and_then(|m| m.get("plugins"))) else {
        return found;
    };
    let mut keys: Vec<(&str, &Value)> = plugins.iter().collect();
    keys.sort_by(|a, b| a.0.cmp(b.0));
    let user = Value::Str("user".into());
    for (name, entries) in keys {
        let base = name.split('@').next().unwrap_or(name);
        let Some(entries) = entries.as_array() else {
            continue;
        };
        if !DUPLICATE_NAMES.contains(&base) {
            continue;
        }
        for entry in entries {
            let Some(entry) = entry.as_object() else {
                continue;
            };
            let scope = match entry.get("scope") {
                None | Some(Value::Null) => &user,
                Some(v) => v,
            };
            let command = match scope {
                Value::Str(s)
                    if UNINSTALL_SCOPES.contains(&s.as_str()) && safe_plugin_key(name) =>
                {
                    Some(format!("claude plugin uninstall {name} -s {s}"))
                }
                _ => None,
            };
            found.push(Duplicate {
                plugin: safe(name),
                scope: safe(&py_str(scope)),
                command,
            });
        }
    }
    found
}

/// `installed_plugins_state(env)`: the parsed `<config>/plugins/installed_plugins.json` and
/// `"ok"`, or `None` and `"missing"` (no such file) or `"unreadable"` (any other read error,
/// bytes that are not UTF-8, JSON that does not parse or nests past CPython's limit).
pub fn installed_plugins_state(env: &Env) -> (Option<Value>, &'static str) {
    let path = compat::config_dir(env)
        .join("plugins")
        .join("installed_plugins.json");
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.raw_os_error() == Some(libc_errno::ENOENT) => return (None, "missing"),
        Err(_) => return (None, "unreadable"),
    };
    let Ok(text) = String::from_utf8(bytes) else {
        return (None, "unreadable");
    };
    // read_text's universal newlines: only whitespace or invalid JSON can hold a CR.
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    match compat::loads_user(&text) {
        Some(v) => (Some(v), "ok"),
        None => (None, "unreadable"),
    }
}

/// `installed_plugins(env)`: the parsed file when it is an object, else an empty map.
pub fn installed_plugins(env: &Env) -> Map {
    match installed_plugins_state(env).0 {
        Some(mut v) => v.take().into_object().unwrap_or_default(),
        None => Map::new(),
    }
}

/// `find_duplicates(installed_plugins(env))` (plan §3.6). `cwd` is unused: the installs that
/// count are in the user's config.
pub fn find_duplicates(env: &Env, _cwd: &Path) -> Vec<Duplicate> {
    find_duplicates_in(installed_plugins_state(env).0.as_ref())
}

// ---------------------------------------------------------------------------------------------
// main()

/// What `main` prints: `{"checks", "duplicates", "installed_plugins"}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub checks: Vec<Check>,
    pub duplicates: Vec<Duplicate>,
    pub installed_plugins: &'static str,
}

/// The report `main` prints, for the plugin root, working folder and seams given.
pub fn report(env: &Env, plugin_root: &Path, cwd: &Path, deps: &DoctorDeps) -> Report {
    let (installed, state) = installed_plugins_state(env);
    Report {
        checks: check_env(env, plugin_root, cwd, deps),
        duplicates: find_duplicates_in(installed.as_ref()),
        installed_plugins: state,
    }
}

fn opt_bool(v: Option<bool>) -> Value {
    v.map_or(Value::Null, Value::Bool)
}

/// A check as JSON: `{"name", "ok", "detail"}`.
pub fn check_value(c: &Check) -> Value {
    let mut m = Map::new();
    m.insert("name".into(), Value::Str(c.name.into()));
    m.insert("ok".into(), opt_bool(c.ok));
    m.insert("detail".into(), Value::Str(c.detail.clone()));
    Value::Object(m)
}

/// A duplicate as JSON: `{"plugin", "scope", "command"}`.
pub fn duplicate_value(d: &Duplicate) -> Value {
    let mut m = Map::new();
    m.insert("plugin".into(), Value::Str(d.plugin.clone()));
    m.insert("scope".into(), Value::Str(d.scope.clone()));
    m.insert(
        "command".into(),
        d.command.clone().map_or(Value::Null, Value::Str),
    );
    Value::Object(m)
}

/// `main`'s stdout and exit code for `report`: `json.dumps(result, indent=2)` with `json`, else
/// one `"%-18s %-4s %s"` line per check, then the unreadable note and the duplicate lines. The
/// exit code is 0 when `platform_supported` is ok, else 2 (GC-5).
pub fn render(report: &Report, as_json: bool) -> (String, i32) {
    let mut out = String::new();
    if as_json {
        let mut m = Map::new();
        m.insert(
            "checks".into(),
            Value::Array(report.checks.iter().map(check_value).collect()),
        );
        m.insert(
            "duplicates".into(),
            Value::Array(report.duplicates.iter().map(duplicate_value).collect()),
        );
        m.insert(
            "installed_plugins".into(),
            Value::Str(report.installed_plugins.into()),
        );
        out.push_str(&json::dumps(&Value::Object(m), &json::INDENT2));
        out.push('\n');
    } else {
        for c in &report.checks {
            let mark = match c.ok {
                Some(true) => "ok",
                Some(false) => "FAIL",
                None => "n/a",
            };
            out.push_str(&format!("{:<18} {:<4} {}\n", c.name, mark, c.detail));
        }
        if report.installed_plugins == "unreadable" {
            out.push_str(
                "duplicates: installed_plugins.json is unreadable, so installs of the bundled \
                 plugins were not checked\n",
            );
        }
        for d in &report.duplicates {
            match &d.command {
                Some(command) => {
                    out.push_str(&format!(
                        "duplicate: {} ({}): {command}\n",
                        d.plugin, d.scope
                    ));
                }
                None => out.push_str(&format!(
                    "duplicate: {} (scope '{}' is not one of {}): no uninstall command offered\n",
                    d.plugin,
                    d.scope,
                    UNINSTALL_SCOPES.join(", ")
                )),
            }
        }
    }
    let supported = report
        .checks
        .iter()
        .find(|c| c.name == "platform_supported")
        .and_then(|c| c.ok)
        .unwrap_or(false);
    (out, if supported { 0 } else { 2 })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `subprocess.run` raises `OSError` for a file `execve` cannot run (`ENOEXEC`); so must
    /// `run_command` on every platform (macOS's `posix_spawn` would run it with `sh`).
    #[test]
    fn run_command_fails_on_a_file_without_exec_format() {
        let run = |body: &[u8]| {
            let dir = tempfile::tempdir().expect("tempdir");
            let exe = dir.path().join("docker");
            fs::write(&exe, body).expect("write");
            fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).expect("chmod");
            run_command(
                &[exe.as_os_str(), OsStr::new("info")],
                &Env::default(),
                Duration::from_secs(10),
            )
        };
        assert_eq!(run(b"not a program\n"), RunResult::Failed);
        assert_eq!(run(b""), RunResult::Failed);
        assert_eq!(run(b"#!/bin/sh\nexit 3\n"), RunResult::Exited(3));
    }

    #[test]
    fn safe_escapes_and_cuts() {
        assert_eq!(safe("a\x1b[31m\nb"), "a\\x1b[31m\\nb");
        assert_eq!(safe("it's"), "it's");
        assert_eq!(safe("it's \"q\""), "it\\'s \"q\"");
        let long = "x".repeat(81);
        assert_eq!(safe(&long), format!("{}...", "x".repeat(80)));
        assert_eq!(safe(&"x".repeat(80)), "x".repeat(80));
    }

    #[test]
    fn py_str_of_json_values() {
        let v = json::loads(r#"[1, "a", null, true, 1.5, {"k": [false]}]"#).expect("json");
        assert_eq!(py_str(&v), "[1, 'a', None, True, 1.5, {'k': [False]}]");
        assert_eq!(py_str(&Value::Str("x".into())), "x");
    }

    #[test]
    fn plugin_keys() {
        for good in ["superpowers@m", "a", "a.b_c-d@x.y"] {
            assert!(safe_plugin_key(good), "{good}");
        }
        for bad in ["", "@m", "a@", "a@b@c", "a b", "-a", "a@-b", "é@m", "a\n"] {
            assert!(!safe_plugin_key(bad), "{bad:?}");
        }
    }

    #[test]
    fn dirname_and_parents() {
        assert_eq!(py_dirname(b"/usr/bin/node"), b"/usr/bin");
        assert_eq!(py_dirname(b"node"), b"");
        assert_eq!(py_dirname(b"/node"), b"/");
        assert_eq!(py_dirname(b"a//b"), b"a");
        assert_eq!(pathlib_parents(b"/a/b/c").len(), 3);
        assert_eq!(pathlib_parents(b"a/b/c").len(), 3);
        assert_eq!(pathlib_parents(b"x").len(), 1);
        assert_eq!(pathlib_parents(b"/").len(), 0);
    }

    #[test]
    fn which_without_path_dirs() {
        let env = Env::default().with("PATH", "");
        assert_eq!(which("sh", &env), None);
        let env = Env::default().with("PATH", "/nonexistent:/bin:/usr/bin");
        assert!(which("sh", &env).is_some());
        assert_eq!(
            which("/bin/sh", &Env::default()),
            Some(PathBuf::from("/bin/sh"))
        );
    }
}
