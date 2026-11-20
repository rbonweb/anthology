//! `anthology doctor [--json]` (GC-11). Owner: RT1b.

use std::ffi::OsString;
use std::io::Read;

use super::Output;

/// Runs `anthology doctor …`; `args` are the arguments after `doctor`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let _ = stdin;
    let _ = args;
    Output::not_ported("doctor")
}
