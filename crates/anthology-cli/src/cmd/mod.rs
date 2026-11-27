//! Command dispatch (plan §1.3). Every command is registered here; its lane fills `run` in
//! `cmd/<name>.rs` and never edits this file. Until then the command exits 70 with
//! `anthology: <command> is not ported yet`.
//!
//! F3 adds one `pub fn env() -> anthology_core::Env` here (the process environment, `HOME` from
//! the environment or else the passwd entry; GC-2), so every command builds its `Env` the same way.

pub mod atl_install;
pub mod doctor;
pub mod guard;
pub mod hook;
pub mod mem;
pub mod memory;
pub mod perm;
pub mod setup;
pub mod switch;

use std::ffi::OsString;
use std::io::Read;

/// What a command produces. `main` writes `stdout`, then `stderr`, then exits with `code`.
#[derive(Debug, Default)]
pub struct Output {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub code: i32,
}

// The stub helpers stay compiled after the last stub is gone, so no lane has to edit this file.
#[allow(dead_code)]
impl Output {
    /// Exit 70 for a registered command whose lane has not landed yet.
    pub fn not_ported(command: &str) -> Output {
        Output {
            stderr: format!("anthology: {command} is not ported yet\n").into_bytes(),
            code: 70,
            ..Output::default()
        }
    }

    /// Exit 2 with a usage message.
    pub fn usage(message: &str) -> Output {
        Output {
            stderr: format!("anthology: {message}\n{USAGE}").into_bytes(),
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

const USAGE: &str = "usage: anthology <command> [args]
commands:
  guard hook [--shim]
  hook session-start
  setup preview|apply|remove
  doctor
  atl-install
  mem search|related|orphans|stale|rebuild|save
  memory export|import
  perm classify
  switch routing_guide|delegation <folder>
  --version
";

/// `anthology <version> (<target>)`.
pub fn version_line() -> String {
    format!(
        "anthology {} ({})\n",
        env!("CARGO_PKG_VERSION"),
        env!("ANTHOLOGY_TARGET")
    )
}

/// Runs the command `args` names (`args` excludes the program name).
pub fn dispatch(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let Some(first) = args.first() else {
        return Output::usage("missing command");
    };
    let rest = &args[1..];
    match first.to_str() {
        Some("--version") => Output {
            stdout: version_line().into_bytes(),
            ..Output::default()
        },
        Some("--help" | "-h") => Output {
            stdout: USAGE.as_bytes().to_vec(),
            ..Output::default()
        },
        Some("guard") => guard::run(rest, stdin),
        Some("hook") => hook::run(rest, stdin),
        Some("setup") => setup::run(rest, stdin),
        Some("doctor") => doctor::run(rest, stdin),
        Some("atl-install") => atl_install::run(rest, stdin),
        Some("mem") => mem::run(rest, stdin),
        Some("memory") => memory::run(rest, stdin),
        Some("perm") => perm::run(rest, stdin),
        Some("switch") => switch::run(rest, stdin),
        _ => Output::usage(&format!("unknown command: {}", first.to_string_lossy())),
    }
}
