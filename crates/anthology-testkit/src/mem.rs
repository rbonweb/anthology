//! Port of `tests/_mem.py` (F4, m-13): the memory tests' isolated environment, temporary git
//! repositories and note files.
//!
//! | `_mem.py` | here |
//! |---|---|
//! | `GIT_ID` | [`GIT_ID`] (shared with `git_repo`) |
//! | `HAVE_GIT`, `needs_git` | [`have_git`], [`needs_git`] (`if !needs_git() { return; }`) |
//! | `git(cwd, *args, check=True)` | [`git`] (panics unless 0), [`try_git`] |
//! | `note_text(name, ...)` | [`note_text`] with a [`Note`] |
//! | `write(path, text, mode)` | [`write`] |
//! | `snapshot(root)` | [`snapshot`]: call it `mem::snapshot`, since the crate root's `snapshot` is `tree::snapshot`; a name that is not UTF-8 panics |
//! | `MemCase` | [`MemCase`]: `tmp`, `home`, `config`, `anth`, `data`, `env`, `make_repo`, `commit_all` |
//! | `MemCase.fake_repo` | [`MemCase::fake_repo`] makes the folder only: Python patched `memory_sync`'s `repo_root`, `repo_key`, `_main_root`, `settings_tracked` and `ensure_ignored`; Rust tests pass those through `memory::sync`'s `Deps` (GC-19) |
//!
//! Python patched `os.environ`; here [`MemCase::env`] is the whole environment of what a test
//! runs: `HOME`, `CLAUDE_CONFIG_DIR`, `GIT_CONFIG_NOSYSTEM=1`, `GIT_CONFIG_GLOBAL=/dev/null`,
//! this process's `PATH` and `LANG=C.UTF-8`. The variables `_mem.py` removes
//! (`CLAUDE_CODE_DISABLE_AUTO_MEMORY`, `CLAUDE_PLUGIN_DATA`, `GIT_DIR`, `GIT_WORK_TREE`,
//! `GIT_INDEX_FILE`) are therefore never set.
#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anthology_core::Env;
use tempfile::TempDir;

use crate::run::{Output, run_program, which};
pub use crate::sandbox::{GIT_ID, have_git};

/// The remote key `_mem.py`'s `fake_repo` uses by default.
pub const FAKE_REPO_KEY: &str = "example.invalid/acme/app";

/// `needs_git`: whether git is installed. When it is not, writes a skip note to stderr; the
/// test then returns early.
pub fn needs_git() -> bool {
    let have = have_git();
    if !have {
        eprintln!("skipped: git is not installed");
    }
    have
}

/// `git(cwd, *args)`: `git <GIT_ID> <args>` in `cwd` with exactly `env`. Panics unless git
/// exits 0, as `check=True` raises.
pub fn git(env: &Env, cwd: &Path, args: &[&str]) -> Output {
    let out = try_git(env, cwd, args);
    assert_eq!(
        out.code,
        0,
        "git {args:?} in {}: {}",
        cwd.display(),
        out.stderr_text()
    );
    out
}

/// `git(cwd, *args, check=False)`.
pub fn try_git(env: &Env, cwd: &Path, args: &[&str]) -> Output {
    let program = which("git").expect("git is not installed (use needs_git)");
    let mut argv: Vec<&str> = GIT_ID.to_vec();
    argv.extend_from_slice(args);
    run_program(&program, &argv, b"", env, cwd)
}

/// The keyword arguments of `note_text`, with its defaults ([`Note::new`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub name: String,
    /// `type_`, default `project`.
    pub type_: String,
    /// `desc`, default `A note`.
    pub desc: String,
    /// `updated`; `None` or empty leaves the line out.
    pub updated: Option<String>,
    /// `body`, default `Body.\n`.
    pub body: String,
    /// `relations`, in order: kind and targets.
    pub relations: Vec<(String, Vec<String>)>,
    pub top_level_type: bool,
}

impl Note {
    /// A note named `name` with the defaults.
    pub fn new(name: &str) -> Note {
        Note {
            name: name.to_string(),
            type_: "project".into(),
            desc: "A note".into(),
            updated: None,
            body: "Body.\n".into(),
            relations: Vec::new(),
            top_level_type: false,
        }
    }

    pub fn type_(mut self, t: &str) -> Note {
        self.type_ = t.into();
        self
    }

    pub fn desc(mut self, d: &str) -> Note {
        self.desc = d.into();
        self
    }

    pub fn updated(mut self, u: &str) -> Note {
        self.updated = Some(u.into());
        self
    }

    pub fn body(mut self, b: &str) -> Note {
        self.body = b.into();
        self
    }

    pub fn relation(mut self, kind: &str, targets: &[&str]) -> Note {
        self.relations
            .push((kind.into(), targets.iter().map(|t| t.to_string()).collect()));
        self
    }

    pub fn top_level_type(mut self) -> Note {
        self.top_level_type = true;
        self
    }

    /// [`note_text`] of this note.
    pub fn text(&self) -> String {
        note_text(self)
    }
}

/// `note_text`: frontmatter shaped like the notes Claude Code writes (`metadata.type`,
/// `metadata.modified`), or with a top-level `type`/`updated` when `top_level_type`.
pub fn note_text(n: &Note) -> String {
    let mut lines = vec![
        "---".to_string(),
        format!("name: {}", n.name),
        format!("description: {}", n.desc),
    ];
    let updated = n.updated.as_deref().filter(|u| !u.is_empty());
    if n.top_level_type {
        lines.push(format!("type: {}", n.type_));
        if let Some(u) = updated {
            lines.push(format!("updated: {u}"));
        }
    } else {
        lines.push("metadata:".into());
        lines.push(format!("  type: {}", n.type_));
        if let Some(u) = updated {
            lines.push(format!("  modified: {u}"));
        }
        if !n.relations.is_empty() {
            lines.push("  relations:".into());
            for (kind, targets) in &n.relations {
                lines.push(format!("    {kind}: [{}]", targets.join(", ")));
            }
        }
    }
    lines.extend(["---".to_string(), String::new(), n.body.clone()]);
    lines.join("\n")
}

/// `write(path, text, mode)`: creates the parent folders, writes UTF-8 text, then sets `mode`.
pub fn write(path: &Path, text: &str, mode: Option<u32>) -> PathBuf {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).unwrap_or_else(|e| panic!("write: {}: {e}", dir.display()));
    }
    fs::write(path, text).unwrap_or_else(|e| panic!("write: {}: {e}", path.display()));
    if let Some(m) = mode {
        fs::set_permissions(path, fs::Permissions::from_mode(m)).expect("write: chmod");
    }
    path.to_path_buf()
}

/// `snapshot(root)`: `{relative path: (bytes, mtime_ns)}` of every file under `root` except
/// `.git`. As Python 3.12's `rglob("*")` with `is_file()`: symlinked folders are not entered,
/// a symlink to a file counts as that file (its bytes and mtime).
pub fn snapshot(root: &Path) -> BTreeMap<String, (Vec<u8>, i128)> {
    let mut out = BTreeMap::new();
    let mut stack = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, rel)) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().into_string().unwrap_or_else(|n| {
                panic!(
                    "snapshot: a name that is not UTF-8 under {}: {n:?}",
                    dir.display()
                )
            });
            let rel = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            if name == ".git" {
                continue;
            }
            let path = entry.path();
            let Ok(lmeta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if lmeta.is_dir() {
                stack.push((path, rel));
                continue;
            }
            if let Ok(meta) = fs::metadata(&path)
                && meta.is_file()
                && let Ok(bytes) = fs::read(&path)
            {
                let ns = i128::from(meta.mtime()) * 1_000_000_000 + i128::from(meta.mtime_nsec());
                out.insert(rel, (bytes, ns));
            }
        }
    }
    out
}

/// `MemCase`: a temporary `HOME` and `CLAUDE_CONFIG_DIR`, and a git environment that ignores
/// the host's config (module docs). The folder is removed on drop.
pub struct MemCase {
    _dir: TempDir,
    /// The temporary folder (its realpath).
    pub tmp: PathBuf,
    /// `tmp/home` (created).
    pub home: PathBuf,
    /// `home/.claude` (not created, as in Python).
    pub config: PathBuf,
    /// `config/anthology`.
    pub anth: PathBuf,
    /// `tmp/data` (created).
    pub data: PathBuf,
    /// The environment of everything the test runs.
    pub env: Env,
}

impl Default for MemCase {
    fn default() -> MemCase {
        MemCase::new()
    }
}

impl MemCase {
    /// `setUp`.
    pub fn new() -> MemCase {
        let dir = tempfile::tempdir().expect("MemCase: tempdir");
        let tmp = fs::canonicalize(dir.path()).expect("MemCase: realpath");
        let home = tmp.join("home");
        let config = home.join(".claude");
        let anth = config.join("anthology");
        let data = tmp.join("data");
        fs::create_dir(&home).expect("MemCase: home");
        fs::create_dir(&data).expect("MemCase: data");
        let path = std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into());
        let env = Env::default()
            .with_os("HOME", &home)
            .with_os("CLAUDE_CONFIG_DIR", &config)
            .with("GIT_CONFIG_NOSYSTEM", "1")
            .with("GIT_CONFIG_GLOBAL", "/dev/null")
            .with_os("PATH", path)
            .with("LANG", "C.UTF-8");
        MemCase {
            _dir: dir,
            tmp,
            home,
            config,
            anth,
            data,
            env,
        }
    }

    /// `git(cwd, *args)` with this case's environment.
    pub fn git(&self, cwd: &Path, args: &[&str]) -> Output {
        git(&self.env, cwd, args)
    }

    /// `make_repo(name, remote)`: `git init -q` in `tmp/<name>` (which must not exist), with an
    /// `origin` when `remote` is given; returns the realpath.
    pub fn make_repo(&self, name: &str, remote: Option<&str>) -> PathBuf {
        let root = self.tmp.join(name);
        if let Some(parent) = root.parent() {
            fs::create_dir_all(parent).expect("make_repo: mkdir");
        }
        fs::create_dir(&root).unwrap_or_else(|e| panic!("make_repo: {}: {e}", root.display()));
        self.git(&root, &["init", "-q"]);
        if let Some(url) = remote {
            self.git(&root, &["remote", "add", "origin", url]);
        }
        fs::canonicalize(&root).expect("make_repo: realpath")
    }

    /// `commit_all(root, msg)`: `git add -A` then `git commit -q -m <msg>`.
    pub fn commit_all(&self, root: &Path, msg: &str) {
        self.git(root, &["add", "-A"]);
        self.git(root, &["commit", "-q", "-m", msg]);
    }

    /// The folder of `fake_repo(name)` (created; module docs for the patched functions).
    pub fn fake_repo(&self, name: &str) -> PathBuf {
        let root = self.tmp.join(name);
        fs::create_dir_all(&root).expect("fake_repo: mkdir");
        fs::canonicalize(&root).expect("fake_repo: realpath")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn note_text_matches_the_python_shapes() {
        assert_eq!(
            Note::new("n").text(),
            "---\nname: n\ndescription: A note\nmetadata:\n  type: project\n---\n\nBody.\n"
        );
        let full = Note::new("deploy")
            .type_("feedback")
            .desc("How we deploy")
            .updated("2026-01-02")
            .relation("supersedes", &["a", "b"])
            .relation("related", &["c"])
            .body("Text.");
        assert_eq!(
            note_text(&full),
            "---\nname: deploy\ndescription: How we deploy\nmetadata:\n  type: feedback\n  \
             modified: 2026-01-02\n  relations:\n    supersedes: [a, b]\n    related: [c]\n---\n\nText."
        );
        let top = Note::new("t")
            .top_level_type()
            .updated("2026-03-04")
            .relation("x", &["y"]);
        assert_eq!(
            top.text(),
            "---\nname: t\ndescription: A note\ntype: project\nupdated: 2026-03-04\n---\n\nBody.\n"
        );
        // `if updated:` is false for an empty string.
        assert_eq!(Note::new("n").updated("").text(), Note::new("n").text());
    }

    #[test]
    fn write_makes_folders_and_sets_the_mode() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = write(&dir.path().join("a/b/c.md"), "x\n", Some(0o600));
        assert_eq!(fs::read_to_string(&p).expect("read"), "x\n");
        assert_eq!(
            fs::metadata(&p).expect("stat").permissions().mode() & 0o777,
            0o600
        );
        let q = write(&dir.path().join("d.md"), "", None);
        assert_eq!(fs::read(q).expect("read"), b"");
    }

    #[test]
    fn snapshot_skips_git_and_folders_and_follows_file_links_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        write(&root.join("a/n.md"), "one", None);
        write(&root.join(".git/config"), "x", None);
        write(&root.join("sub/.git/HEAD"), "x", None);
        write(&root.join("wt/.git"), "gitdir: x", None);
        write(&root.join("outside/o.md"), "o", None);
        symlink(root.join("a/n.md"), root.join("link.md")).expect("symlink");
        symlink(root.join("outside"), root.join("linkdir")).expect("symlink");
        symlink(root.join("missing"), root.join("dangling")).expect("symlink");
        let snap = snapshot(root);
        let keys: Vec<&str> = snap.keys().map(String::as_str).collect();
        assert_eq!(keys, ["a/n.md", "link.md", "outside/o.md"]);
        assert!(!snap.contains_key("wt/.git"));
        assert_eq!(snap["link.md"].0, b"one");
        let meta = fs::metadata(root.join("a/n.md")).expect("stat");
        assert_eq!(
            snap["a/n.md"].1,
            i128::from(meta.mtime()) * 1_000_000_000 + i128::from(meta.mtime_nsec())
        );
        assert_eq!(snap["link.md"].1, snap["a/n.md"].1);
    }

    #[test]
    fn mem_case_is_isolated() {
        let case = MemCase::new();
        assert!(case.home.is_dir() && case.data.is_dir());
        assert!(!case.config.exists());
        assert_eq!(case.anth, case.home.join(".claude/anthology"));
        assert_eq!(
            case.env.get_os("CLAUDE_CONFIG_DIR"),
            Some(case.config.as_os_str())
        );
        for gone in [
            "CLAUDE_CODE_DISABLE_AUTO_MEMORY",
            "CLAUDE_PLUGIN_DATA",
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
        ] {
            assert!(case.env.get_os(gone).is_none(), "{gone}");
        }
        assert_eq!(case.env.get("GIT_CONFIG_GLOBAL"), Some("/dev/null"));
        let fake = case.fake_repo("f");
        assert!(fake.is_dir() && fake.starts_with(&case.tmp));
    }

    #[test]
    fn make_repo_and_commit_all() {
        if !needs_git() {
            return;
        }
        let case = MemCase::new();
        let root = case.make_repo("nested/repo", Some("git@example.invalid:acme/app.git"));
        assert_eq!(root, case.tmp.join("nested/repo"));
        write(&root.join("x.md"), "x", None);
        case.commit_all(&root, "c");
        let log = case.git(&root, &["log", "--format=%an %ae %s"]);
        assert_eq!(log.stdout_text(), "t t@example.invalid c\n");
        let url = case.git(&root, &["remote", "get-url", "origin"]);
        assert_eq!(url.stdout_text(), "git@example.invalid:acme/app.git\n");
        let failed = try_git(&case.env, &root, &["rev-parse", "--verify", "nope"]);
        assert_ne!(failed.code, 0);
        let err = std::panic::catch_unwind(|| case.git(&root, &["no-such-command"]));
        assert!(err.is_err());
    }
}
