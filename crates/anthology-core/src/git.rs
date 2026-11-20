//! The `Git` trait and `RealGit` (plan §3.3, GC-2): `memory_sync._git`.
//!
//! `git -c core.fsmonitor=false <args>` runs in `cwd` with exactly the given [`Env`] minus
//! `GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE` and `GIT_COMMON_DIR` (never the real process
//! environment), stdin `/dev/null`, and stdout and stderr read by their own threads so a large
//! output cannot deadlock. Past the timeout (10 s, [`GIT_TIMEOUT`]) the child is killed and
//! reaped and the result is `None`, as is a missing `git` or any spawn failure. As with
//! `subprocess.run(timeout=10)`, the timeout also covers reading the pipes to their end, and a
//! killed child's grandchildren are not waited for. After a timeout the reader threads are
//! told to stop: each drops its pipe when its current read returns, so a grandchild that keeps
//! writing gets `SIGPIPE`, as it does once Python's `Popen` closes the pipes (Rust closes them
//! at the grandchild's next write rather than at once, and buffers at most one more chunk).
#![forbid(unsafe_code)]

use std::ffi::OsStr;
use std::io::Read;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
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
    let abandoned = Arc::new(AtomicBool::new(false));
    let stdout = reader(child.stdout.take(), &abandoned);
    let stderr = reader(child.stderr.take(), &abandoned);
    // As `communicate(timeout)`: read both pipes to their end, then wait for the exit, all
    // within the one deadline.
    let left = || deadline.saturating_duration_since(Instant::now());
    let Ok(stdout) = stdout.recv_timeout(left()) else {
        return kill(child, &abandoned);
    };
    let Ok(stderr) = stderr.recv_timeout(left()) else {
        return kill(child, &abandoned);
    };
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(left().min(Duration::from_millis(2)));
            }
            Ok(None) | Err(_) => return kill(child, &abandoned),
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
/// Once `abandoned` is set, the thread stops at its next read and drops the pipe.
fn reader(
    pipe: Option<impl Read + Send + 'static>,
    abandoned: &Arc<AtomicBool>,
) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    if let Some(mut pipe) = pipe {
        let abandoned = Arc::clone(abandoned);
        let _ = std::thread::Builder::new().spawn(move || {
            let mut buf = Vec::new();
            let mut chunk = vec![0u8; 64 * 1024];
            while !abandoned.load(Ordering::Relaxed) {
                match pipe.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    // A read error ends the output early, as a closed pipe would.
                    Err(_) => break,
                }
            }
            drop(pipe);
            let _ = tx.send(buf);
        });
    } else {
        let _ = tx.send(Vec::new());
    }
    rx
}

/// Kills and reaps the child, and tells the reader threads to drop their pipes.
fn kill(mut child: Child, abandoned: &AtomicBool) -> Option<GitOut> {
    abandoned.store(true, Ordering::Relaxed);
    let _ = child.kill();
    let _ = child.wait();
    None
}
