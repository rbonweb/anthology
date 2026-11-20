//! `anthology-dev lock`: anthology.lock (GC-17, design §8.5). Owner: RL1.

use std::ffi::OsString;
use std::io::Read;

use crate::Output;

/// Runs `anthology-dev lock …`; `args` are the arguments after `lock`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let _ = stdin;
    Output::not_ported_sub("lock", &["write", "check", "check-fresh"], args)
}
