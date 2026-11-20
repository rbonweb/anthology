//! `run_bin`, `run_sh` (F4, plan §3.7): run a program with exactly a given environment.
//!
//! Every runner clears the environment (`env_clear`) and passes exactly the [`Env`] it is given,
//! so the developer's or CI's variables never reach the program. stdin is written by its own
//! thread and both output pipes are read by their own threads, so no size of input or output
//! can deadlock.
//!
//! [`Output::code`] is the exit code; when a signal ends the program it is `128 + signal`, as a
//! POSIX shell reports it (`SIGKILL` is 137).
//!
//! Every program starts in its own process group. With a timeout ([`run_program_timeout`],
//! [`run_sh_in`]) the whole call is bounded: past the deadline the **group** is killed with
//! `SIGKILL` (code 137 when the program itself was still running), so grandchildren that hold
//! the pipes open (a downloader, a watchdog's `sleep`) die too, and the output read so far is
//! returned within a short grace period. Without a timeout the call waits for the pipes to close.
//!
//! The `anthology` binary is `ANTHOLOGY_BIN` when that is set, else the first `anthology` file
//! in a folder above the test binary. cargo puts test binaries below `target/<profile>/`
//! (`deps/<test>-<hash>`, or `build/<pkg>/<hash>/out/<test>-<hash>` with toolchain 1.100), and
//! the binary at `target/<profile>/anthology`. `cargo test --workspace` builds every binary of
//! the workspace before it runs any test, so the file is there and current. A crate tested on its
//! own (`cargo test -p <crate>`) must build it first (`cargo build -p anthology-cli`) or set
//! `ANTHOLOGY_BIN`; anthology-cli's own tests use `CARGO_BIN_EXE_anthology` instead.
#![forbid(unsafe_code)]

use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anthology_core::Env;

/// What a finished program did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    /// The exit code, or `128 + signal` (module docs).
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Output {
    /// stdout as text (lossy).
    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    /// stderr as text (lossy).
    pub fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}

/// Runs the built `anthology` binary with `args`, `stdin`, exactly `env`, in `cwd`.
pub fn run_bin(args: &[&str], stdin: &[u8], env: &Env, cwd: &Path) -> Output {
    run_program(&anthology_bin(), args, stdin, env, cwd)
}

/// The `anthology` binary (module docs); panics with what to do when it is missing.
pub fn anthology_bin() -> PathBuf {
    let exe = std::env::current_exe().expect("current_exe");
    let path = bin_path(std::env::var_os("ANTHOLOGY_BIN"), &exe);
    assert!(
        path.is_file(),
        "run_bin: no anthology binary at {}; build it (cargo build -p anthology-cli, or run \
         cargo test --workspace) or point ANTHOLOGY_BIN at it",
        path.display()
    );
    path
}

/// `ANTHOLOGY_BIN` when set and not empty, else the first `<folder>/anthology` file in a folder
/// above `exe` (module docs). When there is none, `<profile>/anthology` for the message, the
/// profile being the folder that holds `exe`'s `deps` or `build` folder.
pub fn bin_path(var: Option<OsString>, exe: &Path) -> PathBuf {
    if let Some(v) = var.filter(|v| !v.is_empty()) {
        return PathBuf::from(v);
    }
    if let Some(found) = exe
        .ancestors()
        .skip(1)
        .map(|dir| dir.join("anthology"))
        .find(|p| p.is_file())
    {
        return found;
    }
    exe.ancestors()
        .find(|d| {
            matches!(
                d.file_name().and_then(|n| n.to_str()),
                Some("deps" | "build")
            )
        })
        .and_then(Path::parent)
        .or_else(|| exe.parent().and_then(Path::parent))
        .unwrap_or(Path::new("."))
        .join("anthology")
}

/// Runs `program` with `args`, `stdin`, exactly `env`, in `cwd`, until it exits.
pub fn run_program(program: &Path, args: &[&str], stdin: &[u8], env: &Env, cwd: &Path) -> Output {
    run_program_timeout(program, args, stdin, env, cwd, None)
}

/// [`run_program`], bounded by `timeout` (module docs). A program that cannot start panics
/// with its path.
pub fn run_program_timeout(
    program: &Path,
    args: &[&str],
    stdin: &[u8],
    env: &Env,
    cwd: &Path,
    timeout: Option<Duration>,
) -> Output {
    use std::os::unix::process::CommandExt;
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
    let mut child = cmd
        .spawn()
        .unwrap_or_else(|e| panic!("cannot run {}: {e}", program.display()));
    let pid = child.id();
    let mut pipe_in = child.stdin.take().expect("stdin");
    let input = stdin.to_vec();
    // A program that exits without reading its input closes the pipe: that is not an error.
    thread::spawn(move || {
        let _ = pipe_in.write_all(&input);
    });
    let out = reader(child.stdout.take().expect("stdout"));
    let err = reader(child.stderr.take().expect("stderr"));
    let Some(t) = timeout else {
        let status = child.wait().expect("wait");
        return Output {
            code: code_of(status),
            stdout: out.recv().unwrap_or_default(),
            stderr: err.recv().unwrap_or_default(),
        };
    };
    let deadline = Instant::now() + t;
    let mut killed = false;
    let status = loop {
        if let Some(status) = child.try_wait().expect("wait") {
            break status;
        }
        if Instant::now() >= deadline {
            kill_group(pid);
            killed = true;
            break child.wait().expect("wait");
        }
        thread::sleep(Duration::from_millis(5));
    };
    // The program has exited; its pipes may still be held by what it started.
    let mut collect = |rx: &mpsc::Receiver<Vec<u8>>| -> Vec<u8> {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(b) => b,
            Err(_) => {
                if !killed {
                    kill_group(pid);
                    killed = true;
                }
                rx.recv_timeout(GRACE).unwrap_or_default()
            }
        }
    };
    let stdout = collect(&out);
    let stderr = collect(&err);
    Output {
        code: code_of(status),
        stdout,
        stderr,
    }
}

/// How long the pipes may stay open after the process group was killed.
const GRACE: Duration = Duration::from_secs(2);

/// `SIGKILL` to the process group `pid` (made by `process_group(0)`), through the shell's
/// `kill`, which takes a negative id: no `unsafe` code. No `--` before the id: dash's `kill`
/// rejects it ("Illegal number"), and dash is Debian's `/bin/sh`.
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

fn reader(mut pipe: impl Read + Send + 'static) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buf = Vec::new();
        // In chunks, so a killed group still returns what was written before.
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

/// The exit code, or `128 + signal`.
pub fn code_of(status: ExitStatus) -> i32 {
    match (status.code(), status.signal()) {
        (Some(c), _) => c,
        (None, Some(sig)) => 128 + sig,
        (None, None) => -1,
    }
}

/// A POSIX shell to run shims under (design §13.4).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Shell {
    /// `dash` (Debian's `/bin/sh`).
    Dash,
    /// `bash --posix`.
    BashPosix,
    /// `busybox sh` (Alpine's `/bin/sh`).
    Busybox,
    /// `/bin/sh`, whatever it is (bash on macOS, dash on Debian).
    Sh,
}

impl Shell {
    /// Every shell, in this order.
    pub const ALL: [Shell; 4] = [Shell::Dash, Shell::BashPosix, Shell::Busybox, Shell::Sh];

    /// A short name for test messages.
    pub fn name(self) -> &'static str {
        match self {
            Shell::Dash => "dash",
            Shell::BashPosix => "bash --posix",
            Shell::Busybox => "busybox sh",
            Shell::Sh => "/bin/sh",
        }
    }

    /// The program and its leading arguments, or `None` when the shell is not installed
    /// (looked up on this process's `PATH`, so a test's own `PATH` does not hide it).
    pub fn command(self) -> Option<(PathBuf, Vec<&'static str>)> {
        match self {
            Shell::Dash => which("dash").map(|p| (p, vec![])),
            Shell::BashPosix => which("bash").map(|p| (p, vec!["--posix"])),
            Shell::Busybox => which("busybox").map(|p| (p, vec!["sh"])),
            Shell::Sh => {
                let p = PathBuf::from("/bin/sh");
                p.is_file().then_some((p, vec![]))
            }
        }
    }

    /// Whether the shell is installed. macOS has no `dash` or `busybox`: skip those there.
    pub fn available(self) -> bool {
        self.command().is_some()
    }
}

/// `program` on this process's `PATH`, if any.
pub fn which(program: &str) -> Option<PathBuf> {
    which_in(program, std::env::var_os("PATH").as_deref())
}

/// `program` on `path` (a `PATH` value), if any.
pub fn which_in(program: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(path?)
        .map(|dir| dir.join(program))
        .find(|p| {
            std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

/// Runs `script` with `args` under `shell`, with `stdin` and exactly `env`, in the script's
/// folder. Panics when the shell is not installed: check [`Shell::available`] first.
pub fn run_sh(shell: Shell, script: &Path, args: &[&str], stdin: &[u8], env: &Env) -> Output {
    let cwd = script.parent().unwrap_or(Path::new("."));
    run_sh_in(shell, script, args, stdin, env, cwd, None)
}

/// [`run_sh`] in `cwd`, with an optional timeout (module docs).
pub fn run_sh_in(
    shell: Shell,
    script: &Path,
    args: &[&str],
    stdin: &[u8],
    env: &Env,
    cwd: &Path,
    timeout: Option<Duration>,
) -> Output {
    let (program, lead) = shell
        .command()
        .unwrap_or_else(|| panic!("{} is not installed", shell.name()));
    let script = script.to_string_lossy();
    let mut argv: Vec<&str> = lead;
    argv.push(&script);
    argv.extend_from_slice(args);
    run_program_timeout(&program, &argv, stdin, env, cwd, timeout)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> Env {
        Env::default()
            .with("PATH", "/usr/bin:/bin")
            .with("ONLY", "this")
    }

    #[test]
    fn the_program_gets_exactly_the_env_stdin_args_and_cwd() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cwd = std::fs::canonicalize(dir.path()).expect("realpath");
        let out = run_program(
            Path::new("/bin/sh"),
            &[
                "-c",
                "env | sort; pwd; cat; echo \"$0|$1\" >&2; exit 3",
                "zero",
                "one",
            ],
            b"input\n",
            &env(),
            &cwd,
        );
        assert_eq!(out.code, 3);
        let text = out.stdout_text();
        assert!(text.contains("ONLY=this\n"), "{text}");
        assert!(!text.contains("HOME="), "{text}");
        assert!(text.contains(&format!("{}\n", cwd.display())), "{text}");
        assert!(text.ends_with("input\n"), "{text}");
        assert_eq!(out.stderr_text(), "zero|one\n");
    }

    #[test]
    fn a_signal_is_128_plus_the_signal() {
        let out = run_program(
            Path::new("/bin/sh"),
            &["-c", "kill -TERM $$"],
            b"",
            &env(),
            Path::new("/"),
        );
        assert_eq!(out.code, 128 + 15);
    }

    #[test]
    fn large_input_and_output_do_not_deadlock() {
        let big = vec![b'x'; 4 << 20];
        let out = run_program(
            Path::new("/bin/sh"),
            &["-c", "cat; cat /dev/null >&2"],
            &big,
            &env(),
            Path::new("/"),
        );
        assert_eq!(out.code, 0);
        assert_eq!(out.stdout.len(), big.len());
    }

    #[test]
    fn a_program_that_ignores_stdin_is_fine() {
        let big = vec![b'y'; 4 << 20];
        let out = run_program(
            Path::new("/bin/sh"),
            &["-c", "exit 0"],
            &big,
            &env(),
            Path::new("/"),
        );
        assert_eq!(out.code, 0);
    }

    #[test]
    fn the_binary_is_the_override_or_in_a_folder_above_the_test_binary() {
        let dir = tempfile::tempdir().expect("tempdir");
        let profile = dir.path().join("target/debug");
        std::fs::create_dir_all(profile.join("deps")).expect("mkdir");
        std::fs::create_dir_all(profile.join("build/p/h/out")).expect("mkdir");
        // Not found: the old layout's guess, for the message.
        let deps = profile.join("deps/x-123");
        assert_eq!(bin_path(None, &deps), profile.join("anthology"));
        std::fs::write(profile.join("anthology"), "").expect("write");
        for exe in [deps, profile.join("build/p/h/out/x-123")] {
            assert_eq!(
                bin_path(None, &exe),
                profile.join("anthology"),
                "{}",
                exe.display()
            );
            assert_eq!(
                bin_path(Some(OsString::new()), &exe),
                profile.join("anthology")
            );
            assert_eq!(
                bin_path(Some("/opt/anthology".into()), &exe),
                PathBuf::from("/opt/anthology")
            );
        }
        // A folder named anthology is not the binary.
        std::fs::remove_file(profile.join("anthology")).expect("rm");
        std::fs::create_dir(profile.join("build/p/anthology")).expect("mkdir");
        assert_eq!(
            bin_path(None, &profile.join("build/p/h/out/x")),
            profile.join("anthology")
        );
    }

    #[test]
    fn run_bin_runs_the_built_binary() {
        // `cargo test --workspace` builds the anthology binary first (module docs); a crate
        // tested alone needs `cargo build -p anthology-cli` or ANTHOLOGY_BIN.
        let out = run_bin(&["--version"], b"", &env(), Path::new("/"));
        assert_eq!(out.code, 0, "{}", out.stderr_text());
        assert!(
            out.stdout_text().starts_with("anthology 0.1.0 ("),
            "{}",
            out.stdout_text()
        );
    }

    #[test]
    fn a_missing_binary_panics_with_what_to_do() {
        let err = std::panic::catch_unwind(|| {
            let exe = Path::new("/nonexistent/target/debug/deps/x");
            let path = bin_path(None, exe);
            assert!(
                path.is_file(),
                "run_bin: no anthology binary at {}",
                path.display()
            );
        })
        .expect_err("panics");
        let msg = err.downcast_ref::<String>().expect("message");
        assert!(msg.contains("/nonexistent/target/debug/anthology"), "{msg}");
    }

    #[test]
    fn a_timeout_kills_grandchildren_that_hold_the_pipes() {
        for script in [
            "echo started; sleep 30; :",
            "echo started; (sleep 30; :) & wait",
            "echo started; sleep 30 & exit 0",
        ] {
            let start = Instant::now();
            let out = run_program_timeout(
                Path::new("/bin/sh"),
                &["-c", script],
                b"",
                &env(),
                Path::new("/"),
                Some(Duration::from_millis(300)),
            );
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "{script}: {:?}",
                start.elapsed()
            );
            assert_eq!(out.stdout_text(), "started\n", "{script}");
            if !script.contains("exit 0") {
                assert_eq!(out.code, 137, "{script}");
            } else {
                assert_eq!(out.code, 0, "{script}");
            }
        }
    }

    #[test]
    fn which_finds_executables_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        let exe = dir.path().join("tool");
        std::fs::write(&exe, "#!/bin/sh\n").expect("write");
        let plain = dir.path().join("plain");
        std::fs::write(&plain, "x").expect("write");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let path = std::env::join_paths([Path::new("/nonexistent"), dir.path()]).expect("join");
        assert_eq!(which_in("tool", Some(&path)), Some(exe));
        assert_eq!(which_in("plain", Some(&path)), None);
        assert_eq!(which_in("tool", None), None);
    }

    #[test]
    fn every_available_shell_runs_a_script() {
        let dir = tempfile::tempdir().expect("tempdir");
        let script = dir.path().join("s.sh");
        std::fs::write(
            &script,
            "read x; printf '%s|%s|%s\\n' \"$x\" \"$1\" \"$ONLY\"\n",
        )
        .expect("write");
        let mut ran = 0;
        for shell in Shell::ALL {
            if !shell.available() {
                eprintln!("skipped: {} is not installed", shell.name());
                continue;
            }
            let out = run_sh(shell, &script, &["arg"], b"in\n", &env());
            assert_eq!(out.code, 0, "{}: {}", shell.name(), out.stderr_text());
            assert_eq!(out.stdout_text(), "in|arg|this\n", "{}", shell.name());
            ran += 1;
        }
        assert!(Shell::Sh.available());
        assert!(ran >= 1);
    }

    #[test]
    fn bash_posix_is_really_posix_mode() {
        if !Shell::BashPosix.available() {
            eprintln!("skipped: bash is not installed");
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let script = dir.path().join("p.sh");
        std::fs::write(
            &script,
            "case :$SHELLOPTS: in *:posix:*) echo posix;; esac\n",
        )
        .expect("write");
        let out = run_sh(Shell::BashPosix, &script, &[], b"", &env());
        assert_eq!(out.stdout_text(), "posix\n");
    }
}
