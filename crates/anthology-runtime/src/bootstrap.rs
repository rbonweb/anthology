//! Calling `hooks/bootstrap.sh` (RT2), and doctor's `anthology_binary` check (GC-14, GC-17).
//!
//! [`ensure_live_guard_binary`] is how session start (budget ≤ `BOOTSTRAP_BUDGET` = 20 s,
//! GC-7) and setup (60 s, design §8.8) make sure the live guardrails plugin's pinned binary is
//! installed. It runs **one** `sh <guard>/hooks/bootstrap.sh session --plugin guardrails
//! --quiet --budget N` and never retries, so a session start makes at most one download here:
//! - stdin is `/dev/null`, stderr is discarded, and the environment is exactly the [`Env`];
//! - the script runs as the child of a small group leader ([`LEADER`]) in a process group of
//!   its own. When the bootstrap ends, the leader prints its exit code and kills the whole
//!   group, so nothing it started outlives the call on any path. The bootstrap's own watchdog
//!   bounds its download by N; this side adds a hard deadline of N + [`GRACE_SECS`] for the
//!   hash, extract and probe, after which the group gets `TERM` (the bootstrap's traps remove
//!   its staging folder), then `KILL`, and the leader is reaped;
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
//! "installed" (`<ah>/bin` and `<version>` real folders; `anthology` a regular, non-empty,
//! executable file; `.sha256` non-empty) and "current" (the first line of `.sha256` is the
//! lock's hash for this target and the first line of `.bin-sha256` is the binary's sha256), as
//! `lib.sh` `anth_installed` and `anth_current` do. `<ah>` is `paths::install_home`, so a
//! relative or empty `HOME` or `CLAUDE_CONFIG_DIR` is [`CHECK_NO_HOME`], as the shims refuse it.
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

use crate::atl_install::{ct_eq, is_executable_file, sha256_file};

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
/// Setup's `skipped: guardrails not found` (GC-14), for callers that found no guard root.
pub const SKIPPED_NOT_FOUND: &str = "guardrails not found";
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
    let start = Instant::now();
    let deadline = start
        .checked_add(Duration::from_secs(budget_secs).saturating_add(grace))
        .unwrap_or_else(|| start + Duration::from_secs(365 * 86400));
    let budget = budget_secs.to_string();
    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-c", LEADER, "anthology-bootstrap"])
        .arg(&script)
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

    let reader_done = || *done.lock().unwrap_or_else(|e| e.into_inner());
    let mut exited_at: Option<Instant> = None;
    let timed_out = loop {
        if exited_at.is_none() && !matches!(child.try_wait(), Ok(None)) {
            exited_at = Some(Instant::now());
        }
        match exited_at {
            Some(_) if reader_done() => break false,
            // The leader is gone (its last act killed the group), but something that left the
            // group still holds stdout: do not wait for it.
            Some(at) if at.elapsed() >= LINGER => break false,
            _ => {}
        }
        if exited_at.is_none() && Instant::now() >= deadline {
            break true;
        }
        thread::sleep(Duration::from_millis(10));
    };
    if timed_out {
        // The leader is not reaped yet, so the group id is still reserved: TERM lets the
        // bootstrap's traps clean up its staging folder, then KILL ends whatever is left.
        signal_group(pgid, "TERM");
        thread::sleep(TERM_WAIT);
        signal_group(pgid, "KILL");
        let _ = child.wait();
        return GuardBinaryStatus::Failed(FAILED_TIMED_OUT.into());
    }
    // A reader that is merely slow still gets its last bytes copied.
    let until = Instant::now() + LINGER;
    while !reader_done() && Instant::now() < until {
        thread::sleep(Duration::from_millis(5));
    }
    let stdout = out.lock().unwrap_or_else(|e| e.into_inner()).clone();
    parse_status(&stdout, exit_code(&stdout))
}

/// The group leader: runs the bootstrap as its child, reports the child's exit code on its own
/// line, then kills its whole process group (itself included), so nothing the bootstrap
/// started — a watchdog, a downloader, a `sleep` — outlives the call, whatever path the
/// bootstrap took. Only a process that left the group (`setsid`) escapes.
const LEADER: &str = "script=$1; shift; /bin/sh \"$script\" \"$@\"; rc=$?; \
                      printf '\\nanthology-bootstrap-exit=%s\\n' \"$rc\"; kill -9 0";

/// The marker [`LEADER`] prints the bootstrap's exit code after.
const EXIT_MARK: &str = "anthology-bootstrap-exit=";

/// How long a timed-out group gets between TERM and KILL.
const TERM_WAIT: Duration = Duration::from_millis(200);

/// The bootstrap's exit code from [`LEADER`]'s line; -1 when it is missing.
fn exit_code(stdout: &[u8]) -> i32 {
    String::from_utf8_lossy(stdout)
        .lines()
        .rev()
        .find_map(|l| l.strip_prefix(EXIT_MARK))
        .and_then(|c| c.trim().parse().ok())
        .unwrap_or(-1)
}

/// Sends `signal` to process group `pgid` with the POSIX `kill` utility (safe code: no libc).
fn signal_group(pgid: u32, signal: &str) {
    let group = format!("-{pgid}");
    let _ = Command::new("/bin/sh")
        .args([
            "-c",
            "kill -s \"$1\" -- \"$2\" 2>/dev/null || kill -s \"$1\" \"$2\" 2>/dev/null",
            "sh",
            signal,
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
/// `anthology_binary` when `<ah>` is not an absolute path (`paths::install_home`): the shims
/// refuse it the same way (`lib.sh` `anth_home_ok`).
pub const CHECK_NO_HOME: &str = "HOME or CLAUDE_CONFIG_DIR is not an absolute path, so the guard \
     cannot find its binary; it blocks shell commands, file edits and MCP tools until it can";

/// `lib.sh` `anth_installed`: `<ah>/bin` and `<version>` are real folders, `anthology` is a
/// regular, non-empty, executable file, and `.sha256` is non-empty.
fn is_installed(bindir: &Path, vdir: &Path) -> bool {
    let real_dir = |p: &Path| fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_dir());
    let bin = vdir.join("anthology");
    real_dir(bindir)
        && real_dir(vdir)
        && is_executable_file(&bin)
        && fs::symlink_metadata(&bin).is_ok_and(|m| m.len() > 0)
        && fs::read(vdir.join(".sha256")).is_ok_and(|b| !b.is_empty())
}

/// Whether the binary the guardrails' `anthology.lock` pins is installed right now, as the
/// guard shim's hot path sees it (`anth_installed`: no hashing). `false` when the lock does not
/// read or `<ah>` is not usable.
pub fn guard_binary_installed(guard_root: &Path, env: &Env) -> bool {
    let lock = fs::read(guard_root.join("anthology.lock"))
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|text| lock::parse(&text, ANTHOLOGY_TAG_PREFIX).ok());
    let (Some(lock), Some(ah)) = (lock, paths::install_home(env)) else {
        return false;
    };
    let bindir = ah.join("bin");
    is_installed(&bindir, &bindir.join(&lock.version))
}

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
    // The `<ah>` the shims use: none for a relative or empty HOME or CLAUDE_CONFIG_DIR.
    let Some(ah) = paths::install_home(env) else {
        return (Some(false), CHECK_NO_HOME.into());
    };
    let vdir = ah.join("bin").join(&lock.version);
    let bin = vdir.join("anthology");
    if !is_installed(&ah.join("bin"), &vdir) {
        return not_installed();
    }
    // As `anth_first_line` reads a marker: the first line, without its `\n`.
    let marker = |name: &str| {
        fs::read(vdir.join(name))
            .map(|b| {
                let line = b.split(|&c| c == b'\n').next().unwrap_or_default();
                String::from_utf8_lossy(line).into_owned()
            })
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
        assert_eq!(
            parse_status(b"status=present\n", 0),
            GuardBinaryStatus::Present
        );
        assert_eq!(
            parse_status(b"status=installed\n", 0),
            GuardBinaryStatus::Installed
        );
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
        let GuardBinaryStatus::Failed(r) =
            parse_status("status=failed:a\u{1b}b\u{e9}".as_bytes(), 1)
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
