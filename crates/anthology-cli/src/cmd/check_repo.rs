//! `anthology check-repo`: a scan of a folder or git URL for malicious code (Task 8 fills this
//! in). Until then the command exits 70 with the stub line.

use std::ffi::OsString;
use std::io::Read;

use super::Output;

pub fn run(_args: &[OsString], _stdin: &mut dyn Read) -> Output {
    Output::not_ported("check-repo")
}
