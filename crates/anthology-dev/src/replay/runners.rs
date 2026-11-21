//! `replay` part `runners`: the four ways a replayed call is guarded (design §13.7, m-R7).
//!
//! 1. Python `guard.py` (the authority), run as Claude Code runs it.
//! 2. `anthology guard hook` (plain mode) of the binary under test.
//! 3. The staged `hooks/guard.sh` with the binary under test **seeded** into a scratch
//!    `CLAUDE_CONFIG_DIR`.
//! 4. The staged `hooks/guard.sh` with the binary **missing** from a scratch
//!    `CLAUDE_CONFIG_DIR` and `ANTHOLOGY_GUARD_NO_FETCH=1`, so it never installs one.
//!
//! Every runner gets the same base environment (the real `HOME`, so commands that name it
//! resolve as they did) and runs in a scratch folder, so none reads or writes the owner's
//! `~/.claude/anthology/bin` or the repository (`PYTHONDONTWRITEBYTECODE=1`). The shim runs
//! from a scratch copy of the staged hooks next to the staged `anthology.lock` when there is
//! one, else next to a lock written for the binary under test.
//!
//! Runner 3 also sets `ANTHOLOGY_GUARD_NO_FETCH=1`: it changes nothing while the seeded binary
//! is installed (the shim only reads it for a missing binary), and it keeps a quarantine (a
//! runner-3 difference anyway) from ever downloading a release. A quarantined seed is put back
//! before the next call and counted.
//!
//! Each call is bounded ([`TIMEOUT`]); past it the whole process group is killed and the call's
//! exit code is [`TIMED_OUT`].

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use anthology_core::Env;
use anthology_core::lock::{self, ANTHOLOGY_TAG_PREFIX, Lock};
use anthology_core::platform::{self, Target};

use super::hmac::hex;

/// One runner (module docs).
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Runner {
    Python = 1,
    Hook = 2,
    Shim = 3,
    Missing = 4,
}

impl Runner {
    pub const ALL: [Runner; 4] = [Runner::Python, Runner::Hook, Runner::Shim, Runner::Missing];

    pub fn number(self) -> u8 {
        self as u8
    }

    /// `1,2,3,4` (any subset, any order) → the runners in order.
    pub fn parse_list(s: &str) -> Result<Vec<Runner>, String> {
        let mut out = Vec::new();
        for part in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let r = Runner::ALL
                .into_iter()
                .find(|r| r.number().to_string() == part)
                .ok_or_else(|| format!("unknown runner {part:?} (runners are 1, 2, 3 and 4)"))?;
            if !out.contains(&r) {
                out.push(r);
            }
        }
        if out.is_empty() {
            return Err("--runners names no runner".into());
        }
        out.sort();
        Ok(out)
    }
}

/// What a runner did with one call: its exit code, stdout and stderr (compared byte for byte),
/// and how long it took (not compared; it corroborates a D-7 limit block).
#[derive(Clone, Debug, Default)]
pub struct RunOut {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub elapsed: Duration,
}

impl PartialEq for RunOut {
    fn eq(&self, other: &RunOut) -> bool {
        (self.code, &self.stdout, &self.stderr) == (other.code, &other.stdout, &other.stderr)
    }
}

impl Eq for RunOut {}

impl RunOut {
    /// Exit 0 with an empty stdout (the hook contract: Claude Code parses stdout on exit 0).
    pub fn allowed(&self) -> bool {
        self.code == 0 && self.stdout.is_empty()
    }

    /// A well-formed guard block.
    pub fn blocked(&self) -> bool {
        self.code == 2
            && self.stdout.is_empty()
            && self
                .stderr
                .starts_with(b"Blocked by anthology-guardrails (")
    }

    /// A block by one of the rules G1-G6 (not `(setup)` or `(internal)`).
    pub fn rule_block(&self) -> bool {
        self.blocked()
            && self
                .stderr
                .starts_with(b"Blocked by anthology-guardrails (G")
    }

    /// The call hit [`TIMEOUT`] or could not start.
    pub fn no_answer(&self) -> bool {
        self.code == TIMED_OUT || self.code == NOT_STARTED
    }
}

/// The per-call bound.
pub const TIMEOUT: Duration = Duration::from_secs(30);
/// The exit code of a call that hit [`TIMEOUT`].
pub const TIMED_OUT: i32 = -2;
/// The exit code of a call that could not start.
pub const NOT_STARTED: i32 = -1;

/// The staged guardrails plugin, relative to the repository root.
pub const STAGED_PLUGIN: &str = "port/staging/plugins/anthology-guardrails";
/// Python's guard, relative to the repository root.
pub const GUARD_PY: &str = "plugins/anthology-guardrails/scripts/guard.py";

/// Everything the runners share.
pub struct Setup {
    python: PathBuf,
    guard_py: PathBuf,
    bin: Option<PathBuf>,
    base: Env,
    scratch: tempfile::TempDir,
    plugin: PathBuf,
    version: String,
    lock_sha: String,
    bin_sha: String,
}

/// One worker's scratch folders.
pub struct Slot {
    cwd: PathBuf,
    tmp: PathBuf,
    r3: PathBuf,
    r4: PathBuf,
    /// How many times runner 3's shim quarantined the seeded binary (it is put back each time).
    pub quarantined: u64,
}

impl Setup {
    /// `repo` is the repository root, `bin` the binary under test (needed by runners 2 and 3),
    /// `base` the base environment.
    pub fn new(
        repo: &Path,
        python: &Path,
        bin: Option<&Path>,
        base: Env,
        runners: &[Runner],
    ) -> Result<Setup, String> {
        let needs_bin = runners.contains(&Runner::Hook) || runners.contains(&Runner::Shim);
        let bin = match bin {
            Some(b) => {
                Some(fs::canonicalize(b).map_err(|e| format!("--bin {}: {e}", b.display()))?)
            }
            None if needs_bin => return Err("runners 2 and 3 need --bin <anthology binary>".into()),
            None => None,
        };
        let scratch = tempfile::Builder::new()
            .prefix("anthology-replay-")
            .tempdir()
            .map_err(|e| format!("cannot make a scratch folder: {e}"))?;
        let plugin = scratch.path().join("plugin");
        let mut setup = Setup {
            python: python.to_path_buf(),
            guard_py: repo.join(GUARD_PY),
            bin,
            base,
            plugin,
            scratch,
            version: String::new(),
            lock_sha: String::new(),
            bin_sha: String::new(),
        };
        if runners.contains(&Runner::Python) && !setup.guard_py.is_file() {
            return Err(format!("no {} in {}", GUARD_PY, repo.display()));
        }
        if let Some(b) = &setup.bin {
            let bytes = fs::read(b).map_err(|e| format!("{}: {e}", b.display()))?;
            setup.bin_sha = hex(&Sha256::digest(&bytes));
        }
        if runners.contains(&Runner::Shim) || runners.contains(&Runner::Missing) {
            setup.stage_plugin(repo)?;
        }
        Ok(setup)
    }

    /// The scratch copy of the staged guardrails hooks and its lock (module docs).
    fn stage_plugin(&mut self, repo: &Path) -> Result<(), String> {
        let staged = repo.join(STAGED_PLUGIN);
        let hooks = self.plugin.join("hooks");
        fs::create_dir_all(&hooks).map_err(|e| format!("{}: {e}", hooks.display()))?;
        let entries = fs::read_dir(staged.join("hooks"))
            .map_err(|e| format!("{}/hooks: {e}", staged.display()))?;
        for entry in entries.flatten() {
            let from = entry.path();
            if from.is_file() {
                fs::copy(&from, hooks.join(entry.file_name()))
                    .map_err(|e| format!("{}: {e}", from.display()))?;
            }
        }
        let target =
            platform::detect_with((platform::uname().0.as_str(), platform::uname().1.as_str()))?;
        let staged_lock = staged.join("anthology.lock");
        let text = if staged_lock.is_file() {
            fs::read_to_string(&staged_lock)
                .map_err(|e| format!("{}: {e}", staged_lock.display()))?
        } else {
            let version = match &self.bin {
                Some(b) => binary_version(b)?,
                None => "0.0.0".to_string(),
            };
            let digest = if self.bin_sha.is_empty() {
                "0".repeat(64)
            } else {
                self.bin_sha.clone()
            };
            lock::render_anthology(&Lock {
                version: version.clone(),
                tag: format!("{ANTHOLOGY_TAG_PREFIX}{version}"),
                repo: "replay/scratch".into(),
                sha256: Target::ALL
                    .into_iter()
                    .map(|t| (t, digest.clone()))
                    .collect(),
            })
        };
        let parsed = lock::parse(&text, ANTHOLOGY_TAG_PREFIX)
            .map_err(|e| format!("the guardrails anthology.lock is not valid: {e}"))?;
        self.version = parsed.version.clone();
        self.lock_sha = parsed.sha256.get(&target).cloned().unwrap_or_default();
        fs::write(self.plugin.join("anthology.lock"), text)
            .map_err(|e| format!("cannot write the scratch lock: {e}"))
    }

    /// A worker's scratch folders; runner 3's is seeded.
    pub fn slot(&self, n: usize) -> Result<Slot, String> {
        let root = self.scratch.path().join(format!("w{n}"));
        let slot = Slot {
            cwd: root.join("cwd"),
            tmp: root.join("tmp"),
            r3: root.join("r3"),
            r4: root.join("r4"),
            quarantined: 0,
        };
        for d in [&slot.cwd, &slot.tmp, &slot.r3, &slot.r4] {
            fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        if self.bin.is_some() && !self.version.is_empty() {
            self.seed(&slot)?;
        }
        Ok(slot)
    }

    fn vdir(&self, slot: &Slot) -> PathBuf {
        slot.r3.join("anthology/bin").join(&self.version)
    }

    /// `<r3>/anthology/bin/<version>/{anthology,.bin-sha256,.sha256}`, replacing whatever the
    /// shim left there.
    fn seed(&self, slot: &Slot) -> Result<(), String> {
        let Some(bin) = &self.bin else { return Ok(()) };
        let bindir = slot.r3.join("anthology/bin");
        let _ = fs::remove_dir_all(&bindir);
        let vdir = self.vdir(slot);
        fs::create_dir_all(&vdir).map_err(|e| format!("{}: {e}", vdir.display()))?;
        let dest = vdir.join("anthology");
        fs::copy(bin, &dest).map_err(|e| format!("cannot seed {}: {e}", dest.display()))?;
        fs::set_permissions(&dest, fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("{}: {e}", dest.display()))?;
        fs::write(vdir.join(".bin-sha256"), format!("{}\n", self.bin_sha))
            .and_then(|()| fs::write(vdir.join(".sha256"), format!("{}\n", self.lock_sha)))
            .map_err(|e| format!("cannot seed {}: {e}", vdir.display()))
    }

    /// Runs `runner` on `envelope`.
    pub fn run(&self, slot: &mut Slot, runner: Runner, envelope: &[u8]) -> RunOut {
        let env = self
            .base
            .clone()
            .with_os("TMPDIR", &slot.tmp)
            .with("PYTHONDONTWRITEBYTECODE", "1");
        let sh = Path::new("/bin/sh");
        let guard_sh = self.plugin.join("hooks/guard.sh");
        match runner {
            Runner::Python => exec(
                &self.python,
                &[self.guard_py.as_os_str()],
                envelope,
                &env,
                &slot.cwd,
            ),
            Runner::Hook => match &self.bin {
                Some(b) => exec(
                    b,
                    &["guard".as_ref(), "hook".as_ref()],
                    envelope,
                    &env,
                    &slot.cwd,
                ),
                None => not_started("no --bin"),
            },
            Runner::Shim => {
                let seeded = |slot: &Slot| self.vdir(slot).join("anthology").is_file();
                // A seed that could not be put back after the last call is retried here.
                if self.bin.is_some()
                    && !seeded(slot)
                    && let Err(e) = self.seed(slot)
                {
                    return not_started(&e);
                }
                let env = env
                    .with_os("CLAUDE_CONFIG_DIR", &slot.r3)
                    .with("ANTHOLOGY_GUARD_NO_FETCH", "1");
                let out = exec(sh, &[guard_sh.as_os_str()], envelope, &env, &slot.cwd);
                if self.bin.is_some() && !seeded(slot) {
                    slot.quarantined += 1;
                    let _ = self.seed(slot);
                }
                out
            }
            Runner::Missing => {
                let env = env
                    .with_os("CLAUDE_CONFIG_DIR", &slot.r4)
                    .with("ANTHOLOGY_GUARD_NO_FETCH", "1");
                exec(sh, &[guard_sh.as_os_str()], envelope, &env, &slot.cwd)
            }
        }
    }
}

/// `<version>` from `anthology --version` (`anthology <version> (<target>)`).
pub fn binary_version(bin: &Path) -> Result<String, String> {
    let out = Command::new(bin)
        .arg("--version")
        .env_clear()
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("cannot run {}: {e}", bin.display()))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout.lines().next().unwrap_or("");
    line.strip_prefix("anthology ")
        .and_then(|rest| rest.split(' ').next())
        .filter(|v| !v.is_empty() && out.status.success())
        .map(str::to_string)
        .ok_or_else(|| format!("{} --version printed {line:?}", bin.display()))
}

fn not_started(why: &str) -> RunOut {
    RunOut {
        code: NOT_STARTED,
        stderr: why.as_bytes().to_vec(),
        ..RunOut::default()
    }
}

/// Runs `program` with exactly `env`, `stdin` on its standard input, bounded by [`TIMEOUT`].
pub fn exec(
    program: &Path,
    args: &[&std::ffi::OsStr],
    stdin: &[u8],
    env: &Env,
    cwd: &Path,
) -> RunOut {
    exec_bounded(program, args, stdin, env, cwd, TIMEOUT)
}

/// [`exec`] with the bound as a parameter.
pub fn exec_bounded(
    program: &Path,
    args: &[&std::ffi::OsStr],
    stdin: &[u8],
    env: &Env,
    cwd: &Path,
    timeout: Duration,
) -> RunOut {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(cwd)
        .env_clear()
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env.iter() {
        cmd.env(k, v);
    }
    let started = Instant::now();
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return not_started(&format!("cannot run {}: {e}", program.display())),
    };
    let pid = child.id();
    if let Some(mut pipe) = child.stdin.take() {
        let input = stdin.to_vec();
        // A program that exits without reading its input closes the pipe: not an error.
        thread::spawn(move || {
            let _ = pipe.write_all(&input);
        });
    }
    let out = child.stdout.take().map(reader);
    let err = child.stderr.take().map(reader);
    let deadline = started + timeout;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(_) => break None,
        }
        if Instant::now() >= deadline {
            kill_group(pid);
            timed_out = true;
            break child.wait().ok();
        }
        thread::sleep(Duration::from_millis(1));
    };
    let elapsed = started.elapsed();
    let mut killed = timed_out;
    let mut collect = |rx: Option<mpsc::Receiver<Vec<u8>>>| -> Vec<u8> {
        let Some(rx) = rx else { return Vec::new() };
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left.max(Duration::from_millis(100))) {
            Ok(b) => b,
            Err(_) => {
                // Something the program started still holds the pipe.
                if !killed {
                    kill_group(pid);
                    killed = true;
                }
                rx.recv_timeout(Duration::from_secs(2)).unwrap_or_default()
            }
        }
    };
    let stdout = collect(out);
    let stderr = collect(err);
    let code = match status {
        _ if timed_out => TIMED_OUT,
        Some(s) => match (s.code(), s.signal()) {
            (Some(c), _) => c,
            (None, Some(sig)) => 128 + sig,
            (None, None) => NOT_STARTED,
        },
        None => NOT_STARTED,
    };
    RunOut {
        code,
        stdout,
        stderr,
        elapsed,
    }
}

/// The whole of `pipe`, read on its own thread.
fn reader(mut pipe: impl Read + Send + 'static) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 8192];
        while let Ok(n) = pipe.read(&mut chunk) {
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
        let _ = tx.send(buf);
    });
    rx
}

/// `SIGKILL` to the process group `pid`, through the shell's `kill` (no `unsafe` code).
fn kill_group(pid: u32) {
    let _ = Command::new("/bin/sh")
        .args([
            "-c",
            "kill -9 \"-$1\" 2>/dev/null",
            "kill",
            &pid.to_string(),
        ])
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runner_lists_parse() {
        assert_eq!(
            Runner::parse_list("4,1, 2,2").expect("ok"),
            [Runner::Python, Runner::Hook, Runner::Missing]
        );
        assert!(
            Runner::parse_list("5")
                .expect_err("bad")
                .contains("unknown runner")
        );
        assert!(Runner::parse_list(",").is_err());
    }

    #[test]
    fn exec_captures_the_code_and_stderr_and_is_bounded() {
        let sh = Path::new("/bin/sh");
        let env = Env::default().with("PATH", "/usr/bin:/bin");
        let cwd = Path::new("/");
        let out = exec(
            sh,
            &["-c".as_ref(), "cat >&2; exit 3".as_ref()],
            b"in\n",
            &env,
            cwd,
        );
        assert_eq!((out.code, out.stderr.as_slice()), (3, &b"in\n"[..]));
        let out = exec(sh, &["-c".as_ref(), "echo hi".as_ref()], b"", &env, cwd);
        assert_eq!((out.code, out.stdout.as_slice()), (0, &b"hi\n"[..]));
        assert!(!out.allowed(), "an exit 0 with stdout is not a clean allow");
        let out = exec_bounded(
            sh,
            &["-c".as_ref(), "printf x >&2; sleep 30 & wait".as_ref()],
            b"",
            &env,
            cwd,
            Duration::from_millis(300),
        );
        assert_eq!(out.code, TIMED_OUT);
        assert_eq!(out.stderr, b"x");
        let out = exec(Path::new("/nonexistent/x"), &[], b"", &env, cwd);
        assert_eq!(out.code, NOT_STARTED);
        assert!(!out.blocked() && !out.allowed());
        let out = exec(sh, &["-c".as_ref(), "kill -9 $$".as_ref()], b"", &env, cwd);
        assert_eq!(out.code, 137);
    }
}
