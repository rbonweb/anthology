//! `anthology-dev version-bump` (scripts/check_version_bump.py): a plugin whose files changed
//! must carry a higher version. Owner: DV2.
//!
//! Only released plugins are gated: a plugin counts as released once a git tag `<plugin>--v*`
//! exists. The comparison is made against `git merge-base <base> <head>`.
//!
//! Exit codes: 0 pass (or nothing to check), 1 a plugin changed without a valid bump, 2 the refs
//! could not be resolved or share no history (and 2 for a usage error).

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anthology_core::Env;
use anthology_pycompat::json::{self, Value};
use anthology_pycompat::pyerr::PyExc;

use crate::Output;
use crate::json_edit::{Uncaught, py_repr, universal_newlines, utf8_decode_error};
use crate::memory_diff::pystr;
use crate::release_edit::argp::{self, OptSpec, Parser, PosSpec};
use crate::semver;

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
                pystr::strip(&r.stderr)
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
        Ok(pystr::strip(&r.stdout).to_string())
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

    /// `_released_plugins`: the plugins with a `<plugin>--v*` tag (matched by prefix, not by git
    /// glob, so a plugin name can never be read as a pattern).
    fn released_plugins(&self, plugins: &[String]) -> Result<BTreeSet<String>, Stop> {
        let listing = self.ok(&["for-each-ref", "--format=%(refname:strip=2)", "refs/tags"])?;
        let tags = pystr::splitlines(&listing, false);
        Ok(plugins
            .iter()
            .filter(|p| {
                let prefix = format!("{p}--v");
                tags.iter().any(|t| t.starts_with(&prefix))
            })
            .cloned()
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
    if mb.code != 0 || pystr::strip(&mb.stdout).is_empty() {
        return Err(Stop::Gate(format!(
            "no merge base between {base} and {head}"
        )));
    }
    let mb_c = pystr::strip(&mb.stdout).to_string();
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
    let released = git.released_plugins(&plugins)?;
    let mut failed = false;
    for plugin in &plugins {
        if !released.contains(plugin) {
            out.push_str(&format!(
                "{plugin}: unreleased (no {plugin}--v* tag) \u{2014} version bump not required\n"
            ));
            continue;
        }
        let (base_exists, base_version) = git.plugin_version_at(&mb_c, plugin)?;
        let (head_exists, head_version) = git.plugin_version_at(&head_c, plugin)?;
        let (ok, reason) = verdict(
            plugin,
            &base_version,
            &head_version,
            base_exists,
            head_exists,
        );
        out.push_str(&format!("{} {reason}\n", if ok { "ok  " } else { "FAIL" }));
        failed = failed || !ok;
    }
    Ok(i32::from(failed))
}

// ---- command line --------------------------------------------------------------------

const PROG: &str = "anthology-dev version-bump";
const USAGE: &str = "usage: anthology-dev version-bump [-h] [--repo REPO] base head";
const OPTS: [OptSpec; 1] = [OptSpec {
    long: "--repo",
    dest: "repo",
}];
const POS: [PosSpec; 2] = [
    PosSpec {
        name: "base",
        choices: &[],
    },
    PosSpec {
        name: "head",
        choices: &[],
    },
];

/// Runs `anthology-dev version-bump …`; `args` are the arguments after `version-bump`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    run_in(args, stdin, &Env::from_process(), &cwd)
}

/// `check_version_bump.main(args)`: git runs in `cwd` with exactly `env`.
pub fn run_in(args: &[OsString], stdin: &mut dyn Read, env: &Env, cwd: &Path) -> Output {
    let _ = stdin;
    let parser = Parser {
        prog: PROG,
        usage: USAGE,
        opts: &OPTS,
        pos: &POS,
    };
    let parsed = match argp::parse_args(&parser, args) {
        Ok(p) => p,
        Err(out) => return out,
    };
    let base = parsed.pos[0].to_string_lossy().into_owned();
    let head = parsed.pos[1].to_string_lossy().into_owned();
    let repo = parsed
        .opt("repo")
        .cloned()
        .unwrap_or_else(|| OsString::from("."));
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
