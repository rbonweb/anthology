//! `anthology-dev`: repository tooling (not shipped). Plan §1.3 lists the commands.
//!
//! A library plus a thin `main.rs`, so each module's tests in `tests/` can call it. Every command
//! is registered in `dispatch`; its lane fills `run` in its own module and never edits this file.
//! Until then the command exits 70 with `anthology-dev: <command> is not ported yet`.
#![forbid(unsafe_code)]

pub mod atl_trim;
pub mod eval_baseline;
pub mod gen_errno;
pub mod gen_unicode;
pub mod golden;
pub mod json_edit;
pub mod ledger;
pub mod lock;
pub mod memory_diff;
pub mod release_edit;
pub mod replay;
pub mod seed;
pub mod semver;
pub mod smoke;
pub mod startup_budget;
pub mod upstream_watch;
pub mod vendor;
pub mod version_bump;

use std::ffi::OsString;
use std::io::Read;

/// What a command produces. `main` writes `stdout`, then `stderr`, then exits with `code`.
#[derive(Debug, Default)]
pub struct Output {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub code: i32,
}

impl Output {
    /// Exit 70 for a registered command whose lane has not landed yet.
    pub fn not_ported(command: &str) -> Output {
        Output {
            stderr: format!("anthology-dev: {command} is not ported yet\n").into_bytes(),
            code: 70,
            ..Output::default()
        }
    }

    /// Exit 2 with a usage message.
    pub fn usage(message: &str) -> Output {
        Output {
            stderr: format!("anthology-dev: {message}\n{USAGE}").into_bytes(),
            code: 2,
            ..Output::default()
        }
    }

    /// The registered-subcommand stub: exit 70 for a known `sub` of `command`, else usage.
    pub fn not_ported_sub(command: &str, subs: &[&str], args: &[OsString]) -> Output {
        match args.first().and_then(|a| a.to_str()) {
            Some(sub) if subs.contains(&sub) => Output::not_ported(&format!("{command} {sub}")),
            Some(other) => Output::usage(&format!("unknown command: {command} {other}")),
            None => Output::usage(&format!("{command} needs one of: {}", subs.join(", "))),
        }
    }
}

const USAGE: &str = "usage: anthology-dev <command> [args]
commands:
  vendor fetch|build|check|sync|diff|set-atl-upstream
  version-bump
  release-edit plugin-version|check-version|check-dependents|changelog-check|changelog-move|set-version|set-source-version
  memory-diff
  startup-budget
  eval-baseline record|compare|candidate
  upstream-watch
  atl-trim
  smoke
  lock write|check|check-fresh
  seed
  ledger check|ids
  golden bless|check
  gen-unicode
  gen-errno
  replay collect|run|report
";

/// Runs the command `args` names (`args` excludes the program name).
pub fn dispatch(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let Some(first) = args.first() else {
        return Output::usage("missing command");
    };
    let rest = &args[1..];
    match first.to_str() {
        Some("--help" | "-h") => Output {
            stdout: USAGE.as_bytes().to_vec(),
            ..Output::default()
        },
        Some("vendor") => vendor::run(rest, stdin),
        Some("version-bump") => version_bump::run(rest, stdin),
        Some("release-edit") => release_edit::run(rest, stdin),
        Some("memory-diff") => memory_diff::run(rest, stdin),
        Some("startup-budget") => startup_budget::run(rest, stdin),
        Some("eval-baseline") => eval_baseline::run(rest, stdin),
        Some("upstream-watch") => upstream_watch::run(rest, stdin),
        Some("atl-trim") => atl_trim::run(rest, stdin),
        Some("smoke") => smoke::run(rest, stdin),
        Some("lock") => lock::run(rest, stdin),
        Some("seed") => seed::run(rest, stdin),
        Some("ledger") => ledger::run(rest, stdin),
        Some("golden") => golden::run(rest, stdin),
        Some("gen-unicode") => gen_unicode::run(rest, stdin),
        Some("gen-errno") => gen_errno::run(rest, stdin),
        Some("replay") => replay::run(rest, stdin),
        _ => Output::usage(&format!("unknown command: {}", first.to_string_lossy())),
    }
}
