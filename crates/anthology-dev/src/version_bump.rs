//! `anthology-dev version-bump` (scripts/check_version_bump.py). Owner: DV2.

use std::ffi::OsString;
use std::io::Read;

use crate::Output;

/// Runs `anthology-dev version-bump …`; `args` are the arguments after `version-bump`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let _ = stdin;
    let _ = args;
    Output::not_ported("version-bump")
}
