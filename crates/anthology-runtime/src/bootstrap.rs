//! Calling `hooks/bootstrap.sh` (RT2), and doctor's `anthology_binary` check (GC-14, GC-17).
//!
//! [`ensure_live_guard_binary`] is how session start (budget ≤ `BOOTSTRAP_BUDGET` = 20 s,
//! GC-7) and setup (60 s, design §8.8) make sure the live guardrails plugin's pinned binary is
//! installed. It runs **one** `sh <guard>/hooks/bootstrap.sh session --plugin guardrails
//! --quiet --budget N` and never retries, so a session start makes at most one download here:
//! - stdin is `/dev/null`, stderr is discarded, and the environment is exactly the [`Env`];
//! - the script runs in its own process group. The bootstrap's own watchdog bounds its
//!   download by N; this side adds a hard deadline of N + [`GRACE_SECS`] for the hash, extract
//!   and probe, after which the whole group is killed (`kill -KILL -- -<pgid>`, so a hung curl
//!   or `sleep` dies with the shell) and the child is reaped;
//! - the answer is the first stdout line starting `status=` (GC-14): `present` and `installed`
//!   count only with exit 0; `failed:<reason>` keeps the reason as printable ASCII (others
//!   become `?`), cut to [`REASON_MAX`] characters.
//!
//! Outcomes this side adds (lead ruling R7): a guard root without `hooks/bootstrap.sh` is
//! [`SKIPPED_NO_BOOTSTRAP`] (guardrails releases before the Rust guard ship none); the deadline
//! is [`FAILED_TIMED_OUT`]; a spawn error or no status line is [`FAILED_COULD_NOT_RUN`].
//!
//! [`anthology_binary_check`] is doctor's `anthology_binary` (RT1b): it reads the guardrails'
//! `anthology.lock` (the design §8.2 layout included) and checks `<ah>/bin/<version>/` is
//! "installed" (a real folder; `anthology` a regular, non-empty, executable file; `.sha256`
//! non-empty) and "current" (`.sha256` is the lock's hash for this target and `.bin-sha256` is
//! the binary's sha256).
#![forbid(unsafe_code)]

use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anthology_core::lock::{self, ANTHOLOGY_TAG_PREFIX};
use anthology_core::platform;
use anthology_core::{Env, paths};

use crate::rtk_install::{ct_eq, is_executable_file, sha256_file};

/// Seconds allowed after the budget for the hash, extract and `--version` probe.
pub const GRACE_SECS: u64 = 5;
/// The longest bootstrap reason kept.
pub const REASON_MAX: usize = 300;
/// Most stdout bytes read from the bootstrap.
const STDOUT_MAX: usize = 64 * 1024;
/// How long to wait for stdout to close once the shell has exited.
const LINGER: Duration = Duration::from_millis(200);

/// The guard root has no bootstrap (R7).
pub const SKIPPED_NO_BOOTSTRAP: &str = "guardrails bootstrap not found";
/// The hard deadline passed (R7).
pub const FAILED_TIMED_OUT: &str = "the bootstrap timed out";
/// The bootstrap could not be started, or printed no status line (R7).
pub const FAILED_COULD_NOT_RUN: &str = "the bootstrap could not run";

/// What [`ensure_live_guard_binary`] found (plan §3.6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GuardBinaryStatus {
    Present,
    Installed,
    Failed(String),
    Skipped(&'static str),
}

impl GuardBinaryStatus {
    /// Setup's `guard_binary.status` (GC-14): `present`, `installed`, `failed: <reason>` or
    /// `skipped: <why>`.
    pub fn status_text(&self) -> String {
        match self {
            GuardBinaryStatus::Present => "present".into(),
            GuardBinaryStatus::Installed => "installed".into(),
            GuardBinaryStatus::Failed(r) => format!("failed: {r}"),
            GuardBinaryStatus::Skipped(r) => format!("skipped: {r}"),
        }
    }
}

/// Runs the guardrails bootstrap once, within `budget_secs` + [`GRACE_SECS`] (module docs).
pub fn ensure_live_guard_binary(
    guard_root: &Path,
    budget_secs: u64,
    env: &Env,
) -> GuardBinaryStatus {
    ensure_with_grace(
        guard_root,
        budget_secs,
        env,
        Duration::from_secs(GRACE_SECS),
    )
}

/// [`ensure_live_guard_binary`] with another grace period (tests).
pub fn ensure_with_grace(
    guard_root: &Path,
    budget_secs: u64,
    env: &Env,
    grace: Duration,
) -> GuardBinaryStatus {
    let script = guard_root.join("hooks").join("bootstrap.sh");
    if !script.is_file() {
        return GuardBinaryStatus::Skipped(SKIPPED_NO_BOOTSTRAP);
    }
    let deadline = Instant::now() + Duration::from_secs(budget_secs) + grace;
    let budget = budget_secs.to_string();
    let mut cmd = Command::new("/bin/sh");
    cmd.arg(&script)
        .args(["session", "--plugin", "guardrails", "--quiet", "--budget"])
        .arg(&budget)
        .env_clear()
        .envs(env.iter())
        .current_dir(guard_root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let Ok(mut child) = cmd.spawn() else {
        return GuardBinaryStatus::Failed(FAILED_COULD_NOT_RUN.into());
    };
    let pgid = child.id();
    let out = Arc::new(Mutex::new(Vec::new()));
    let done = Arc::new(Mutex::new(false));
    if let Some(mut stdout) = child.stdout.take() {
        let (out, done) = (Arc::clone(&out), Arc::clone(&done));
        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = stdout.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let mut o = out.lock().unwrap_or_else(|e| e.into_inner());
                let room = STDOUT_MAX.saturating_sub(o.len());
                o.extend_from_slice(&buf[..n.min(room)]);
            }
            *done.lock().unwrap_or_else(|e| e.into_inner()) = true;
        });
    } else {
        *done.lock().unwrap_or_else(|e| e.into_inner()) = true;
    }

    let mut exited: Option<(i32, Instant)> = None;
    let mut timed_out = false;
    let mut reader_done;
    loop {
        reader_done = *done.lock().unwrap_or_else(|e| e.into_inner());
        if exited.is_none() {
            match child.try_wait() {
                Ok(Some(status)) => exited = Some((status.code().unwrap_or(-1), Instant::now())),
                Ok(None) => {}
                Err(_) => exited = Some((-1, Instant::now())),
            }
        }
        match exited {
            Some(_) if reader_done => break,
            // The shell is gone but something it started still holds stdout.
            Some((_, at)) if at.elapsed() >= LINGER => break,
            _ => {}
        }
        if Instant::now() >= deadline {
            timed_out = exited.is_none();
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    // Nothing the bootstrap started outlives this call. The group is killed only while it is
    // known to exist (the unreaped shell, or a member still holding stdout), so its id cannot
    // have been reused.
    if exited.is_none() || !reader_done {
        kill_group(pgid);
    }
    if exited.is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }
    if timed_out {
        return GuardBinaryStatus::Failed(FAILED_TIMED_OUT.into());
    }
    let code = exited.map_or(-1, |(c, _)| c);
    let stdout = out.lock().unwrap_or_else(|e| e.into_inner()).clone();
    parse_status(&stdout, code)
}

/// Kills process group `pgid` with the POSIX `kill` utility (safe code: no libc here).
fn kill_group(pgid: u32) {
    let group = format!("-{pgid}");
    let _ = Command::new("/bin/sh")
        .args([
            "-c",
            "kill -KILL -- \"$1\" 2>/dev/null || kill -KILL \"$1\" 2>/dev/null",
            "sh",
            &group,
        ])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// The `--quiet` status line (GC-14) and the exit code → a status (module docs).
pub fn parse_status(stdout: &[u8], code: i32) -> GuardBinaryStatus {
    let text = String::from_utf8_lossy(stdout);
    let Some(line) = text.lines().find(|l| l.starts_with("status=")) else {
        return GuardBinaryStatus::Failed(FAILED_COULD_NOT_RUN.into());
    };
    let value = line["status=".len()..].trim_end_matches('\r');
    match value {
        "present" if code == 0 => GuardBinaryStatus::Present,
        "installed" if code == 0 => GuardBinaryStatus::Installed,
        v => match v.strip_prefix("failed:") {
            Some(reason) => GuardBinaryStatus::Failed(clean_reason(reason)),
            None => GuardBinaryStatus::Failed(FAILED_COULD_NOT_RUN.into()),
        },
    }
}

/// Printable ASCII only (others become `?`), at most [`REASON_MAX`] characters.
fn clean_reason(reason: &str) -> String {
    let cleaned: String = reason
        .chars()
        .map(|c| if (' '..='~').contains(&c) { c } else { '?' })
        .take(REASON_MAX)
        .collect();
    if cleaned.is_empty() {
        FAILED_COULD_NOT_RUN.into()
    } else {
        cleaned
    }
}

// --- doctor's anthology_binary --------------------------------------------------------------

/// `anthology_binary` when the guardrails plugin is not found (GC-14).
pub const CHECK_NOT_FOUND: &str = "anthology-guardrails not found; nothing to check";
/// `anthology_binary` when the binary is current (GC-14).
pub const CHECK_OK: &str = "the anthology binary matches anthology.lock";
/// `anthology_binary` when the guardrails' lock is missing or invalid (lead ruling R8).
pub const CHECK_BAD_LOCK: &str =
    "the guardrails' anthology.lock is not valid; reinstall anthology-guardrails";

/// Doctor's `anthology_binary` check for this machine (module docs, GC-14).
pub fn anthology_binary_check(guard_root: Option<&Path>, env: &Env) -> (Option<bool>, String) {
    let (system, machine) = platform::uname();
    anthology_binary_check_with(guard_root, env, (&system, &machine))
}

/// [`anthology_binary_check`] for the platform `uname` (the `platform` seam).
pub fn anthology_binary_check_with(
    guard_root: Option<&Path>,
    env: &Env,
    uname: (&str, &str),
) -> (Option<bool>, String) {
    let Some(root) = guard_root else {
        return (None, CHECK_NOT_FOUND.into());
    };
    let lock = fs::read(root.join("anthology.lock"))
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|text| lock::parse(&text, ANTHOLOGY_TAG_PREFIX).ok());
    let Some(lock) = lock else {
        return (Some(false), CHECK_BAD_LOCK.into());
    };
    let Ok(target) = platform::detect_with(uname) else {
        return (
            None,
            format!(
                "there is no guard binary for this platform ({}/{})",
                uname.0, uname.1
            ),
        );
    };
    let not_installed = || {
        (
            Some(false),
            format!(
                "the guardrails' anthology binary ({}) is not installed; the guard blocks shell \
                 commands, file edits and MCP tools until it is; run /anthology:setup",
                lock.version
            ),
        )
    };
    let vdir = paths::anthology_home(env).join("bin").join(&lock.version);
    let bin = vdir.join("anthology");
    let installed = fs::symlink_metadata(&vdir).is_ok_and(|m| m.file_type().is_dir())
        && is_executable_file(&bin)
        && fs::symlink_metadata(&bin).is_ok_and(|m| m.len() > 0)
        && fs::read(vdir.join(".sha256")).is_ok_and(|b| !b.is_empty());
    if !installed {
        return not_installed();
    }
    let marker = |name: &str| {
        fs::read(vdir.join(name))
            .map(|b| String::from_utf8_lossy(&b).trim().to_string())
            .unwrap_or_default()
    };
    let expected = lock.sha256.get(&target).map(String::as_str).unwrap_or("");
    let tarball_ok = ct_eq(marker(".sha256").as_bytes(), expected.as_bytes());
    let binary_ok = sha256_file(&bin)
        .is_ok_and(|actual| ct_eq(marker(".bin-sha256").as_bytes(), actual.as_bytes()));
    if tarball_ok && binary_ok {
        (Some(true), CHECK_OK.into())
    } else {
        (
            Some(false),
            format!(
                "the guardrails' anthology binary ({}) does not match its install marker; start \
                 a new session to reinstall it",
                lock.version
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_lines_parse() {
        assert_eq!(parse_status(b"status=present\n", 0), GuardBinaryStatus::Present);
        assert_eq!(parse_status(b"status=installed\n", 0), GuardBinaryStatus::Installed);
        assert_eq!(
            parse_status(b"status=failed:download failed\n", 1),
            GuardBinaryStatus::Failed("download failed".into())
        );
        assert_eq!(
            parse_status(b"noise\nstatus=failed:rate-limited\r\nstatus=present\n", 1),
            GuardBinaryStatus::Failed("rate-limited".into())
        );
        for (out, code) in [
            (&b"status=present\n"[..], 1),
            (b"status=installed\n", 2),
            (b"", 0),
            (b"present\n", 0),
            (b"status=weird\n", 0),
            (b"status=failed:\n", 1),
        ] {
            assert_eq!(
                parse_status(out, code),
                GuardBinaryStatus::Failed(FAILED_COULD_NOT_RUN.into()),
                "{out:?} {code}"
            );
        }
    }

    #[test]
    fn reasons_are_printable_ascii_and_capped() {
        let long = format!("status=failed:{}\u{1b}[31m\u{e9}\n", "x".repeat(400));
        let GuardBinaryStatus::Failed(r) = parse_status(long.as_bytes(), 1) else {
            panic!()
        };
        assert_eq!(r.chars().count(), REASON_MAX);
        let GuardBinaryStatus::Failed(r) = parse_status("status=failed:a\u{1b}b\u{e9}".as_bytes(), 1)
        else {
            panic!()
        };
        assert_eq!(r, "a?b?");
    }

    #[test]
    fn status_text_is_setup_s_wording() {
        assert_eq!(GuardBinaryStatus::Present.status_text(), "present");
        assert_eq!(GuardBinaryStatus::Installed.status_text(), "installed");
        assert_eq!(
            GuardBinaryStatus::Failed("download failed".into()).status_text(),
            "failed: download failed"
        );
        assert_eq!(
            GuardBinaryStatus::Skipped("guardrails not found").status_text(),
            "skipped: guardrails not found"
        );
    }
}
