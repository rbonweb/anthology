//! `anthology-dev vendor`: the vendored trees (scripts/vendor.py). Owner: DV1.

pub mod build;
pub mod compare;
pub mod diff;
pub mod fetch;
pub mod licenses;
pub mod manifest;
pub mod patch;
pub mod portmap;
pub mod set_rtk_upstream;
pub mod sync;
pub mod transforms;

use std::ffi::OsString;
use std::io::Read;

use crate::Output;

/// Runs `anthology-dev vendor …`; `args` are the arguments after `vendor`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let _ = stdin;
    Output::not_ported_sub(
        "vendor",
        &[
            "fetch",
            "build",
            "check",
            "sync",
            "diff",
            "set-rtk-upstream",
        ],
        args,
    )
}
