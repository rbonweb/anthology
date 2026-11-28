//! Auto-run surfaces: install scripts, editor tasks, devcontainers, build files, git config
//! (Task 9).

use crate::{FileCtx, Finding, Surface};

pub(crate) fn surfaces(_f: &FileCtx, _out: &mut Vec<Finding>) -> Vec<Surface> {
    Vec::new()
}

/// The `(name, command)` pairs of the `scripts` of a `package.json`.
pub(crate) fn npm_scripts(_f: &FileCtx) -> Vec<(String, String)> {
    Vec::new()
}
