//! `anthology-dev replay`: replay of local transcripts (design §13.7). Owner: RP1.

pub mod collect;
pub mod compare;
pub mod envelope;
pub mod report;
pub mod runners;

use std::ffi::OsString;
use std::io::Read;

use crate::Output;

/// Runs `anthology-dev replay …`; `args` are the arguments after `replay`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let _ = stdin;
    Output::not_ported_sub("replay", &["collect", "run", "report"], args)
}
