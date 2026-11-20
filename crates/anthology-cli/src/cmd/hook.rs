//! `anthology hook session-start`: the SessionStart hook (GC-7). Owner: RT3.

use std::ffi::OsString;
use std::io::Read;

use super::Output;

/// Runs `anthology hook …`; `args` are the arguments after `hook`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let _ = stdin;
    Output::not_ported_sub("hook", &["session-start"], args)
}
