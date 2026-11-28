//! The quarantine fetch of a URL target (Task 6). The stubs refuse everything.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::tools::Runner;

const STUB: &str = "not implemented yet";

/// What to fetch and where to put it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchPlan {
    pub url: String,
    pub rev: Option<String>,
    pub dest: PathBuf,
}

/// `s` if it is an `https://`, `ssh://` or `git@host:` URL without options, whitespace or
/// control characters.
pub fn validate_url(_s: &str) -> Result<String, String> {
    Err(STUB.to_string())
}

/// `s` if it is a commit hash, a branch or a tag name that cannot be read as an option.
pub fn validate_rev(_s: &str) -> Result<String, String> {
    Err(STUB.to_string())
}

/// The folder name for a fetch of `url` at `epoch`: `<host>-<owner>-<repo>-<epoch>`.
pub fn dest_name(_url: &str, _epoch: u64) -> String {
    String::new()
}

/// The git calls that fetch `p`, in order; `template` is an empty folder for `git init`.
pub fn git_steps(_p: &FetchPlan, _template: &Path) -> Vec<Vec<OsString>> {
    Vec::new()
}

/// The whole environment of a git call: `path` is `PATH`, `home` is `HOME`, `ssh` is
/// `SSH_AUTH_SOCK` when set.
pub fn git_env(_path: &OsStr, _home: &Path, _ssh: Option<&OsStr>) -> Vec<(OsString, OsString)> {
    Vec::new()
}

/// Runs the steps of `p` through `r` within `timeout`, removes `p.dest` on failure and returns
/// the `(path, mode)` pairs of `git ls-files -s`.
pub fn fetch(
    _p: &FetchPlan,
    _r: &dyn Runner,
    _timeout: Duration,
) -> Result<Vec<(String, u32)>, String> {
    Err(STUB.to_string())
}
