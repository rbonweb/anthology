//! `anthology rtk-install` (GC-9). Owner: RT2.

use std::ffi::OsString;
use std::io::Read;

use super::Output;

/// Runs `anthology rtk-install …`; `args` are the arguments after `rtk-install`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let _ = stdin;
    let _ = args;
    Output::not_ported("rtk-install")
}
