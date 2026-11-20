//! The `Git` trait and `RealGit` (plan §3.3, GC-2): `memory_sync._git`.
//!
//! `git -c core.fsmonitor=false <args>` runs in `cwd` with exactly the given [`Env`] minus
//! `GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE` and `GIT_COMMON_DIR` (never the real process
//! environment), stdin `/dev/null`, and stdout and stderr read by their own threads so a large
//! output cannot deadlock. Past the timeout (10 s, [`GIT_TIMEOUT`]) the child is killed and
//! reaped and the result is `None`, as is a missing `git` or any spawn failure. As with
//! `subprocess.run(timeout=10)`, the timeout also covers reading the pipes to their end, and a
//! killed child's grandchildren are not waited for.

use std::ffi::OsStr;
use std::io::Read;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::Env;

/// `subprocess.run(..., timeout=10)`.
pub const GIT_TIMEOUT: Duration = Duration::from_secs(10);

/// The variables removed from git's environment.
pub const STRIPPED_VARS: [&str; 4] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
];

/// A finished git: its exit code (`-signal` when a signal ended it, as `returncode`) and output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitOut {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Runs git (the GC-19 seam for `memory_sync._git` and `_run` patches).
pub trait Git {
    /// `None` when git is missing, cannot start, or times out.
    fn run(&self, env: &Env, cwd: &Path, args: &[&str]) -> Option<GitOut>;
}

/// The real git, with the 10 s timeout.
#[derive(Clone, Copy, Debug, Default)]
pub struct RealGit;

impl RealGit {
    /// The real git with another timeout (tests).
    pub fn with_timeout(timeout: Duration) -> TimedGit {
        TimedGit { timeout }
    }
}

impl Git for RealGit {
    fn run(&self, env: &Env, cwd: &Path, args: &[&str]) -> Option<GitOut> {
        run_git(env, cwd, args, GIT_TIMEOUT)
    }
}

/// The real git with a chosen timeout; `Default` is [`GIT_TIMEOUT`].
#[derive(Clone, Copy, Debug)]
pub struct TimedGit {
    pub timeout: Duration,
}

impl Default for TimedGit {
    fn default() -> TimedGit {
        TimedGit {
            timeout: GIT_TIMEOUT,
        }
    }
}

impl Git for TimedGit {
    fn run(&self, env: &Env, cwd: &Path, args: &[&str]) -> Option<GitOut> {
        run_git(env, cwd, args, self.timeout)
    }
}

fn run_git(env: &Env, cwd: &Path, args: &[&str], timeout: Duration) -> Option<GitOut> {
    let mut cmd = Command::new("git");
    cmd.arg("-c")
        .arg("core.fsmonitor=false")
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env.iter() {
        if !STRIPPED_VARS.iter().any(|s| OsStr::new(s) == k) {
            cmd.env(k, v);
        }
    }
    let deadline = Instant::now() + timeout;
    let mut child = cmd.spawn().ok()?;
    let stdout = reader(child.stdout.take());
    let stderr = reader(child.stderr.take());
    // As `communicate(timeout)`: read both pipes to their end, then wait for the exit, all
    // within the one deadline.
    let left = || deadline.saturating_duration_since(Instant::now());
    let Ok(stdout) = stdout.recv_timeout(left()) else {
        return kill(child);
    };
    let Ok(stderr) = stderr.recv_timeout(left()) else {
        return kill(child);
    };
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(left().min(Duration::from_millis(2)));
            }
            Ok(None) | Err(_) => return kill(child),
        }
    };
    let code = status
        .code()
        .unwrap_or_else(|| status.signal().map_or(-1, |s| -s));
    Some(GitOut {
        code,
        stdout,
        stderr,
    })
}

/// Reads a pipe to its end on its own thread; the bytes arrive on the channel. If the thread
/// cannot start, the sender is dropped with it and the run ends as "no result".
fn reader(pipe: Option<impl Read + Send + 'static>) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    if let Some(mut pipe) = pipe {
        let _ = std::thread::Builder::new().spawn(move || {
            let mut buf = Vec::new();
            // A read error ends the output early, as a closed pipe would.
            let _ = pipe.read_to_end(&mut buf);
            let _ = tx.send(buf);
        });
    } else {
        let _ = tx.send(Vec::new());
    }
    rx
}

/// Kills and reaps the child; the reader threads end when the pipes close.
fn kill(mut child: Child) -> Option<GitOut> {
    let _ = child.kill();
    let _ = child.wait();
    None
}
