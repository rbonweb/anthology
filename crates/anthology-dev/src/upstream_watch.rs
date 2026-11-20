//! `anthology-dev upstream-watch` (scripts/upstream_watch.py). Owner: DV3.

use std::ffi::OsString;
use std::io::Read;

use crate::Output;

/// Runs `anthology-dev upstream-watch …`; `args` are the arguments after `upstream-watch`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let _ = stdin;
    let _ = args;
    Output::not_ported("upstream-watch")
}
