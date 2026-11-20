//! `anthology-dev release-edit` (scripts/release_edit.py). Owner: DV2.

use std::ffi::OsString;
use std::io::Read;

use crate::Output;

/// Runs `anthology-dev release-edit …`; `args` are the arguments after `release-edit`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let _ = stdin;
    Output::not_ported_sub(
        "release-edit",
        &[
            "plugin-version",
            "check-version",
            "check-dependents",
            "changelog-check",
            "changelog-move",
            "set-version",
            "set-source-version",
        ],
        args,
    )
}
