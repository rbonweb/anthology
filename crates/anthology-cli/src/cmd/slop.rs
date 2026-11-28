//! `anthology slop hook|check|rules`: the slop gate (plan
//! docs/superpowers/plans/2026-11-28-anti-slop-gate.md, Task 7 fills it in).
//!
//! - `hook`: stdin is the Claude Code PreToolUse JSON. Exit 0 allows (no output); exit
//!   `HOOK_BLOCK` blocks, with the message on stderr.
//! - `check [--root DIR] [--since REF] [PATH...]`: one line per unwaived finding, then one per
//!   waiver in the new text. Exit 0 none unwaived, 1 some, 2 a usage or git error.
//! - `rules`: one tab-separated line per rule.

use std::ffi::OsString;
use std::io::Read;

use super::Output;

/// The exit code of `slop hook` that blocks the edit; the hook script turns it into exit 2.
#[allow(dead_code)] // the hook of Task 7 returns it
pub const HOOK_BLOCK: i32 = 10;

/// Runs `anthology slop …`; `args` are the arguments after `slop`.
pub fn run(args: &[OsString], _stdin: &mut dyn Read) -> Output {
    Output::not_ported_sub("slop", &["hook", "check", "rules"], args)
}
