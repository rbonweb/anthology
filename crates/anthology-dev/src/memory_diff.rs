//! `anthology-dev memory-diff` (scripts/memory_diff.py). Owner: DV2.

pub mod pystr;
pub mod unicode;

use std::ffi::OsString;
use std::io::Read;

use crate::Output;

/// Runs `anthology-dev memory-diff …`; `args` are the arguments after `memory-diff`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let _ = stdin;
    let _ = args;
    Output::not_ported("memory-diff")
}
