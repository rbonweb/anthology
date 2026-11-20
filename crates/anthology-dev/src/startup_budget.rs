//! `anthology-dev startup-budget` (scripts/startup_budget.py, GC-13). Owner: DV2.

use std::ffi::OsString;
use std::io::Read;

use crate::Output;

/// Runs `anthology-dev startup-budget …`; `args` are the arguments after `startup-budget`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let _ = stdin;
    let _ = args;
    Output::not_ported("startup-budget")
}
