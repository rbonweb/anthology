//! The walk of a tree on disk (Task 2): never follows a symlink, never reads outside the root.

use std::io;
use std::path::{Path, PathBuf};

/// The limits of a walk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WalkOpts {
    pub max_files: usize,
    pub max_bytes: u64,
    pub text_cap: usize,
    pub all: bool,
}

impl Default for WalkOpts {
    fn default() -> WalkOpts {
        WalkOpts {
            max_files: 100_000,
            max_bytes: 512 << 20,
            text_cap: 2 << 20,
            all: false,
        }
    }
}

/// What an entry of the tree is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntryKind {
    File { exec: bool },
    Symlink { target: String },
    Submodule,
    Other,
}

/// One entry of the tree: a root-relative `/` path, its kind and its size in bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub kind: EntryKind,
    pub size: u64,
}

/// A tree on disk, opened once and read through `read`.
#[derive(Debug)]
pub struct DiskTree {
    root: PathBuf,
    entries: Vec<Entry>,
}

impl DiskTree {
    /// Walks `root`. `modes` are the git modes of a fetched repo (`ls-files` path and mode); they
    /// override the kinds the disk shows. The stub refuses.
    pub fn open(
        _root: &Path,
        _opts: &WalkOpts,
        _modes: Option<&[(String, u32)]>,
    ) -> io::Result<DiskTree> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "not implemented yet",
        ))
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Up to `max` bytes of the file at the root-relative `path`. The stub refuses.
    pub fn read(&self, _path: &str, _max: usize) -> io::Result<Vec<u8>> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "not implemented yet",
        ))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walk_opts_default_to_the_plan_caps() {
        let o = WalkOpts::default();
        assert_eq!(o.max_files, 100_000);
        assert_eq!(o.max_bytes, 512 * 1024 * 1024);
        assert_eq!(o.text_cap, 2 * 1024 * 1024);
        assert!(!o.all);
    }
}
