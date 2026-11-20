//! `anthology-dev eval-baseline` (scripts/eval_baseline.py). Owner: DV2.

use std::ffi::OsString;
use std::io::Read;

use crate::Output;

/// Runs `anthology-dev eval-baseline …`; `args` are the arguments after `eval-baseline`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let _ = stdin;
    Output::not_ported_sub("eval-baseline", &["record", "compare", "candidate"], args)
}
