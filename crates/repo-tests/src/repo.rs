//! The repository root and its files (F1, minimal; m-R11). BS1 and RL1 use it as is; MT1
//! extends it.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The repository root: two levels above this crate's manifest.
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `rel` (a `/`-separated path) under the repository root.
pub fn path(rel: &str) -> PathBuf {
    root().join(rel)
}

/// The UTF-8 text of `rel`; panics with the path when it cannot be read.
pub fn read(rel: &str) -> String {
    let p = path(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// The tracked paths (`git ls-files`), or `None` when git cannot list them here: no git, or a
/// linked worktree whose git directory is outside the container.
pub fn tracked_files() -> Option<Vec<String>> {
    let out = Command::new("git")
        .args(["-c", "core.fsmonitor=false", "ls-files", "-z"])
        .current_dir(root())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let listing = String::from_utf8_lossy(&out.stdout);
    Some(
        listing
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_root_holds_the_workspace_and_rtk() {
        assert!(path("Cargo.toml").is_file());
        assert!(path("rtk/Cargo.toml").is_file());
        assert!(read("rust-toolchain.toml").contains("[toolchain]"));
    }
}
