//! `anthology-dev smoke` (evals/smoke/smoke.py). Owner: EV1.

pub mod cases;

use std::ffi::OsString;
use std::io::Read;

use crate::Output;

/// Runs `anthology-dev smoke …`; `args` are the arguments after `smoke`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let _ = stdin;
    let _ = args;
    Output::not_ported("smoke")
}
