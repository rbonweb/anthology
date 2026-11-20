//! `anthology-dev golden`: the frozen difftest goldens. Owner: CO2.

use std::ffi::OsString;
use std::io::Read;

use crate::Output;

/// Runs `anthology-dev golden …`; `args` are the arguments after `golden`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let _ = stdin;
    Output::not_ported_sub("golden", &["bless", "check"], args)
}
