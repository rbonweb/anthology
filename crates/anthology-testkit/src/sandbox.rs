//! `Sandbox`, `git_repo` and the repository root (F4, plan §3.7).
//!
//! A [`Sandbox`] is a temporary folder (its realpath, so macOS's `/var` → `/private/var` never
//! shows in a comparison) holding:
//!
//! - `home/`: the `HOME` of every program a test runs;
//! - `home/.claude/` (`config`): Claude Code's config folder, where `CLAUDE_CONFIG_DIR` would
//!   point; [`Sandbox::env`] does not set that variable, a test adds it with `.with()`;
//! - `data/`: the plugin data folder (`CLAUDE_PLUGIN_DATA`);
//! - `tmp/`: `TMPDIR`, so temporary files of the programs stay inside the sandbox.
//!
//! [`Sandbox::env`] is minimal, as `tests/_mem.py`'s isolated environment is: the sandbox's
//! `HOME`, this process's `PATH`, `LANG=C.UTF-8`, `CLAUDE_PLUGIN_DATA`, `TMPDIR`, and git kept
//! away from the developer's configuration (`GIT_CONFIG_NOSYSTEM=1`,
//! `GIT_CONFIG_GLOBAL=/dev/null`). Nothing else of the test process's environment is passed on.
//!
//! [`git_repo`] makes a repository with `_mem.py`'s `GIT_ID` settings and runs git through
//! `anthology_core::git` with such an environment.
#![forbid(unsafe_code)]

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use anthology_core::Env;
use anthology_core::git::{Git, GitOut, RealGit};
use tempfile::TempDir;

/// `tests/_mem.py`'s `GIT_ID`: an identity, `main` as the first branch, and local file
/// remotes allowed. Passed as `-c` options before every git command.
pub const GIT_ID: [&str; 8] = [
    "-c",
    "user.name=t",
    "-c",
    "user.email=t@example.invalid",
    "-c",
    "init.defaultBranch=main",
    "-c",
    "protocol.file.allow=always",
];

/// The repository root: two folders above the testkit crate (`CARGO_MANIFEST_DIR` read at run
/// time when cargo sets it, else the one this crate was built from).
pub fn repo_root() -> PathBuf {
    let manifest = std::env::var_os("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    // CARGO_MANIFEST_DIR is the crate under test: crates/<crate>, or the root itself.
    let mut dir = manifest.as_path();
    loop {
        if dir.join("Cargo.lock").is_file() && dir.join("crates").is_dir() {
            return dir.to_path_buf();
        }
        match dir.parent() {
            Some(p) => dir = p,
            None => panic!("repo_root: no workspace root above {}", manifest.display()),
        }
    }
}

/// Where a file CO2 moves out of `port/staging/` is: `<repo>/port/staging/<rel>` while that
/// folder exists (before the cutover), else `<repo>/<rel>` (after it). The one switch for every
/// test that reads a staged file, so the move itself needs no test edit.
pub fn staged(rel: &str) -> PathBuf {
    let root = repo_root();
    let staging = root.join("port/staging");
    if staging.is_dir() {
        staging.join(rel)
    } else {
        root.join(rel)
    }
}

/// A plugin of this repository.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Plugin {
    Anthology,
    Guardrails,
    Superpowers,
}

impl Plugin {
    /// Every plugin.
    pub const ALL: [Plugin; 3] = [Plugin::Anthology, Plugin::Guardrails, Plugin::Superpowers];

    /// Its folder name under `plugins/`.
    pub fn dir_name(self) -> &'static str {
        match self {
            Plugin::Anthology => "anthology",
            Plugin::Guardrails => "anthology-guardrails",
            Plugin::Superpowers => "anthology-superpowers",
        }
    }
}

/// A temporary home, config and data folder (module docs). Removed on drop.
pub struct Sandbox {
    pub root: TempDir,
    pub home: PathBuf,
    pub config: PathBuf,
    pub data: PathBuf,
}

impl Default for Sandbox {
    fn default() -> Sandbox {
        Sandbox::new()
    }
}

impl Sandbox {
    /// Creates `home/`, `home/.claude/`, `data/` and `tmp/` in a new temporary folder.
    pub fn new() -> Sandbox {
        let root = tempfile::Builder::new()
            .prefix("anthology-sandbox-")
            .tempdir()
            .expect("sandbox: tempdir");
        let real = fs::canonicalize(root.path()).expect("sandbox: realpath");
        let home = real.join("home");
        let config = home.join(".claude");
        let data = real.join("data");
        for dir in [&config, &data, &real.join("tmp")] {
            fs::create_dir_all(dir).expect("sandbox: mkdir");
        }
        Sandbox {
            root,
            home,
            config,
            data,
        }
    }

    /// The sandbox folder (a realpath).
    pub fn path(&self) -> PathBuf {
        self.home
            .parent()
            .expect("home is in the root")
            .to_path_buf()
    }

    /// The minimal environment (module docs).
    pub fn env(&self) -> Env {
        let path = std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into());
        Env::default()
            .with_os("HOME", &self.home)
            .with_os("PATH", path)
            .with("LANG", "C.UTF-8")
            .with_os("CLAUDE_PLUGIN_DATA", &self.data)
            .with_os("TMPDIR", self.path().join("tmp"))
            .with("GIT_CONFIG_NOSYSTEM", "1")
            .with("GIT_CONFIG_GLOBAL", "/dev/null")
    }

    /// A copy of `plugins/<p>` at `<sandbox>/plugins/<p>`, with `port/staging/plugins/<p>`
    /// copied over it when that exists (the staged versions win). Made once per sandbox;
    /// later calls return the same folder. `__pycache__` folders are left out.
    pub fn plugin_tree(&self, which: Plugin) -> PathBuf {
        let dest = self.path().join("plugins").join(which.dir_name());
        if dest.is_dir() {
            return dest;
        }
        let root = repo_root();
        copy_tree(&root.join("plugins").join(which.dir_name()), &dest);
        let staging = root.join("port/staging/plugins").join(which.dir_name());
        if staging.is_dir() {
            copy_tree(&staging, &dest);
        }
        dest
    }
}

/// Copies `src` into `dest` (created), keeping modes and symlinks; existing files are replaced.
pub fn copy_tree(src: &Path, dest: &Path) {
    let meta =
        fs::symlink_metadata(src).unwrap_or_else(|e| panic!("copy_tree: {}: {e}", src.display()));
    let mut stack = vec![(src.to_path_buf(), dest.to_path_buf(), meta)];
    while let Some((from, to, meta)) = stack.pop() {
        let kind = meta.file_type();
        if kind.is_symlink() {
            let target = fs::read_link(&from).expect("copy_tree: readlink");
            if fs::symlink_metadata(&to).is_ok() {
                remove(&to);
            }
            symlink(target, &to).unwrap_or_else(|e| panic!("copy_tree: {}: {e}", to.display()));
        } else if kind.is_dir() {
            if fs::symlink_metadata(&to).is_ok_and(|m| !m.is_dir()) {
                remove(&to);
            }
            fs::create_dir_all(&to).unwrap_or_else(|e| panic!("copy_tree: {}: {e}", to.display()));
            fs::set_permissions(&to, meta.permissions()).expect("copy_tree: chmod");
            for entry in fs::read_dir(&from).expect("copy_tree: list") {
                let entry = entry.expect("copy_tree: entry");
                if entry.file_name() == "__pycache__" {
                    continue;
                }
                let m = fs::symlink_metadata(entry.path()).expect("copy_tree: lstat");
                stack.push((entry.path(), to.join(entry.file_name()), m));
            }
        } else if kind.is_file() {
            if fs::symlink_metadata(&to).is_ok_and(|m| m.is_dir() || m.is_symlink()) {
                remove(&to);
            }
            fs::copy(&from, &to).unwrap_or_else(|e| panic!("copy_tree: {}: {e}", to.display()));
            fs::set_permissions(&to, fs::Permissions::from_mode(meta.permissions().mode()))
                .expect("copy_tree: chmod");
        }
    }
}

fn remove(path: &Path) {
    let res = if fs::symlink_metadata(path).is_ok_and(|m| m.is_dir()) {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    res.unwrap_or_else(|e| panic!("copy_tree: remove {}: {e}", path.display()));
}

/// A git repository for a test, driven with an isolated environment (module docs).
#[derive(Clone, Debug)]
pub struct GitRepo {
    pub root: PathBuf,
    pub env: Env,
}

/// `git init -q` in `dir` (created when missing), with the `GIT_ID` settings. git's `HOME` is
/// `dir`'s parent, and the system and global configs are off.
pub fn git_repo(dir: &Path) -> GitRepo {
    let home = dir.parent().unwrap_or(dir);
    let path = std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into());
    let env = Env::default()
        .with_os("HOME", home)
        .with_os("PATH", path)
        .with("LANG", "C.UTF-8")
        .with("GIT_CONFIG_NOSYSTEM", "1")
        .with("GIT_CONFIG_GLOBAL", "/dev/null");
    GitRepo::init(dir, env)
}

impl GitRepo {
    /// `git init -q` in `dir` (created when missing) with exactly `env` for every git command.
    pub fn init(dir: &Path, env: Env) -> GitRepo {
        fs::create_dir_all(dir).unwrap_or_else(|e| panic!("git_repo: {}: {e}", dir.display()));
        let root = fs::canonicalize(dir).expect("git_repo: realpath");
        let repo = GitRepo { root, env };
        repo.git(&["init", "-q"]);
        repo
    }

    /// `git <GIT_ID> <args>` in the work tree; `None` when git is missing or timed out.
    pub fn run(&self, args: &[&str]) -> Option<GitOut> {
        let mut argv: Vec<&str> = GIT_ID.to_vec();
        argv.extend_from_slice(args);
        RealGit.run(&self.env, &self.root, &argv)
    }

    /// [`GitRepo::run`], panicking unless git exits 0; returns stdout as text.
    pub fn git(&self, args: &[&str]) -> String {
        let out = self
            .run(args)
            .unwrap_or_else(|| panic!("git {args:?}: git is missing or timed out"));
        assert_eq!(
            out.code,
            0,
            "git {args:?} in {}: {}",
            self.root.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// `git add -A` then `git commit -q -m <msg>`, as `_mem.py`'s `commit_all`.
    pub fn commit_all(&self, msg: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", msg]);
    }

    /// `git remote add origin <url>`.
    pub fn add_origin(&self, url: &str) {
        self.git(&["remote", "add", "origin", url]);
    }

    /// The `HEAD` commit id.
    pub fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"]).trim().to_string()
    }

    /// Writes `rel` (folders created) in the work tree; returns its path.
    pub fn write(&self, rel: &str, text: &str) -> PathBuf {
        let path = self.root.join(rel);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).expect("git_repo: mkdir");
        }
        fs::write(&path, text).expect("git_repo: write");
        path
    }
}

/// Whether `git` is on this process's `PATH`.
pub fn have_git() -> bool {
    crate::run::which("git").is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStrExt;

    #[test]
    fn a_sandbox_has_its_folders_and_a_minimal_env() {
        let sb = Sandbox::new();
        for dir in [&sb.home, &sb.config, &sb.data] {
            assert!(dir.is_dir(), "{}", dir.display());
        }
        assert_eq!(sb.config, sb.home.join(".claude"));
        assert_eq!(
            sb.path(),
            fs::canonicalize(sb.root.path()).expect("realpath")
        );
        let env = sb.env();
        let names: Vec<String> = env
            .iter()
            .map(|(k, _)| k.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            [
                "CLAUDE_PLUGIN_DATA",
                "GIT_CONFIG_GLOBAL",
                "GIT_CONFIG_NOSYSTEM",
                "HOME",
                "LANG",
                "PATH",
                "TMPDIR"
            ]
        );
        assert_eq!(
            env.get_os("HOME").map(OsStrExt::as_bytes),
            Some(sb.home.as_os_str().as_bytes())
        );
        assert!(env.get("CLAUDE_CONFIG_DIR").is_none());
        assert!(Path::new(env.get("TMPDIR").expect("tmp")).is_dir());
        let root = sb.path();
        drop(sb);
        assert!(!root.exists());
    }

    #[test]
    fn the_repo_root_holds_the_workspace() {
        let root = repo_root();
        assert!(root.join("crates/anthology-testkit/Cargo.toml").is_file());
        assert!(root.join("plugins").is_dir());
    }

    #[test]
    fn plugin_trees_are_copies_with_modes_and_links() {
        let sb = Sandbox::new();
        for p in Plugin::ALL {
            let tree = sb.plugin_tree(p);
            assert_eq!(tree, sb.path().join("plugins").join(p.dir_name()));
            let src = repo_root().join("plugins").join(p.dir_name());
            let a = crate::tree::snapshot(&src);
            let b = crate::tree::snapshot(&tree);
            assert!(
                b.iter()
                    .all(|(path, _)| !path.split('/').any(|c| c == "__pycache__"))
            );
            let a = crate::tree::Tree(
                a.0.into_iter()
                    .filter(|(path, _)| !path.split('/').any(|c| c == "__pycache__"))
                    .collect(),
            );
            if !repo_root()
                .join("port/staging/plugins")
                .join(p.dir_name())
                .exists()
            {
                assert!(a.diff(&b).is_empty(), "{p:?}: {:?}", a.diff(&b));
            }
            // A second call keeps the copy.
            fs::write(tree.join("marker"), "x").expect("write");
            assert!(sb.plugin_tree(p).join("marker").is_file());
        }
    }

    #[test]
    fn copy_tree_overlays_files_links_and_folders() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (a, b, dest) = (
            dir.path().join("a"),
            dir.path().join("b"),
            dir.path().join("d"),
        );
        fs::create_dir_all(a.join("x")).expect("mkdir");
        fs::write(a.join("x/f"), "old").expect("write");
        fs::write(a.join("keep"), "k").expect("write");
        fs::write(a.join("swap"), "file").expect("write");
        fs::create_dir_all(b.join("x")).expect("mkdir");
        fs::write(b.join("x/f"), "new").expect("write");
        fs::set_permissions(b.join("x/f"), fs::Permissions::from_mode(0o755)).expect("chmod");
        symlink("x/f", b.join("swap")).expect("symlink");
        copy_tree(&a, &dest);
        copy_tree(&b, &dest);
        assert_eq!(fs::read_to_string(dest.join("x/f")).expect("read"), "new");
        assert_eq!(
            fs::metadata(dest.join("x/f"))
                .expect("stat")
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        assert_eq!(fs::read_to_string(dest.join("keep")).expect("read"), "k");
        assert_eq!(
            fs::read_link(dest.join("swap")).expect("link"),
            Path::new("x/f")
        );
    }

    #[test]
    fn git_repo_commits_with_the_test_identity_and_no_host_config() {
        if !have_git() {
            eprintln!("skipped: git is not installed");
            return;
        }
        let sb = Sandbox::new();
        let repo = git_repo(&sb.path().join("r"));
        assert!(repo.root.join(".git").is_dir());
        repo.write("a/b.txt", "hello\n");
        repo.commit_all("first");
        assert_eq!(repo.head().len(), 40);
        let log = repo.git(&["log", "-1", "--format=%an <%ae> %s"]);
        assert_eq!(log, "t <t@example.invalid> first\n");
        assert_eq!(repo.git(&["branch", "--show-current"]), "main\n");
        repo.add_origin("https://example.invalid/acme/app.git");
        assert_eq!(
            repo.git(&["remote", "get-url", "origin"]),
            "https://example.invalid/acme/app.git\n"
        );
        // Only the repository's own config is read.
        let shown = repo.git(&["config", "--show-origin", "--list"]);
        for line in shown.lines() {
            assert!(
                line.starts_with("file:.git/config") || line.starts_with("command line:"),
                "{line}"
            );
        }
        assert_eq!(
            repo.run(&["rev-parse", "--verify", "nope"]).map(|o| o.code),
            Some(128)
        );
    }
}
