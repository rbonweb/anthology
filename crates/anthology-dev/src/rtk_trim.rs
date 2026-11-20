//! `anthology-dev rtk-trim` (scripts/rtk_trim.py). Owner: DV4.

use std::ffi::OsString;
use std::io::Read;

use crate::Output;

/// Runs `anthology-dev rtk-trim …`; `args` are the arguments after `rtk-trim`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let _ = stdin;
    let _ = args;
    Output::not_ported("rtk-trim")
}
