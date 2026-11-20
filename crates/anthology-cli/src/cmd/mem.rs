//! `anthology mem …`: the memory index CLI behind `bin/anthology-mem`. Owner: MI1.

use std::ffi::OsString;
use std::io::Read;

use super::Output;

/// Runs `anthology mem …`; `args` are the arguments after `mem`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let _ = stdin;
    Output::not_ported_sub(
        "mem",
        &["search", "related", "orphans", "stale", "rebuild", "save"],
        args,
    )
}
