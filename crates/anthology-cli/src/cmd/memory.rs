//! `anthology memory export|import` (memory_portable). Owner: MX1.

use std::ffi::OsString;
use std::io::Read;

use super::Output;

/// Runs `anthology memory …`; `args` are the arguments after `memory`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let _ = stdin;
    Output::not_ported_sub("memory", &["export", "import"], args)
}
