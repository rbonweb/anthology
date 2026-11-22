//! A one-level look for git repos below a folder that is not itself in one. Shared by the
//! session-start warning and the `anthology-mem` refusal, so both name the same repos.
#![forbid(unsafe_code)]

use std::fs;
use std::path::Path;

/// The most entries of a folder looked at when scanning for repos below it.
pub const REPO_SCAN_CAP: usize = 200;

/// The names, sorted, of the folders directly below `dir` that hold a `.git` (a folder or a
/// file, and a symlinked folder counts). Only the first `cap` entries of `dir` are looked at,
/// in the order the file system lists them. A folder that cannot be read has none.
pub fn repos_below(dir: &Path, cap: usize) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .take(cap)
        .flatten()
        .filter(|entry| entry.path().join(".git").exists())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn it_names_the_folders_with_a_git_entry_sorted() {
        let t = tmp();
        fs::create_dir_all(t.path().join("zeta/.git")).unwrap();
        fs::create_dir_all(t.path().join("alpha/.git")).unwrap();
        fs::create_dir_all(t.path().join("plain")).unwrap();
        fs::write(t.path().join("file.txt"), "x").unwrap();
        assert_eq!(repos_below(t.path(), REPO_SCAN_CAP), ["alpha", "zeta"]);
    }

    #[test]
    fn a_git_file_counts() {
        let t = tmp();
        fs::create_dir(t.path().join("wt")).unwrap();
        fs::write(t.path().join("wt/.git"), "gitdir: /elsewhere\n").unwrap();
        assert_eq!(repos_below(t.path(), REPO_SCAN_CAP), ["wt"]);
    }

    #[test]
    fn a_symlink_to_a_repo_counts() {
        let t = tmp();
        fs::create_dir_all(t.path().join("real/.git")).unwrap();
        fs::create_dir(t.path().join("parent")).unwrap();
        std::os::unix::fs::symlink(t.path().join("real"), t.path().join("parent/linked")).unwrap();
        assert_eq!(
            repos_below(&t.path().join("parent"), REPO_SCAN_CAP),
            ["linked"]
        );
    }

    #[test]
    fn it_looks_at_no_more_than_its_cap() {
        let t = tmp();
        fs::create_dir_all(t.path().join("only/.git")).unwrap();
        assert_eq!(repos_below(t.path(), 1), ["only"]);
        assert!(repos_below(t.path(), 0).is_empty());
    }

    #[test]
    fn a_missing_folder_has_none() {
        let t = tmp();
        assert!(repos_below(&t.path().join("missing"), REPO_SCAN_CAP).is_empty());
    }
}
