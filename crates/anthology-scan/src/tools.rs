//! Optional scanners run on a tree, only when found on `PATH` (Task 6). The stubs run nothing.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::Finding;

/// How the CLI starts a program. The CLI implements it with a cleared environment; the tests
/// implement it with a recorder.
pub trait Runner: Sync {
    /// The path of `name` on `PATH`, if it is there.
    fn find(&self, name: &str) -> Option<PathBuf>;

    /// Runs `prog` in `cwd` with exactly `env`, killed after `timeout`; returns the exit code,
    /// stdout and stderr.
    fn run(
        &self,
        prog: &Path,
        args: &[OsString],
        cwd: &Path,
        env: &[(OsString, OsString)],
        timeout: Duration,
    ) -> io::Result<(i32, Vec<u8>, Vec<u8>)>;
}

/// What became of one tool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolStatus {
    /// Ran and reported this many findings.
    Ran(usize),
    /// Not on `PATH`; the text says what it would add.
    Missing(&'static str),
    Failed(String),
    Skipped(String),
}

/// One tool and its status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolRun {
    pub name: &'static str,
    pub status: ToolStatus,
}

/// The arguments of `tool` for `target`; `scratch` holds the config files and reports.
pub fn tool_argv(_tool: &str, _target: &Path, _scratch: &Path) -> Vec<OsString> {
    Vec::new()
}

/// Runs the tools found by `r` on `target`, in parallel.
pub fn run_tools(_target: &Path, _scratch: &Path, _r: &dyn Runner) -> (Vec<ToolRun>, Vec<Finding>) {
    (Vec::new(), Vec::new())
}
