//! Which files the gate skips, and how a path is shown (Task 6). Until then nothing is exempt.

use std::path::{Path, PathBuf};

/// A path whose part below the repo root has one of these components is exempt.
pub const EXEMPT_DIRS: [&str; 13] = [
    ".git",
    ".claude",
    ".superpowers",
    "node_modules",
    "vendor",
    "third_party",
    "dist",
    "build",
    ".next",
    "coverage",
    "fixtures",
    "__fixtures__",
    "__snapshots__",
];

/// The nearest folder above `path` that holds `.git` (a folder or a file).
pub fn repo_root(_path: &Path) -> Option<PathBuf> {
    None
}

/// Whether the gate skips `path`.
pub fn exempt(_path: &Path, _root: Option<&Path>, _config_dir: Option<&Path>) -> bool {
    false
}

/// `path` relative to `root` when under it, else as given.
pub fn shown(_path: &Path, _root: Option<&Path>) -> String {
    String::new()
}
