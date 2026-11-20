//! `anthology-dev seed`: a plugin tree with a seeded binary. Owner: RL1.

use std::ffi::OsString;
use std::io::Read;

use crate::Output;

/// Runs `anthology-dev seed …`; `args` are the arguments after `seed`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let _ = stdin;
    let _ = args;
    Output::not_ported("seed")
}
