//! `anthology-dev gen-errno`: pycompat's errno tables. Owner: P1.

use std::ffi::OsString;
use std::io::Read;

use crate::Output;

/// Runs `anthology-dev gen-errno …`; `args` are the arguments after `gen-errno`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let _ = stdin;
    let _ = args;
    Output::not_ported("gen-errno")
}
