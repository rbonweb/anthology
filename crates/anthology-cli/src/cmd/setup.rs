//! `anthology setup preview|apply|remove` (GC-10). Owner: RT4.

use std::ffi::OsString;
use std::io::Read;

use super::Output;

/// Runs `anthology setup …`; `args` are the arguments after `setup`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let _ = stdin;
    Output::not_ported_sub("setup", &["preview", "apply", "remove"], args)
}
