//! MS2 test support for `memory_sync_part2.rs`: `fake_repo` through a fake `Git`, the
//! `personal_only` context, and a timeout runner.
#![allow(
    dead_code,
    reason = "shared by the test modules; not every helper is used by each"
)]

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use anthology_core::Env;
use anthology_core::git::{Git, GitOut};
use anthology_memory::sync::{self, SyncDeps};
use anthology_testkit::mem::{FAKE_REPO_KEY, MemCase};

/// `MemCase.fake_repo`: Python patched `repo_root`, `repo_key`, `_main_root`,
/// `settings_tracked` and `ensure_ignored`. Here git itself is fake: every folder is inside the
/// work tree `root`, whose `origin` normalises to `key`; nothing is tracked and
/// `.claude/settings.local.json` is already ignored (so `ensure_ignored` is a no-op).
pub struct FakeGit {
    pub root: PathBuf,
    pub key: String,
}

impl Git for FakeGit {
    fn run(&self, _env: &Env, _cwd: &Path, args: &[&str]) -> Option<GitOut> {
        let ok = |out: String| GitOut {
            code: 0,
            stdout: out.into_bytes(),
            stderr: Vec::new(),
        };
        let fail = |code: i32| GitOut {
            code,
            stdout: Vec::new(),
            stderr: Vec::new(),
        };
        let root = self.root.to_string_lossy();
        Some(match args {
            ["rev-parse", "--show-toplevel"] => ok(format!("{root}\n")),
            ["rev-parse", "--path-format=absolute", "--git-common-dir"] => {
                ok(format!("{root}/.git\n"))
            }
            ["remote"] => ok("origin\n".into()),
            ["remote", "get-url", "origin"] => ok(format!("https://{}\n", self.key)),
            ["ls-files", ..] => fail(1),
            ["check-ignore", ..] => fail(0),
            _ => fail(128),
        })
    }
}

/// A fake repo folder `tmp/<name>` and its git.
pub fn fake_repo(case: &MemCase, name: &str, key: Option<&str>) -> FakeGit {
    FakeGit {
        root: case.fake_repo(name),
        key: key.unwrap_or(FAKE_REPO_KEY).to_string(),
    }
}

/// Production deps with `git`.
pub fn deps(git: &dyn Git) -> SyncDeps<'_> {
    SyncDeps {
        git,
        ..SyncDeps::default()
    }
}

/// `memory_sync.run(cwd, env())` with the real git.
pub fn run(case: &MemCase, cwd: &Path) -> (String, Vec<String>) {
    sync::run(cwd, &case.env, &SyncDeps::default())
}

/// `memory_sync.run` with `git`.
pub fn run_with(case: &MemCase, cwd: &Path, git: &dyn Git) -> (String, Vec<String>) {
    sync::run(cwd, &case.env, &deps(git))
}

/// `memory_sync.approve(root)` with the real git (`approve::approve`).
pub fn approve(case: &MemCase, root: &Path) -> String {
    anthology_memory::approve::approve(root, &case.env, &anthology_core::git::RealGit)
        .expect("approve")
}

/// `memory_sync.approve(root)` in a fake repo.
pub fn approve_fake(case: &MemCase, git: &FakeGit) -> String {
    anthology_memory::approve::approve(&git.root, &case.env, git).expect("approve")
}

/// `personal_only(anth)`: the context when personal memory has no notes.
pub fn personal_only(anth: &Path) -> String {
    let p = anth.join("personal-memory");
    let p = p.to_string_lossy();
    format!(
        "## Personal memory ({p})\nSave notes about the user (type: user: role, preferences) here \
         as {p}/user-<slug>.md, never in the repo.\n\n(no notes yet)\n"
    )
}

/// `encode(path)`: the old per-path folder name.
pub fn encode(path: &Path) -> String {
    anthology_pycompat::path::realpath(path)
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// `run_with_timeout`: `Some(result)` when `f` finished within `secs`, else `None` (the thread
/// is left behind, as Python's daemon thread).
pub fn run_with_timeout<T: Send + 'static>(
    secs: u64,
    f: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(Duration::from_secs(secs)).ok()
}

/// File names in `dir`, sorted.
pub fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// Names in `dir` ending in `.md`, other than `MEMORY.md`, sorted.
pub fn md_names(dir: &Path) -> Vec<String> {
    names(dir)
        .into_iter()
        .filter(|n| n.ends_with(".md") && n != "MEMORY.md")
        .collect()
}

/// `os.mkfifo(path)` through the `mkfifo` tool (the tests forbid `unsafe`).
pub fn mkfifo(path: &Path) {
    let st = std::process::Command::new("mkfifo")
        .arg(path)
        .status()
        .expect("mkfifo");
    assert!(st.success(), "mkfifo {}", path.display());
}

/// Lets a stuck FIFO reader finish (`unblock`): opens the FIFO for writing without blocking.
pub fn unblock(fifo: &Path) {
    use std::os::unix::fs::OpenOptionsExt;
    // O_NONBLOCK is 0x4 on macOS and 0o4000 on Linux.
    let flag = if cfg!(target_os = "macos") {
        0x4
    } else {
        0o4000
    };
    let _ = std::fs::OpenOptions::new()
        .write(true)
        .custom_flags(flag)
        .open(fifo);
}
