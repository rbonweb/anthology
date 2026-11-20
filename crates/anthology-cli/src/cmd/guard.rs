//! `anthology guard hook`: the PreToolUse guard (GC-5, GC-6). Owner: GD2.

use std::ffi::OsString;
use std::io::Read;

use super::Output;

/// Runs `anthology guard …`; `args` are the arguments after `guard`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let _ = stdin;
    Output::not_ported_sub("guard", &["hook"], args)
}
