//! `anthology-dev smoke` (evals/smoke/smoke.py): the multi-session, no-model and tool-environment
//! checks `claude plugin eval` cannot do. Owner: EV1.
//!
//! Usage: `smoke [--claude BIN] [--anthology-bin BIN] [--budget-usd X] [--total-budget-usd X]
//! [--out PATH] [--list] [--prepare-evals DIR] [NAME ...]`.
//! Each smoke returns PASS, FAIL or SKIP with a one-line reason; one line per smoke is printed as
//! it finishes, a JSON summary goes to `--out` (rewritten after every smoke), and the exit status
//! is 1 if any smoke FAILED, else 0. Bad usage exits 2. Spend, isolation and the scrub rules are
//! those of smoke.py (see `runner`).
//!
//! # Seams (plan GC-19, row `dev::smoke`)
//!
//! Every smoke body reaches the outside world through [`runner::Deps`], held by the
//! [`runner::Ctx`]: `run` (a subprocess), `git`, `make_repo`, `root` (smoke.py's `ROOT`),
//! `default_claude`, `approved_repo`, `prompt_run` and `claude_run`, plus EV1b's `stage`,
//! `seed` and `default_anthology`. `Deps::default()` is production; `tests/smoke.rs` replaces
//! exactly the field the Python test patched with `mock.patch`. The budget accounting lives in
//! the production `prompt_run`, so the tests with a stub `claude` (an `sh` script) exercise it
//! for real. The environment is a parameter, never the process's: [`run_with`] takes the
//! [`anthology_core::Env`], and [`run`] reads it once.
//!
//! # The plugins under test (EV1b)
//!
//! The smokes load a staged copy of the plugins (`plugins/` with `port/staging/plugins/` over
//! it, the Python `scripts/` left out) and a seeded `anthology` binary: `--anthology-bin`, else
//! the `anthology` beside this `anthology-dev`. Before every `claude` run that loads them, the
//! binary is seeded (`anthology-dev seed`) into each config dir the hooks may use. The live
//! Python plugins stay covered by smoke.py until CO2.
//!
//! `--prepare-evals DIR` builds the workspace `claude plugin eval` runs on (run-evals.sh): the
//! marketplace, `evals/` and the staged plugins, seeded. Probe U10: `plugin eval` gives hooks a
//! fresh config dir of its own, so a caller's seeded config dir is never seen. Each SessionStart
//! hook of anthology and anthology-guardrails is therefore wrapped by `eval-seed/eval-seed.sh`
//! (in the copy only), which first copies the seeded archive from `CLAUDE_PLUGIN_ROOT` to
//! `<config>/anthology/bin/` (the bootstrap's offline seed), then runs the hook unchanged.
//!
//! # Differences from smoke.py
//!
//! - `context-cap` ships a POSIX `sh` hook, not a `python3` one (the smoke needs no Python).
//! - `guard-timeout` is new (m-8); `--list` shows it after the twelve original names, then the
//!   EV1b smokes (`binary-missing`, `live-*`).
//! - `approved_repo` approves with MS1's `approve` in process, and `rtk-rewrite` installs rtk
//!   with `anthology rtk-install`; smoke.py ran the Python scripts for both.
//! - Progress lines go to the progress writer as each smoke finishes (`run` writes them to
//!   stdout; [`run_with`] collects them into the [`Output`] for the tests).

pub mod approve;
pub mod cases;
pub mod compat;
pub mod evals_ws;
pub mod helpers;
pub mod runner;
pub mod staged;

use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anthology_core::Env;
use anthology_pycompat::json::{self, Value};

use self::helpers::{Outcome, Status, jobj, jstr, positive};
use self::runner::{Ctx, Deps, SmokeFn, run_smoke};
use crate::Output;

/// One smoke: its name, body and whether it needs model credentials.
pub struct SmokeDef {
    pub name: &'static str,
    pub run: SmokeFn,
    pub needs_model: bool,
    /// Runs `claude` at all (else the claude binary check is skipped).
    pub needs_claude: bool,
    /// Loads the staged plugins with the seeded `anthology` binary (EV1b).
    pub needs_anthology: bool,
}

/// Every smoke in `--list` order: the twelve of smoke.py, then `guard-timeout` (EV1a), then the
/// EV1b smokes: `binary-missing` (F-13) and the live smokes of the seeded `anthology` binary.
pub const SMOKES: [SmokeDef; 19] = [
    SmokeDef {
        name: "cli-syntax",
        run: cases::cli_syntax::run,
        needs_model: false,
        needs_claude: true,
        needs_anthology: false,
    },
    SmokeDef {
        name: "eval-ceiling",
        run: cases::eval_ceiling::run,
        needs_model: true,
        needs_claude: true,
        needs_anthology: false,
    },
    SmokeDef {
        name: "context-cap",
        run: cases::context_cap::run,
        needs_model: true,
        needs_claude: true,
        needs_anthology: false,
    },
    SmokeDef {
        name: "missing-interpreter",
        run: cases::missing_interpreter::run,
        needs_model: true,
        needs_claude: true,
        needs_anthology: false,
    },
    SmokeDef {
        name: "python-missing",
        run: cases::python_missing::run,
        needs_model: false,
        needs_claude: false,
        needs_anthology: false,
    },
    SmokeDef {
        name: "worktree-base",
        run: cases::worktree_base::run,
        needs_model: true,
        needs_claude: true,
        needs_anthology: true,
    },
    SmokeDef {
        name: "model-check",
        run: cases::model_check::run,
        needs_model: true,
        needs_claude: true,
        needs_anthology: true,
    },
    SmokeDef {
        name: "knowledge-read-writers",
        run: cases::knowledge_read_writers::run,
        needs_model: true,
        needs_claude: true,
        needs_anthology: true,
    },
    SmokeDef {
        name: "auto-memory-dir",
        run: cases::auto_memory_dir::run,
        needs_model: true,
        needs_claude: true,
        needs_anthology: true,
    },
    SmokeDef {
        name: "writer-memory-commit",
        run: cases::writer_memory_commit::run,
        needs_model: true,
        needs_claude: true,
        needs_anthology: true,
    },
    SmokeDef {
        name: "rtk-rewrite",
        run: cases::rtk_rewrite::run,
        needs_model: true,
        needs_claude: true,
        needs_anthology: true,
    },
    SmokeDef {
        name: "agent-memory-path",
        run: cases::agent_memory_path::run,
        needs_model: true,
        needs_claude: true,
        needs_anthology: true,
    },
    SmokeDef {
        name: "guard-timeout",
        run: cases::guard_timeout::run,
        needs_model: true,
        needs_claude: true,
        needs_anthology: false,
    },
    SmokeDef {
        name: "binary-missing",
        run: cases::binary_missing::run,
        needs_model: false,
        needs_claude: false,
        needs_anthology: false,
    },
    SmokeDef {
        name: "live-seed",
        run: cases::live_seed::run,
        needs_model: false,
        needs_claude: false,
        needs_anthology: true,
    },
    SmokeDef {
        name: "live-setup",
        run: cases::live_setup::run,
        needs_model: false,
        needs_claude: false,
        needs_anthology: true,
    },
    SmokeDef {
        name: "live-mem",
        run: cases::live_mem::run,
        needs_model: false,
        needs_claude: false,
        needs_anthology: true,
    },
    SmokeDef {
        name: "live-session-start",
        run: cases::live_session_start::run,
        needs_model: true,
        needs_claude: true,
        needs_anthology: true,
    },
    SmokeDef {
        name: "live-guard",
        run: cases::live_guard::run,
        needs_model: true,
        needs_claude: true,
        needs_anthology: true,
    },
];

/// The smoke names, in order.
pub fn names() -> Vec<&'static str> {
    SMOKES.iter().map(|d| d.name).collect()
}

/// Runs `anthology-dev smoke …`; `args` are the arguments after `smoke`. The progress lines go
/// to stdout as each smoke finishes, so a killed run keeps what it printed (EV1a I-1).
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let _ = stdin;
    let stdout = std::io::stdout();
    let mut progress = stdout.lock();
    run_to(args, Env::from_process(), Deps::default(), &mut progress)
}

// ---- argument parsing (argparse semantics of smoke.py's main) ----------------------------

const PROG: &str = "anthology-dev smoke";
const USAGE: &str = "usage: anthology-dev smoke [-h] [--claude CLAUDE] [--anthology-bin ANTHOLOGY_BIN]\n\
                     \x20                         [--budget-usd BUDGET_USD]\n\
                     \x20                         [--total-budget-usd TOTAL_BUDGET_USD] [--out OUT] [--list]\n\
                     \x20                         [--prepare-evals PREPARE_EVALS]\n\
                     \x20                         [NAME ...]\n";
const HELP: &str = "\nAnthology smoke cases U1-U9\n\npositional arguments:\n  NAME\n\noptions:\n\
\x20 -h, --help            show this help message and exit\n\
\x20 --claude CLAUDE       claude binary (default $CLAUDE_BIN, the pinned CI one, else claude)\n\
\x20 --anthology-bin ANTHOLOGY_BIN\n\
\x20                       the anthology binary the plugins run (default: the one beside\n\
\x20                       anthology-dev)\n\
\x20 --budget-usd BUDGET_USD\n\
\x20                       --max-budget-usd for every claude -p (default 1.00)\n\
\x20 --total-budget-usd TOTAL_BUDGET_USD\n\
\x20                       stop launching claude runs once this much has been spent (default 10)\n\
\x20 --out OUT             write a JSON summary here\n\
\x20 --list                print the smoke names and exit\n\
\x20 --prepare-evals PREPARE_EVALS\n\
\x20                       build the seeded `claude plugin eval` workspace in this new\n\
\x20                       folder, print its path and exit\n";
const LONG_OPTIONS: [&str; 8] = [
    "help",
    "claude",
    "anthology-bin",
    "budget-usd",
    "total-budget-usd",
    "out",
    "list",
    "prepare-evals",
];

#[derive(Debug)]
struct Args {
    claude: Option<String>,
    anthology_bin: Option<String>,
    prepare_evals: Option<String>,
    budget_usd: String,
    total_budget_usd: f64,
    out: Option<String>,
    list: bool,
    help: bool,
    names: Vec<String>,
}

/// `argparse`'s test for "this argument is a value, not an option string".
fn looks_like_value(arg: &std::ffi::OsStr) -> bool {
    let Some(s) = arg.to_str() else {
        return true;
    };
    if !s.starts_with('-') || s == "-" {
        return true;
    }
    let negative_number = {
        let digits = s[1..].trim_start_matches(|c: char| c.is_ascii_digit());
        let whole = s.len() - 1 - digits.len();
        let fraction_ok = match digits.strip_prefix('.') {
            Some(frac) => !frac.is_empty() && frac.chars().all(|c| c.is_ascii_digit()),
            None => digits.is_empty(),
        };
        (whole > 0 || digits.starts_with('.')) && fraction_ok
    };
    negative_number || s.contains(' ')
}

/// The unique long option `name` stands for (an exact name, else a unique prefix).
fn resolve_long(name: &str) -> Result<&'static str, String> {
    if let Some(exact) = LONG_OPTIONS.iter().find(|o| **o == name) {
        return Ok(exact);
    }
    let matches: Vec<&&str> = LONG_OPTIONS
        .iter()
        .filter(|o| o.starts_with(name))
        .collect();
    match matches.as_slice() {
        [one] => Ok(one),
        [] => Err(format!("unrecognized arguments: --{name}")),
        many => Err(format!(
            "ambiguous option: --{name} could match {}",
            many.iter()
                .map(|o| format!("--{o}"))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn parse_args(args: &[OsString]) -> Result<Args, String> {
    use lexopt::Arg::{Long, Short, Value};
    let mut out = Args {
        claude: None,
        anthology_bin: None,
        prepare_evals: None,
        budget_usd: "1.00".to_string(),
        total_budget_usd: 10.0,
        out: None,
        list: false,
        help: false,
        names: Vec::new(),
    };
    let mut extras: Vec<String> = Vec::new();
    // argparse reads the positionals of one uninterrupted group: a later one is "unrecognized".
    let mut group_closed = false;
    let mut parser = lexopt::Parser::from_args(args.iter().cloned());
    let text = |v: OsString| v.into_string().map_err(|v| format!("invalid UTF-8: {v:?}"));
    loop {
        let arg = match parser.next() {
            Ok(Some(arg)) => arg,
            Ok(None) => break,
            Err(e) => return Err(e.to_string()),
        };
        let long = match arg {
            Value(v) => {
                let v = text(v)?;
                if group_closed {
                    extras.push(v);
                } else {
                    out.names.push(v);
                }
                continue;
            }
            Short('h') => "help",
            Short(c) => {
                extras.push(format!("-{c}"));
                group_closed = group_closed || !out.names.is_empty();
                continue;
            }
            Long(name) => match resolve_long(name) {
                Ok(long) => long,
                Err(e) if e.starts_with("unrecognized") => {
                    extras.push(format!("--{name}"));
                    group_closed = group_closed || !out.names.is_empty();
                    continue;
                }
                Err(e) => return Err(e),
            },
        };
        group_closed = group_closed || !out.names.is_empty();
        let mut value = || -> Result<String, String> {
            if let Some(v) = parser.optional_value() {
                return text(v);
            }
            let mut raw = parser.raw_args().map_err(|e| e.to_string())?;
            match raw.next_if(looks_like_value) {
                Some(v) => text(v),
                None => Err(format!("argument --{long}: expected one argument")),
            }
        };
        match long {
            // argparse's help action exits at once: later arguments are never looked at.
            "help" => {
                out.help = true;
                return Ok(out);
            }
            "list" => out.list = true,
            "claude" => out.claude = Some(value()?),
            "anthology-bin" => out.anthology_bin = Some(value()?),
            "prepare-evals" => out.prepare_evals = Some(value()?),
            "out" => out.out = Some(value()?),
            "budget-usd" => out.budget_usd = value()?,
            _ => {
                let v = value()?;
                out.total_budget_usd =
                    positive(&v).map_err(|e| format!("argument --total-budget-usd: {e}"))?;
            }
        }
    }
    if extras.is_empty() {
        Ok(out)
    } else {
        Err(format!("unrecognized arguments: {}", extras.join(" ")))
    }
}

fn usage_error(message: &str) -> Output {
    Output {
        stderr: format!("{USAGE}{PROG}: error: {message}\n").into_bytes(),
        code: 2,
        ..Output::default()
    }
}

/// `os.path.abspath`: against the working directory, with `.`, `..` and `//` collapsed.
fn abspath(path: &str, cwd: &Path) -> String {
    let full: PathBuf = if path.starts_with('/') {
        PathBuf::from(path)
    } else {
        cwd.join(path)
    };
    let mut parts: Vec<&str> = Vec::new();
    let text = full.to_string_lossy().into_owned();
    for part in text.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    format!("/{}", parts.join("/"))
}

/// `main()` with the environment and the seams given, the progress lines collected into the
/// returned `Output.stdout` (the tests' form of [`run_to`]).
pub fn run_with(args: &[OsString], env: Env, deps: Deps) -> Output {
    let mut progress: Vec<u8> = Vec::new();
    let mut out = run_to(args, env, deps, &mut progress);
    progress.extend_from_slice(&out.stdout);
    out.stdout = progress;
    out
}

/// `main()` with the environment and the seams given: each progress line is written to
/// `progress` (and flushed) as its smoke finishes, and `--out` is rewritten after each smoke;
/// the returned `Output` carries the rest (help, `--list`, usage errors).
pub fn run_to(args: &[OsString], env: Env, deps: Deps, progress: &mut dyn Write) -> Output {
    let args = match parse_args(args) {
        Ok(a) => a,
        Err(message) => return usage_error(&message),
    };
    if args.help {
        return Output {
            stdout: format!("{USAGE}{HELP}").into_bytes(),
            ..Output::default()
        };
    }
    if args.list {
        let mut text = names().join("\n");
        text.push('\n');
        return Output {
            stdout: text.into_bytes(),
            ..Output::default()
        };
    }
    if let Err(e) = positive(&args.budget_usd) {
        return usage_error(&format!("--budget-usd {e}"));
    }
    let explicit = args.claude.as_deref().is_some_and(|c| !c.is_empty())
        || env.get("CLAUDE_BIN").is_some_and(|b| !b.is_empty());
    let root = (deps.root)().ok();
    let mut claude = match args.claude.as_deref() {
        Some(c) if !c.is_empty() => c.to_string(),
        _ => (deps.default_claude)(&env, root.as_deref()),
    };
    if claude.contains('/') {
        // Runs happen in other directories, so a path must be absolute.
        let cwd = (deps.cwd)();
        claude = abspath(&claude, &cwd);
    }
    let known = names();
    let unknown: Vec<&str> = args
        .names
        .iter()
        .map(String::as_str)
        .filter(|n| !known.contains(n))
        .collect();
    if !unknown.is_empty() {
        return usage_error(&format!(
            "unknown smoke(s): {}; known: {}",
            unknown.join(", "),
            known.join(", ")
        ));
    }
    let anthology_explicit = args.anthology_bin.as_deref().is_some_and(|b| !b.is_empty());
    let anthology = match args.anthology_bin.as_deref() {
        Some(b) if !b.is_empty() => Some(abspath(b, &(deps.cwd)())),
        _ => (deps.default_anthology)().map(|p| p.to_string_lossy().into_owned()),
    };
    let ctx = Ctx::new(&claude, &args.budget_usd, args.total_budget_usd, env, deps)
        .with_anthology(anthology, anthology_explicit);
    if let Some(dir) = args.prepare_evals.as_deref() {
        return match evals_ws::prepare(&ctx, Path::new(&abspath(dir, &(ctx.deps.cwd)()))) {
            Ok(path) => Output {
                stdout: format!("{}\n", path.display()).into_bytes(),
                ..Output::default()
            },
            Err(e) => Output {
                stderr: format!("{PROG}: {e}\n").into_bytes(),
                code: 1,
                ..Output::default()
            },
        };
    }
    let selected: Vec<&str> = if args.names.is_empty() {
        known
    } else {
        args.names.iter().map(String::as_str).collect()
    };
    let out_path = args.out.as_ref().filter(|p| !p.is_empty());
    let mut out_error: Option<String> = None;
    let mut results: Vec<(&str, Outcome)> = Vec::new();
    for name in selected {
        let (status, reason) = run_smoke(name, &ctx, explicit);
        // One write per line, so a line is never split between writers.
        let line = format!("{} {name}: {reason}\n", status.as_str());
        let _ = progress.write_all(line.as_bytes());
        let _ = progress.flush();
        results.push((name, (status, reason)));
        // The summary so far: a run killed later still leaves the finished smokes behind.
        if let Some(path) = out_path
            && out_error.is_none()
        {
            out_error = write_summary(path, &results).err();
        }
    }
    let spend = format!("smoke spend: ${:.2} of ${:.2}\n", ctx.spent(), ctx.total);
    let _ = progress.write_all(spend.as_bytes());
    let _ = progress.flush();
    // Python's `results` dict keeps the last outcome of a repeated name.
    let failed = results.iter().enumerate().any(|(i, (name, (s, _)))| {
        *s == Status::Fail && !results[i + 1..].iter().any(|(n, _)| n == name)
    });
    let mut output = Output {
        code: i32::from(failed),
        ..Output::default()
    };
    if let Some(path) = out_path
        && out_error.is_none()
    {
        out_error = write_summary(path, &results).err();
    }
    if let Some(e) = out_error {
        output
            .stderr
            .extend_from_slice(format!("{PROG}: {e}\n").as_bytes());
        output.code = 1;
    }
    output
}

/// `json.dumps({"smokes": results}, indent=2) + "\n"` to `path`, its folder made first.
fn write_summary(path: &str, results: &[(&str, Outcome)]) -> Result<(), String> {
    let mut smokes = json::Map::new();
    for (name, (status, reason)) in results {
        smokes.insert(
            (*name).to_string(),
            jobj(vec![
                ("status", jstr(status.as_str())),
                ("reason", jstr(reason)),
            ]),
        );
    }
    let doc = jobj(vec![("smokes", Value::Object(smokes))]);
    let mut text = json::dumps(&doc, &json::INDENT2);
    text.push('\n');
    let path = Path::new(path);
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|e| format!("OSError: {}: {e}", parent.display()))?;
    }
    fs::write(path, text).map_err(|e| format!("OSError: {}: {e}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_and_options_are_told_apart_like_argparse() {
        let v = |s: &str| looks_like_value(std::ffi::OsStr::new(s));
        assert!(v("abc") && v("-") && v("-1") && v("-.5") && v("-1.5") && v("-a b"));
        assert!(!v("--list") && !v("-x") && !v("-1x") && !v("--"));
    }

    #[test]
    fn a_prefix_stands_for_the_unique_option() {
        assert_eq!(resolve_long("cl"), Ok("claude"));
        assert_eq!(resolve_long("total"), Ok("total-budget-usd"));
        assert!(resolve_long("b").is_ok());
        assert!(resolve_long("t").is_ok());
        assert!(resolve_long("zz").is_err());
        assert_eq!(resolve_long("help"), Ok("help"));
    }

    #[test]
    fn abspath_collapses_dots() {
        assert_eq!(abspath("a/../b/./c", Path::new("/w")), "/w/b/c");
        assert_eq!(abspath("/x//y/..", Path::new("/w")), "/x");
    }

    #[test]
    fn the_names_are_the_twelve_then_guard_timeout_then_ev1b() {
        let n = names();
        assert_eq!(n.len(), 19);
        assert_eq!(n[0], "cli-syntax");
        assert_eq!(n[11], "agent-memory-path");
        assert_eq!(n[12], "guard-timeout");
        assert_eq!(
            n[13..],
            [
                "binary-missing",
                "live-seed",
                "live-setup",
                "live-mem",
                "live-session-start",
                "live-guard"
            ]
        );
    }
}
