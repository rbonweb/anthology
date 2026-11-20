//! `anthology-dev gen-unicode`: pycompat's Unicode tables. Owner: P1.

use std::ffi::OsString;
use std::io::Read;

use crate::Output;

/// Runs `anthology-dev gen-unicode …`; `args` are the arguments after `gen-unicode`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let _ = stdin;
    let _ = args;
    Output::not_ported("gen-unicode")
}
