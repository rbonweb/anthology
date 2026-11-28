//! Which files the gate skips, and how a path is shown in a message.

use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

/// A path whose part below the repo root has one of these components is exempt, in any case.
pub const EXEMPT_DIRS: [&str; 17] = [
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
    "__tests__",
    "__mocks__",
    "testdata",
    "stories",
];

/// A file whose name has one of these as a dotted part (`Button.stories.tsx`) is exempt, in any case.
const EXEMPT_FILE_PARTS: [&str; 3] = ["stories", "test", "spec"];

/// The nearest folder above `path` that holds `.git` (a folder or a file), looking at most 64
/// folders up. A Claude Code worktree has a `.git` file, so it is its own root.
pub fn repo_root(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .skip(1)
        .take(64)
        .find(|dir| dir.join(".git").exists())
        .map(Path::to_path_buf)
}

/// Whether the gate skips `path`: it is under the Claude config folder, a folder in the part below
/// the repo root is one of `EXEMPT_DIRS`, or its file name is a story or test file. Names match in
/// any case. Only the part below the root counts, so a worktree at `<repo>/.claude/worktrees/<id>/`
/// is gated while the repo's own `.claude/` is not. With no root the whole path is read.
///
/// When the nearest root is a submodule (`is_submodule`), the part below the repo that holds it is
/// read too, and so on up: a submodule in `vendor/` has its own `.git` and is still vendored code.
pub fn exempt(path: &Path, root: Option<&Path>, config_dir: Option<&Path>) -> bool {
    // An empty or root-only config folder would hold every path (`starts_with("")` is true).
    let named = |dir: &&Path| dir.components().any(|c| matches!(c, Component::Normal(_)));
    if config_dir
        .filter(named)
        .is_some_and(|dir| path.starts_with(dir))
    {
        return true;
    }
    if exempt_name(path) || exempt_folder(below(path, root)) {
        return true;
    }
    let mut inner = root
        .filter(|root| path.starts_with(root))
        .map(Path::to_path_buf);
    while let Some(dir) = inner.filter(|dir| is_submodule(dir)) {
        let Some(outer) = repo_root(&dir) else { break };
        if exempt_folder(below(path, Some(&outer))) {
            return true;
        }
        inner = Some(outer);
    }
    false
}

/// Whether a component of `path` is one of `EXEMPT_DIRS`.
fn exempt_folder(path: &Path) -> bool {
    path.components().any(|part| match part {
        Component::Normal(name) => name
            .to_str()
            .is_some_and(|name| EXEMPT_DIRS.iter().any(|dir| dir.eq_ignore_ascii_case(name))),
        _ => false,
    })
}

/// Whether the file name of `path` has a dotted part of `EXEMPT_FILE_PARTS` that is neither the
/// first nor the last part: `Button.stories.tsx` and `guide.test.md`, not `test.md`.
fn exempt_name(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let parts: Vec<&str> = name.split('.').collect();
    parts
        .get(1..parts.len().saturating_sub(1))
        .unwrap_or_default()
        .iter()
        .any(|part| {
            EXEMPT_FILE_PARTS
                .iter()
                .any(|m| m.eq_ignore_ascii_case(part))
        })
}

/// Whether the `.git` of `root` is the file of a submodule: a `gitdir:` line that points into the
/// outer repo's `.git/modules`. A worktree's file points into `.git/worktrees`, and a worktree of
/// a submodule has both, so the last of the two decides. A `.git` folder is a standalone clone: a
/// repo that merely sits below another one, say in `~/build/proj`, must stay gated.
fn is_submodule(root: &Path) -> bool {
    let mut line = String::new();
    // a `.git` file is one short line; a folder fails to read
    let read = File::open(root.join(".git")).and_then(|f| f.take(4096).read_to_string(&mut line));
    if read.is_err() {
        return false;
    }
    let Some(target) = line.lines().next().and_then(|l| l.strip_prefix("gitdir:")) else {
        return false;
    };
    Path::new(target.trim())
        .components()
        .rev()
        .find_map(|part| match part {
            Component::Normal(name) => match name.to_str() {
                Some("modules") => Some(true),
                Some("worktrees") => Some(false),
                _ => None,
            },
            _ => None,
        })
        .unwrap_or(false)
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
    fn test_data_and_mock_folders_are_exempt() {
        // review-2 M5: Go's `testdata/` and Jest's `__mocks__/` hold fixtures, as `fixtures/` does
        for path in [
            "/r/testdata/slop.md",
            "/r/pkg/testdata/in/a.md",
            "/r/src/__mocks__/a.tsx",
            "/r/src/TestData/a.md",
            "/r/src/__MOCKS__/a.tsx",
        ] {
            assert!(exempt(p(path), Some(p("/r")), None), "{path}");
        }
        // only a whole component counts
        for path in [
            "/r/docs/test-data/a.md",
            "/r/src/mocks/a.tsx",
            "/r/docs/data/a.md",
        ] {
            assert!(!exempt(p(path), Some(p("/r")), None), "{path}");
        }
    }

    #[test]
    fn exempt_folders_match_in_any_case() {
        // macOS folders are case-insensitive: `Vendor/` is the vendor folder
        assert!(exempt(p("/r/Vendor/pkg/README.md"), Some(p("/r")), None));
        assert!(exempt(p("/r/NODE_MODULES/p/a.md"), Some(p("/r")), None));
        assert!(exempt(p("/r/.Claude/memory/x.md"), Some(p("/r")), None));
        for dir in EXEMPT_DIRS {
            let path = format!("/r/{}/pkg/README.md", dir.to_uppercase());
            assert!(exempt(p(&path), Some(p("/r")), None), "{path}");
        }
        // a different name is still a different name
        assert!(!exempt(p("/r/Vendors/a.md"), Some(p("/r")), None));
        assert!(!exempt(p("/r/Docs/a.md"), Some(p("/r")), None));
    }

    #[test]
    fn story_and_test_files_are_exempt() {
        for path in [
            "/r/src/Button.stories.tsx",
            "/r/src/Button.STORIES.mdx",
            "/r/docs/guide.test.md",
            "/r/src/card.spec.css",
            "/r/src/Card.Spec.jsx",
            "/r/src/__tests__/guide.md",
            "/r/src/__TESTS__/guide.md",
            "/r/ui/stories/intro.mdx",
            "/r/ui/Stories/intro.mdx",
        ] {
            assert!(exempt(p(path), Some(p("/r")), None), "{path}");
        }
        // the marker must be a whole dotted part of the name, or a whole folder name
        for path in [
            "/r/docs/stories.md",
            "/r/docs/test.md",
            "/r/docs/spec.md",
            "/r/docs/latest.md",
            "/r/docs/contest.spec-sheet.md",
            "/r/docs/a.testing.md",
            "/r/docs/stories-index/a.md",
            "/r/docs/tests/a.md",
            "/r/docs/story/a.md",
            "/r/docs/test.d/a.md",
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

    /// An outer repo `top/outer` (a `.git` folder) with a submodule whose `.git` file points into
    /// `.git/modules`, at `top/outer/<at>`. Returns the path of `<at>/<file>` and its nearest root.
    fn submodule(top: &Path, at: &str, file: &str) -> (PathBuf, PathBuf) {
        let outer = top.join("outer");
        fs::create_dir_all(outer.join(".git")).unwrap();
        let sub = outer.join(at);
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join(".git"), "gitdir: ../../.git/modules/sub\n").unwrap();
        let path = sub.join(file);
        let root = repo_root(&path).unwrap();
        assert_eq!(root, sub);
        (path, root)
    }

    #[test]
    fn a_submodule_under_an_exempt_folder_is_exempt() {
        let tmp = tempfile::tempdir().unwrap();
        let (path, root) = submodule(tmp.path(), "vendor/sub", "docs/README.md");
        assert!(exempt(&path, Some(&root), None));
        // the case of the folder does not matter
        let tmp = tempfile::tempdir().unwrap();
        let (path, root) = submodule(tmp.path(), "Vendor/sub", "README.md");
        assert!(exempt(&path, Some(&root), None));
        // a submodule in a folder that is not exempt is gated
        let tmp = tempfile::tempdir().unwrap();
        let (path, root) = submodule(tmp.path(), "docs/sub", "README.md");
        assert!(!exempt(&path, Some(&root), None));
        // the exempt folder inside the submodule still counts
        let tmp = tempfile::tempdir().unwrap();
        let (path, root) = submodule(tmp.path(), "docs/sub", "dist/a.md");
        assert!(exempt(&path, Some(&root), None));
    }

    #[test]
    fn a_submodule_inside_a_submodule_walks_up_to_the_exempt_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let (_, mid) = submodule(tmp.path(), "vendor/a", "x.md");
        let inner = mid.join("b");
        fs::create_dir_all(&inner).unwrap();
        fs::write(
            inner.join(".git"),
            "gitdir: ../../../.git/modules/a/modules/b\n",
        )
        .unwrap();
        let path = inner.join("README.md");
        assert_eq!(repo_root(&path), Some(inner.clone()));
        assert!(exempt(&path, Some(&inner), None));
    }

    #[test]
    fn a_worktree_under_dot_claude_is_still_gated_on_disk() {
        // the outer repo has a `.claude` folder above this worktree, whose `.git` file points
        // into `.git/worktrees`, not `.git/modules`
        let tmp = tempfile::tempdir().unwrap();
        let outer = tmp.path().join("outer");
        fs::create_dir_all(outer.join(".git")).unwrap();
        let wt = outer.join(".claude/worktrees/w");
        fs::create_dir_all(wt.join("docs")).unwrap();
        fs::write(wt.join(".git"), "gitdir: ../../../.git/worktrees/w\n").unwrap();
        let path = wt.join("docs/a.md");
        assert_eq!(repo_root(&path), Some(wt.clone()));
        assert!(!exempt(&path, Some(&wt), None));
        // a worktree of a submodule is a worktree too
        fs::write(
            wt.join(".git"),
            "gitdir: ../../../.git/modules/s/worktrees/w\n",
        )
        .unwrap();
        assert!(!exempt(&path, Some(&wt), None));
    }

    #[test]
    fn a_plain_nested_repo_is_gated_even_under_an_exempt_looking_parent() {
        // a clone in `~/build/proj` has a `.git` folder; a repo above it must not switch it off
        let tmp = tempfile::tempdir().unwrap();
        let outer = tmp.path().join("outer");
        fs::create_dir_all(outer.join(".git")).unwrap();
        let proj = outer.join("build/proj");
        fs::create_dir_all(proj.join(".git")).unwrap();
        let path = proj.join("docs/a.md");
        assert_eq!(repo_root(&path), Some(proj.clone()));
        assert!(!exempt(&path, Some(&proj), None));
        // a `.git` file that is not a gitdir line is not a submodule either
        fs::remove_dir(proj.join(".git")).unwrap();
        fs::write(proj.join(".git"), "not a gitdir line\n").unwrap();
        assert!(!exempt(&path, Some(&proj), None));
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
