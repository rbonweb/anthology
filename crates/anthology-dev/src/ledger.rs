//! `anthology-dev ledger`: the test ledger (design §13.3). Owner: F4a.

use std::ffi::OsString;
use std::io::Read;

use crate::Output;

/// Runs `anthology-dev ledger …`; `args` are the arguments after `ledger`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let _ = stdin;
    Output::not_ported_sub("ledger", &["check", "ids"], args)
}
