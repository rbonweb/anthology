//! Which files the gate skips, and how a path is shown in a message.

use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

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

/// The nearest folder above `path` that holds `.git` (a folder or a file), looking at most 64
/// folders up. A Claude Code worktree has a `.git` file, so it is its own root.
pub fn repo_root(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .skip(1)
        .take(64)
        .find(|dir| dir.join(".git").exists())
        .map(Path::to_path_buf)
}

/// Whether the gate skips `path`: it is under the Claude config folder, or a folder in the part
/// below the repo root is one of `EXEMPT_DIRS`. Only the part below the root counts, so a worktree
/// at `<repo>/.claude/worktrees/<id>/` is gated while the repo's own `.claude/` is not. With no
/// root the whole path is read.
pub fn exempt(path: &Path, root: Option<&Path>, config_dir: Option<&Path>) -> bool {
    // An empty or root-only config folder would hold every path (`starts_with("")` is true).
    let named = |dir: &&Path| dir.components().any(|c| matches!(c, Component::Normal(_)));
    if config_dir
        .filter(named)
        .is_some_and(|dir| path.starts_with(dir))
    {
        return true;
    }
    below(path, root).components().any(|part| match part {
        Component::Normal(name) => EXEMPT_DIRS.iter().any(|dir| OsStr::new(dir) == name),
        _ => false,
    })
}

/// `path` relative to `root` when under it, else as given. A control character in a name is
/// written as an escape, so a file name cannot add lines or terminal codes to a message.
pub fn shown(path: &Path, root: Option<&Path>) -> String {
    let mut out = String::new();
    for c in below(path, root).to_string_lossy().chars() {
        if c.is_control() {
            out.extend(c.escape_default());
        } else {
            out.push(c);
        }
    }
    out
}

/// `path` without `root` when it is under it, else `path`.
fn below<'a>(path: &'a Path, root: Option<&Path>) -> &'a Path {
    root.and_then(|root| path.strip_prefix(root).ok())
        .unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn p(s: &str) -> &Path {
        Path::new(s)
    }

    #[test]
    fn worktree_under_dot_claude_is_not_exempt() {
        assert!(!exempt(
            p("/r/.claude/worktrees/w/docs/a.md"),
            Some(p("/r/.claude/worktrees/w")),
            None
        ));
    }

    #[test]
    fn a_repos_own_dot_claude_is_exempt() {
        assert!(exempt(p("/r/.claude/memory/x.md"), Some(p("/r")), None));
        assert!(!exempt(p("/r/docs/x.md"), Some(p("/r")), None));
    }

    #[test]
    fn node_modules_and_vendor_are_exempt() {
        for dir in EXEMPT_DIRS {
            let path = format!("/r/{dir}/pkg/README.md");
            assert!(exempt(p(&path), Some(p("/r")), None), "{path}");
        }
        // a component must equal a listed name; part of a name does not count
        for path in [
            "/r/docs/a.md",
            "/r/src/vendors/a.md",
            "/r/docs/building/a.md",
            "/r/docs/distribution/a.md",
            "/r/docs/not-node_modules/a.md",
        ] {
            assert!(!exempt(p(path), Some(p("/r")), None), "{path}");
        }
    }

    #[test]
    fn only_the_part_below_the_root_is_checked() {
        // the repo itself lives in a folder called build: its own files are still gated
        assert!(!exempt(
            p("/home/user/build/proj/docs/a.md"),
            Some(p("/home/user/build/proj")),
            None
        ));
        assert!(exempt(
            p("/home/user/build/proj/dist/a.md"),
            Some(p("/home/user/build/proj")),
            None
        ));
    }

    #[test]
    fn without_a_root_the_whole_path_is_checked() {
        assert!(exempt(p("/tmp/node_modules/p/README.md"), None, None));
        assert!(!exempt(p("/tmp/docs/a.md"), None, None));
    }

    #[test]
    fn the_config_dir_is_exempt() {
        let cfg = Some(p("/home/user/.claude"));
        assert!(exempt(p("/home/user/.claude/plugins/x.md"), None, cfg));
        assert!(exempt(
            p("/home/user/.claude/plugins/x.md"),
            Some(p("/home/user")),
            cfg
        ));
        // a folder that only starts with the same letters is another folder
        assert!(!exempt(p("/home/user/.claude-other/a.md"), None, cfg));
        assert!(!exempt(p("/home/user/docs/a.md"), None, cfg));
        // a config folder that has no exempt name still exempts what is under it
        let cfg = Some(p("/etc/cfg"));
        assert!(exempt(p("/etc/cfg/a.md"), None, cfg));
        assert!(!exempt(p("/etc/cfgx/a.md"), None, cfg));
    }

    #[test]
    fn an_empty_config_dir_exempts_nothing() {
        // `Path::starts_with("")` is true for every path, so these must be ignored
        for cfg in ["", "/", "."] {
            assert!(
                !exempt(p("/r/docs/a.md"), Some(p("/r")), Some(p(cfg))),
                "{cfg:?}"
            );
        }
    }

    #[test]
    fn repo_root_finds_a_git_folder_or_file() {
        let tmp = tempfile::tempdir().unwrap();
        let top = tmp.path();
        fs::create_dir_all(top.join(".git")).unwrap();
        fs::create_dir_all(top.join("docs/deep")).unwrap();
        assert_eq!(
            repo_root(&top.join("docs/deep/a.md")),
            Some(top.to_path_buf())
        );
        assert_eq!(repo_root(&top.join("a.md")), Some(top.to_path_buf()));
        // a worktree has a .git file; the nearest one wins
        let wt = top.join("work/tree");
        fs::create_dir_all(wt.join("docs")).unwrap();
        fs::write(wt.join(".git"), "gitdir: /elsewhere\n").unwrap();
        assert_eq!(repo_root(&wt.join("docs/a.md")), Some(wt.clone()));
    }

    #[test]
    fn repo_root_looks_up_at_most_64_folders() {
        let tmp = tempfile::tempdir().unwrap();
        let top = tmp.path();
        fs::create_dir_all(top.join(".git")).unwrap();
        // the folders need not exist: only .git is looked for
        let deep = |levels: usize| {
            let mut path = top.to_path_buf();
            for _ in 0..levels {
                path.push("d");
            }
            path.join("a.md")
        };
        assert_eq!(repo_root(&deep(63)), Some(top.to_path_buf()));
        assert_eq!(repo_root(&deep(64)), None);
    }

    #[test]
    fn shown_is_relative_to_the_root() {
        assert_eq!(shown(p("/r/docs/a.md"), Some(p("/r"))), "docs/a.md");
        assert_eq!(shown(p("/r/a.md"), Some(p("/r"))), "a.md");
        assert_eq!(
            shown(p("/elsewhere/a.md"), Some(p("/r"))),
            "/elsewhere/a.md"
        );
        assert_eq!(shown(p("/r/docs/a.md"), None), "/r/docs/a.md");
        assert_eq!(shown(p("docs/a.md"), Some(p("/r"))), "docs/a.md");
    }

    #[test]
    fn shown_escapes_control_characters() {
        // a name is printed to the agent, so it cannot add lines or terminal escapes
        assert_eq!(shown(p("/r/docs/a\nb.md"), Some(p("/r"))), "docs/a\\nb.md");
        assert_eq!(
            shown(p("/r/\u{1b}[2Ja.md"), Some(p("/r"))),
            "\\u{1b}[2Ja.md"
        );
        assert_eq!(shown(p("/r/é ü.md"), Some(p("/r"))), "é ü.md");
    }
}
