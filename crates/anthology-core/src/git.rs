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
//!
//! The `git` that runs is found once per call, on the absolute entries of the `Env`'s `PATH` only
//! ([`resolve_git`]), and run by its absolute path: the working directory is inside a repository,
//! so a `.`, empty or relative entry must not find a `git` the repository holds. No such git
//! found is a missing git. The child's own `PATH` is cut to the same absolute entries, so what
//! git starts (ssh, credential helpers, hooks) is not found through a relative one either.
//!
//! The spawn itself is also bounded by the timeout. On macOS, `Command::spawn` creates its
//! pipes with `pipe()` and sets `FD_CLOEXEC` in a second step, and (with `PATH` in the child's
//! environment) forks rather than using `posix_spawn`, then reads a status pipe until every
//! copy of its write end is closed. A fork by another thread between those two steps hands the
//! pipe to that thread's child, and `spawn` then blocks until that child (or one of its
//! descendants, such as a `sleep`) exits. So spawns take turns (`SPAWN`), and a spawn that
//! still stalls is abandoned at the deadline instead of holding the caller. A stalled spawn
//! holds `SPAWN`, so later calls in the process wait behind it and give up at their own
//! deadlines (each returns `None`); their helpers then start no git at all.
#![forbid(unsafe_code)]

use std::ffi::OsStr;
use std::io::Read;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
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

/// The first executable file called `name` in the absolute entries of `path`, as found. Empty and
/// relative entries (`.`, `::`, `bin`) are skipped: they resolve against the working directory,
/// where a repository could plant a program of that name. `None` when `path` is unset or no
/// absolute entry holds one, as for a tool that is not installed.
pub fn find_on_absolute_path(name: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(path?)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(name))
        .find(|candidate| {
            std::fs::metadata(candidate)
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

/// `path` with its empty and relative entries dropped, in order.
fn absolute_entries(path: &OsStr) -> std::ffi::OsString {
    let dirs = std::env::split_paths(path).filter(|dir| dir.is_absolute());
    std::env::join_paths(dirs).unwrap_or_default()
}

/// The `git` that `env`'s `PATH` finds among its absolute entries, to be run by its absolute path
/// (never by the bare name, which the OS would look up in relative entries too).
pub fn resolve_git(env: &Env) -> Option<PathBuf> {
    find_on_absolute_path("git", env.get_os("PATH"))
}

fn run_git(env: &Env, cwd: &Path, args: &[&str], timeout: Duration) -> Option<GitOut> {
    let mut cmd = Command::new(resolve_git(env)?);
    cmd.arg("-c")
        .arg("core.fsmonitor=false")
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env.iter() {
        if STRIPPED_VARS.iter().any(|s| OsStr::new(s) == k) {
            continue;
        }
        if k == "PATH" {
            // What git starts (ssh, credential helpers, hooks) is found on the absolute entries
            // only, as git itself was. `git` was found on one, so the list is not empty.
            cmd.env(k, absolute_entries(v));
        } else {
            cmd.env(k, v);
        }
    }
    let deadline = Instant::now() + timeout;
    let mut child = spawn(cmd, deadline)?;
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

/// Held while a git is being spawned, so that two spawns in this process never overlap (see the
/// module docs). Only spawns by `RealGit` and `TimedGit` take it; the lock is released as soon
/// as `Command::spawn` returns.
static SPAWN: Mutex<()> = Mutex::new(());

/// Spawns `cmd` on a helper thread, one spawn at a time, and waits for it only until
/// `deadline`. `None` when the spawn fails or is not done by then. A helper that gets its turn
/// after the deadline starts nothing. A child that is spawned after the caller has given up is
/// killed and reaped by the helper: the channel holds no value (capacity 0), so the send fails
/// once the caller's receiver is gone, where a buffered value would be dropped with it, unkilled
/// and unreaped.
fn spawn(mut cmd: Command, deadline: Instant) -> Option<Child> {
    let (tx, rx) = mpsc::sync_channel(0);
    std::thread::Builder::new()
        .spawn(move || {
            let spawned = {
                let _one_at_a_time = SPAWN.lock().unwrap_or_else(PoisonError::into_inner);
                if Instant::now() >= deadline {
                    return;
                }
                cmd.spawn()
            };
            #[cfg(test)]
            tests::after_spawn(&spawned);
            if let Err(mpsc::SendError(Ok(mut late))) = tx.send(spawned) {
                let _ = late.kill();
                let _ = late.wait();
            }
        })
        .ok()?;
    let got = rx.recv_timeout(deadline.saturating_duration_since(Instant::now()));
    #[cfg(test)]
    tests::after_recv();
    got.ok()?.ok()
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use std::sync::atomic::{AtomicU64, AtomicUsize};

    /// Gits started by `spawn`; the pause (ms) between a start and the hand-over; and the pause
    /// (ms) the caller takes after its wait ends, before it drops the receiver.
    static SPAWNED: AtomicUsize = AtomicUsize::new(0);
    static HOLD_MS: AtomicU64 = AtomicU64::new(0);
    static LINGER_MS: AtomicU64 = AtomicU64::new(0);
    /// The tests below read the statics, so they run one at a time.
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

    /// A test hook in `spawn`: counts a started git and can hold it before the hand-over.
    pub(super) fn after_spawn(spawned: &io::Result<Child>) {
        if spawned.is_ok() {
            SPAWNED.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(HOLD_MS.load(Ordering::SeqCst)));
        }
    }

    /// A test hook in `spawn`: the caller waits here with the receiver still open.
    pub(super) fn after_recv() {
        std::thread::sleep(Duration::from_millis(LINGER_MS.load(Ordering::SeqCst)));
    }

    /// A folder with a fake `git` running `body`, as the shell writes it (not this process, so
    /// no writable descriptor is open when other threads fork), and an `Env` that finds it.
    fn fake_git(body: &str) -> (tempfile::TempDir, Env) {
        let dir = tempfile::tempdir().expect("tempdir");
        let git = dir.path().join("git");
        let status = Command::new("/bin/sh")
            .arg("-c")
            .arg("printf '#!/bin/sh\\n%s\\n' \"$1\" > \"$2\" && chmod 755 \"$2\"")
            .arg("sh")
            .arg(body)
            .arg(&git)
            .status()
            .expect("sh");
        assert!(status.success());
        let env = Env::default().with_os("PATH", format!("{}:/usr/bin:/bin", dir.path().display()));
        (dir, env)
    }

    /// A spawn that is stuck (here: waiting for `SPAWN`, as a spawn stalled on a leaked status
    /// pipe would hold it) gives the caller its `None` at the timeout, not when the spawn ends,
    /// and the helper, once it gets its turn, starts no git for the caller that left.
    #[test]
    fn a_stuck_spawn_is_abandoned_at_the_timeout() {
        let _seq = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
        let (_d, env) = fake_git("exit 0");
        let before = SPAWNED.load(Ordering::SeqCst);
        let stuck = SPAWN.lock().unwrap_or_else(PoisonError::into_inner);
        let git = RealGit::with_timeout(Duration::from_millis(300));
        let start = Instant::now();
        assert_eq!(git.run(&env, Path::new("/"), &[]), None);
        let took = start.elapsed();
        drop(stuck);
        assert!(took >= Duration::from_millis(300), "{took:?}");
        assert!(took < Duration::from_secs(5), "{took:?}");
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(SPAWNED.load(Ordering::SeqCst), before, "a git was started");
    }

    /// The git is started and held for 200 ms before the hand-over, so the caller's wait (100 ms)
    /// has ended, and the caller keeps its receiver for 400 ms. The helper must kill and reap
    /// the git: with a buffered channel the send succeeds into that open receiver, the child is
    /// dropped with it, and it keeps running.
    #[test]
    fn a_git_started_after_the_caller_gave_up_is_killed_and_reaped() {
        let _seq = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
        let marks = tempfile::tempdir().expect("marks");
        let pidfile = marks.path().join("pid");
        let (_d, env) = fake_git("echo $$ > \"$PIDFILE\"; exec sleep 30");
        let env = env.with_os("PIDFILE", &pidfile);
        HOLD_MS.store(200, Ordering::SeqCst);
        LINGER_MS.store(400, Ordering::SeqCst);
        let git = RealGit::with_timeout(Duration::from_millis(100));
        let out = git.run(&env, Path::new("/"), &[]);
        HOLD_MS.store(0, Ordering::SeqCst);
        LINGER_MS.store(0, Ordering::SeqCst);
        assert_eq!(out, None);
        let pid = wait_for(|| {
            std::fs::read_to_string(&pidfile)
                .ok()
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
        })
        .expect("the fake git wrote its pid");
        // `ps` prints nothing once the process is gone; a zombie shows as `Z`.
        let gone = wait_for(|| {
            let ps = Command::new("ps")
                .args(["-o", "stat=", "-p", &pid])
                .output()
                .expect("ps");
            String::from_utf8_lossy(&ps.stdout)
                .trim()
                .is_empty()
                .then_some(())
        });
        assert!(gone.is_some(), "git {pid} is still there");
    }

    /// Polls `f` for up to 8 s.
    fn wait_for<T>(f: impl Fn() -> Option<T>) -> Option<T> {
        let end = Instant::now() + Duration::from_secs(8);
        loop {
            if let Some(v) = f() {
                return Some(v);
            }
            if Instant::now() >= end {
                return None;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
