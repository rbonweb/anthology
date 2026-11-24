//! `anthology-dev version-bump` (scripts/check_version_bump.py): a plugin whose files changed
//! must carry a higher version. Owner: DV2.
//!
//! Only released plugins are gated: a plugin counts as released once a git tag `<plugin>--v*`
//! exists on a commit reachable from the base. A changed released plugin passes when its head
//! version is above the highest such tag (semver, all tags) and not below its version at `git merge-base <base> <head>`, so a bump that
//! already landed on the base counts and a branch does not have to bump it again.
//!
//! Exit codes: 0 pass (or nothing to check), 1 a plugin changed without a valid bump, 2 the refs
//! could not be resolved or share no history (and 2 for a usage error).

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anthology_core::Env;
use anthology_pycompat::json::{self, Value};
use anthology_pycompat::pyerr::PyExc;

use crate::Output;
use crate::json_edit::{Uncaught, py_repr, universal_newlines, utf8_decode_error};
use crate::memory_diff::pystr::splitlines;
use crate::semver;
use anthology_pycompat::argparse::{self, Arg};
use anthology_pycompat::str as pystr;

/// The gate marker of the Python script. `GATE_PATH_RS` is its Rust successor (ruling R7): a
/// base has the gate if either file exists at the merge base.
pub const GATE_PATH: &str = "scripts/check_version_bump.py";
pub const GATE_PATH_RS: &str = "crates/anthology-dev/src/version_bump.rs";
pub const PLUGINS_DIR: &str = "plugins";

// ---- pure logic (no git) -------------------------------------------------------------

/// `changed_plugins`: sorted plugin names with at least one changed path under
/// `plugins/<name>/`.
pub fn changed_plugins<S: AsRef<str>>(paths: &[S]) -> Vec<String> {
    let prefix = format!("{PLUGINS_DIR}/");
    let mut names = BTreeSet::new();
    for path in paths {
        let Some(rest) = path.as_ref().strip_prefix(&prefix) else {
            continue;
        };
        if let Some((name, _)) = rest.split_once('/')
            && !name.is_empty()
        {
            names.insert(name.to_string());
        }
    }
    names.into_iter().collect()
}

/// A version that is not a string cannot be valid semver; its `{!r}` is the Python repr.
fn repr(v: &Value) -> String {
    py_repr(v)
}

/// `verdict`: `(ok, reason)` for one changed plugin. The versions are the JSON values found in
/// the manifests (`Null` for a missing one).
pub fn verdict(
    plugin: &str,
    base_version: &Value,
    head_version: &Value,
    base_exists: bool,
    head_exists: bool,
) -> (bool, String) {
    if !base_exists {
        return (true, format!("plugins/{plugin} is new"));
    }
    if !head_exists {
        return (true, format!("plugins/{plugin} was removed"));
    }
    let Some(head) = semver::parse_value(head_version) else {
        return (
            false,
            format!(
                "plugins/{plugin} version {} is not valid semver",
                repr(head_version)
            ),
        );
    };
    let Some(base) = semver::parse_value(base_version) else {
        // A broken base version must not lock the plugin out of fixing it.
        return (
            true,
            format!(
                "plugins/{plugin} {} (base version {} was invalid)",
                head_version.as_str().unwrap_or_default(),
                repr(base_version)
            ),
        );
    };
    let (b, h) = (
        base_version.as_str().unwrap_or_default(),
        head_version.as_str().unwrap_or_default(),
    );
    match semver::cmp_parsed(&head, &base) {
        std::cmp::Ordering::Equal => (
            false,
            format!("plugins/{plugin} changed without a version bump (still {b})"),
        ),
        std::cmp::Ordering::Less => (
            false,
            format!("plugins/{plugin} version went down ({b} -> {h})"),
        ),
        std::cmp::Ordering::Greater => (true, format!("plugins/{plugin} {b} -> {h}")),
    }
}

/// The verdict for a released plugin: the head must be above the latest release `release` (the
/// highest `<plugin>--v*` tag that is valid semver) and not below the version on the merge base.
/// A bump already on the base therefore counts. Without a usable release version this is
/// [`verdict`]: the head is compared with the base alone.
pub fn release_verdict(
    plugin: &str,
    base_version: &Value,
    head_version: &Value,
    base_exists: bool,
    head_exists: bool,
    release: Option<&str>,
) -> (bool, String) {
    let plain = || verdict(plugin, base_version, head_version, base_exists, head_exists);
    if !base_exists || !head_exists {
        return plain();
    }
    let (Some(head), Some(release_v)) = (
        semver::parse_value(head_version),
        release.and_then(semver::parse),
    ) else {
        return plain();
    };
    // A base version that is not valid semver only drops the base side of the check.
    let base = semver::parse_value(base_version);
    let vs_base = base.as_ref().map(|b| semver::cmp_parsed(&head, b));
    if vs_base == Some(std::cmp::Ordering::Less) {
        return plain();
    }
    let (b, h, r) = (
        base_version.as_str().unwrap_or_default(),
        head_version.as_str().unwrap_or_default(),
        release.unwrap_or_default(),
    );
    match (semver::cmp_parsed(&head, &release_v), vs_base) {
        (std::cmp::Ordering::Greater, None) => plain(),
        (std::cmp::Ordering::Greater, Some(std::cmp::Ordering::Greater)) => {
            (true, format!("plugins/{plugin} {b} -> {h}"))
        }
        (std::cmp::Ordering::Greater, Some(_)) => (
            true,
            format!("plugins/{plugin} {h} (bump already on the base; latest release {r})"),
        ),
        (std::cmp::Ordering::Equal, Some(std::cmp::Ordering::Equal)) => (
            false,
            format!("plugins/{plugin} changed without a version bump (still {b})"),
        ),
        _ => (
            false,
            format!("plugins/{plugin} version {h} is not above the latest release {r}"),
        ),
    }
}

/// The highest release of `plugin` among `tags`: the semver-greatest `<plugin>--v<version>`.
/// `None` when no such tag has a valid semver version. The tag is matched by prefix, not by git
/// glob, so a plugin name can never be read as a pattern and `pp--v1.0.0` is not a release of `p`.
pub fn highest_release<S: AsRef<str>>(tags: &[S], plugin: &str) -> Option<String> {
    let prefix = format!("{plugin}--v");
    tags.iter()
        .filter_map(|t| t.as_ref().strip_prefix(&prefix))
        .filter_map(|v| semver::parse(v).map(|parsed| (parsed, v)))
        .max_by(|(a, _), (b, _)| semver::cmp_parsed(a, b))
        .map(|(_, v)| v.to_string())
}

// ---- git layer -----------------------------------------------------------------------

/// Why the gate stopped: `GateError` (exit 2), or an exception Python does not catch (R10).
#[derive(Debug)]
enum Stop {
    Gate(String),
    Uncaught(Uncaught),
}

struct Proc {
    code: i32,
    stdout: String,
    stderr: String,
}

/// `git -C <repo> …` run as Python's `subprocess.run(…, universal_newlines=True)` does, with
/// exactly `env` as its environment, started in `cwd`.
struct Git<'a> {
    env: &'a Env,
    cwd: &'a Path,
    repo: &'a OsStr,
}

/// Decodes a child's output as `universal_newlines=True` does.
fn decode(bytes: Vec<u8>) -> Result<String, Stop> {
    match String::from_utf8(bytes) {
        Ok(s) => Ok(universal_newlines(&s)),
        Err(e) => Err(Stop::Uncaught(Uncaught::new(
            "UnicodeDecodeError",
            utf8_decode_error(e.as_bytes()).unwrap_or_default(),
        ))),
    }
}

impl Git<'_> {
    fn run(&self, args: &[&str]) -> Result<Proc, Stop> {
        let out = Command::new("git")
            .arg("-C")
            .arg(self.repo)
            .args(args)
            .current_dir(self.cwd)
            .env_clear()
            .envs(self.env.iter())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| {
                let exc = match PyExc::from(e) {
                    PyExc::OSError { errno, .. } => PyExc::OSError {
                        errno,
                        filename: Some("git".into()),
                    },
                    other => other,
                };
                Stop::Uncaught(Uncaught::from_exc(&exc))
            })?;
        Ok(Proc {
            code: out.status.code().unwrap_or(-1),
            stdout: decode(out.stdout)?,
            stderr: decode(out.stderr)?,
        })
    }

    /// `_git_ok`: stdout, or a `GateError` naming the failed command.
    fn ok(&self, args: &[&str]) -> Result<String, Stop> {
        let r = self.run(args)?;
        if r.code != 0 {
            return Err(Stop::Gate(format!(
                "git {} failed: {}",
                args.join(" "),
                pystr::strip_ws(&r.stderr)
            )));
        }
        Ok(r.stdout)
    }

    /// `_resolve`: the commit `reference` names.
    fn resolve(&self, reference: &str) -> Result<String, Stop> {
        let r = self.run(&[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{reference}^{{commit}}"),
        ])?;
        if r.code != 0 {
            return Err(Stop::Gate(format!(
                "cannot resolve {} to a commit (is the checkout deep enough?)",
                crate::json_edit::str_repr(reference)
            )));
        }
        Ok(pystr::strip_ws(&r.stdout).to_string())
    }

    /// `_plugin_version_at`: `(manifest exists at rev, its version or Null if unreadable)`.
    fn plugin_version_at(&self, rev: &str, plugin: &str) -> Result<(bool, Value), Stop> {
        let spec = format!("{rev}:{PLUGINS_DIR}/{plugin}/.claude-plugin/plugin.json");
        let r = self.run(&["show", &spec])?;
        if r.code != 0 {
            return Ok((false, Value::Null));
        }
        // `json.loads(...).get("version")`: a ValueError or an AttributeError (not an object)
        // means unreadable.
        let version = json::loads(&r.stdout)
            .ok()
            .and_then(|v| v.as_object().and_then(|m| m.get("version")).cloned())
            .unwrap_or(Value::Null);
        Ok((true, version))
    }

    /// `_released_plugins`: each plugin with a `<plugin>--v*` tag, with its highest release (see
    /// [`highest_release`]; `None` if no such tag is valid semver). Matched by prefix, not by git
    /// glob, so a plugin name can never be read as a pattern. Only tags on a commit that is an
    /// ancestor of `base` count: a tag made inside `base..head` (the release commit of a push to
    /// the base branch carries its own new tag) is not a release the head has to exceed.
    fn released_plugins(
        &self,
        plugins: &[String],
        base: &str,
    ) -> Result<BTreeMap<String, Option<String>>, Stop> {
        let listing = self.ok(&[
            "for-each-ref",
            "--merged",
            base,
            "--format=%(refname:strip=2)",
            "refs/tags",
        ])?;
        let tags = splitlines(&listing, false);
        Ok(plugins
            .iter()
            .filter(|p| {
                let prefix = format!("{p}--v");
                tags.iter().any(|t| t.starts_with(&prefix))
            })
            .map(|p| (p.clone(), highest_release(&tags, p)))
            .collect())
    }
}

/// `run`: prints the verdict lines to `out` and returns the exit code.
fn gate(base: &str, head: &str, git: &Git<'_>, out: &mut String) -> Result<i32, Stop> {
    if !base.is_empty() && base.bytes().all(|b| b == b'0') {
        out.push_str("version gate: no base commit (new branch); nothing to compare\n");
        return Ok(0);
    }
    let base_c = git.resolve(base)?;
    let head_c = git.resolve(head)?;
    let mb = git.run(&["merge-base", &base_c, &head_c])?;
    if mb.code != 0 || pystr::strip_ws(&mb.stdout).is_empty() {
        return Err(Stop::Gate(format!(
            "no merge base between {base} and {head}"
        )));
    }
    let mb_c = pystr::strip_ws(&mb.stdout).to_string();
    let has_gate = [GATE_PATH, GATE_PATH_RS]
        .iter()
        .try_fold(false, |found, path| {
            if found {
                return Ok(true);
            }
            git.run(&["cat-file", "-e", &format!("{mb_c}:{path}")])
                .map(|r| r.code == 0)
        })?;
    if !has_gate {
        out.push_str("version gate: base predates the gate; skipping\n");
        return Ok(0);
    }
    // -z: NUL-separated and never quoted (core.quotePath would hide non-ASCII paths from the filter)
    let diff = git.ok(&[
        "-c",
        "core.quotePath=false",
        "diff",
        "--name-only",
        "-z",
        "--no-renames",
        &mb_c,
        &head_c,
    ])?;
    let paths: Vec<&str> = diff.split('\0').filter(|p| !p.is_empty()).collect();
    let plugins = changed_plugins(&paths);
    if plugins.is_empty() {
        out.push_str("version gate: no plugin changed\n");
        return Ok(0);
    }
    let released = git.released_plugins(&plugins, &base_c)?;
    let mut failed = false;
    for plugin in &plugins {
        let Some(release) = released.get(plugin) else {
            out.push_str(&format!(
                "{plugin}: unreleased (no {plugin}--v* tag) \u{2014} version bump not required\n"
            ));
            continue;
        };
        let (base_exists, base_version) = git.plugin_version_at(&mb_c, plugin)?;
        let (head_exists, head_version) = git.plugin_version_at(&head_c, plugin)?;
        let (ok, reason) = release_verdict(
            plugin,
            &base_version,
            &head_version,
            base_exists,
            head_exists,
            release.as_deref(),
        );
        out.push_str(&format!("{} {reason}\n", if ok { "ok  " } else { "FAIL" }));
        failed = failed || !ok;
    }
    Ok(i32::from(failed))
}

// ---- command line --------------------------------------------------------------------

/// `main`'s `ArgumentParser` (`prog` is the script's name, as `sys.argv[0]` gives it).
/// `COLUMNS` sets the help width, as `shutil.get_terminal_size` reads it.
fn spec(env: &Env) -> argparse::Spec {
    argparse::Spec::new("check_version_bump.py")
        .description("Version-bump gate: a plugin whose files changed must carry a higher version.")
        .width(argparse::help_width(env.get("COLUMNS"), None))
        .arg(Arg::positional("base").help("base commit (all zeros = new branch, no-op)"))
        .arg(Arg::positional("head").help("head commit"))
        .arg(Arg::option(&["--repo"]).help("repository root (default: current directory)"))
}

/// Runs `anthology-dev version-bump …`; `args` are the arguments after `version-bump`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    run_in(args, stdin, &Env::from_process(), &cwd)
}

/// `check_version_bump.main(args)`: git runs in `cwd` with exactly `env`.
pub fn run_in(args: &[OsString], stdin: &mut dyn Read, env: &Env, cwd: &Path) -> Output {
    let _ = stdin;
    let parsed = match argparse::parse(&spec(env), args) {
        Ok(p) => p,
        Err(e) => {
            return Output {
                stdout: e.stdout.into_bytes(),
                stderr: e.stderr.into_bytes(),
                code: e.code,
            };
        }
    };
    let base = parsed
        .os_str("base")
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let head = parsed
        .os_str("head")
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let repo = parsed
        .os_str("repo")
        .map_or_else(|| OsString::from("."), OsStr::to_os_string);
    let git = Git {
        env,
        cwd,
        repo: &repo,
    };
    let mut stdout = String::new();
    match gate(&base, &head, &git, &mut stdout) {
        Ok(code) => Output {
            stdout: stdout.into_bytes(),
            code,
            ..Output::default()
        },
        Err(Stop::Gate(e)) => Output {
            stdout: stdout.into_bytes(),
            stderr: format!("version gate: {e}\n").into_bytes(),
            code: 2,
        },
        Err(Stop::Uncaught(u)) => u.into_output("version-bump", stdout.into_bytes()),
    }
}
