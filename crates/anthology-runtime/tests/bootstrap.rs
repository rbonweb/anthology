//! `bootstrap.rs` (RT2): the live-guard bootstrap caller and doctor's `anthology_binary` check.
//!
//! The bootstrap is a fake `hooks/bootstrap.sh` per test (BS1 owns the real one), so these
//! tests pin the caller's contract: one run, the exact argv and environment, the `--quiet`
//! status line, the budget plus grace as a hard deadline, and no process left behind.
#![forbid(unsafe_code)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anthology_core::Env;
use anthology_core::lock::{self, Lock};
use anthology_core::platform::Target;
use anthology_runtime::bootstrap::{
    self, CHECK_BAD_LOCK, CHECK_NOT_FOUND, CHECK_OK, FAILED_COULD_NOT_RUN, FAILED_TIMED_OUT,
    GuardBinaryStatus, SKIPPED_NO_BOOTSTRAP,
};
use anthology_testkit::release::sha256_hex;

struct Guard {
    _dir: tempfile::TempDir,
    root: PathBuf,
    tmp: PathBuf,
    env: Env,
}

fn guard(script: Option<&str>) -> Guard {
    let dir = tempfile::tempdir().expect("tmp");
    let tmp = fs::canonicalize(dir.path()).expect("realpath");
    let root = tmp.join("anthology-guardrails");
    fs::create_dir_all(root.join("hooks")).expect("hooks");
    if let Some(body) = script {
        let path = root.join("hooks").join("bootstrap.sh");
        fs::write(&path, body).expect("script");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("chmod");
    }
    let env = Env::default()
        .with("PATH", "/usr/bin:/bin")
        .with_os("HOME", tmp.join("home"))
        .with_os("CLAUDE_CONFIG_DIR", tmp.join("config"))
        .with_os("T", &tmp);
    Guard {
        _dir: dir,
        root,
        tmp,
        env,
    }
}

/// Whether process `pid` still runs (a zombie counts as gone).
fn alive(pid: &str) -> bool {
    let out = Command::new("ps")
        .args(["-o", "stat=", "-p", pid.trim()])
        .output()
        .expect("ps");
    let stat = String::from_utf8_lossy(&out.stdout);
    let stat = stat.trim();
    !stat.is_empty() && !stat.starts_with('Z')
}

fn wait_gone(pid: &str) -> bool {
    let until = Instant::now() + Duration::from_secs(3);
    while Instant::now() < until {
        if !alive(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

mod ensure_live_guard_binary {
    use super::*;

    #[test]
    fn present_passes_the_exact_argv_and_only_the_env() {
        let g = guard(Some(
            "printf '%s\\n' \"$@\" > \"$T/argv\"\nenv > \"$T/env\"\npwd > \"$T/cwd\"\n\
             echo status=present\n",
        ));
        let env = g.env.clone().with("ANTHOLOGY_TEST_MARK", "1");
        let status = bootstrap::ensure_live_guard_binary(&g.root, 20, &env);
        assert_eq!(status, GuardBinaryStatus::Present);
        let argv = fs::read_to_string(g.tmp.join("argv")).expect("argv");
        assert_eq!(
            argv,
            "session\n--plugin\nguardrails\n--quiet\n--budget\n20\n"
        );
        let seen = fs::read_to_string(g.tmp.join("env")).expect("env");
        assert!(seen.contains("ANTHOLOGY_TEST_MARK=1"));
        let names: Vec<&str> = seen
            .lines()
            .filter_map(|l| l.split_once('=').map(|(k, _)| k))
            .filter(|k| !matches!(*k, "PWD" | "SHLVL" | "_" | "OLDPWD"))
            .collect();
        let mut names = names;
        names.sort_unstable();
        assert_eq!(
            names,
            ["ANTHOLOGY_TEST_MARK", "CLAUDE_CONFIG_DIR", "HOME", "PATH", "T"]
        );
    }

    #[test]
    fn installed_and_failed_lines_are_read() {
        let g = guard(Some("echo status=installed\n"));
        assert_eq!(
            bootstrap::ensure_live_guard_binary(&g.root, 20, &g.env),
            GuardBinaryStatus::Installed
        );
        let g = guard(Some("echo status=failed:download failed; exit 1\n"));
        assert_eq!(
            bootstrap::ensure_live_guard_binary(&g.root, 20, &g.env),
            GuardBinaryStatus::Failed("download failed".into())
        );
        let g = guard(Some(
            "echo status=failed:sha256 does not match anthology.lock; exit 1\n",
        ));
        assert_eq!(
            bootstrap::ensure_live_guard_binary(&g.root, 20, &g.env).status_text(),
            "failed: sha256 does not match anthology.lock"
        );
    }

    #[test]
    fn odd_output_or_exit_codes_are_failures() {
        for script in [
            "echo hello\n",
            "exit 0\n",
            "echo status=present; exit 1\n",
            "echo status=installed; kill -9 $$\n",
            "echo status=bogus\n",
        ] {
            let g = guard(Some(script));
            assert_eq!(
                bootstrap::ensure_live_guard_binary(&g.root, 20, &g.env),
                GuardBinaryStatus::Failed(FAILED_COULD_NOT_RUN.into()),
                "{script}"
            );
        }
    }

    #[test]
    fn a_guard_without_a_bootstrap_is_skipped() {
        let g = guard(None);
        assert_eq!(
            bootstrap::ensure_live_guard_binary(&g.root, 20, &g.env),
            GuardBinaryStatus::Skipped(SKIPPED_NO_BOOTSTRAP)
        );
    }

    #[test]
    fn stdin_is_empty() {
        let g = guard(Some("cat > \"$T/stdin\"; echo status=present\n"));
        assert_eq!(
            bootstrap::ensure_live_guard_binary(&g.root, 20, &g.env),
            GuardBinaryStatus::Present
        );
        assert_eq!(fs::read(g.tmp.join("stdin")).expect("stdin"), b"");
    }

    #[test]
    fn a_hung_bootstrap_is_killed_at_the_deadline_with_its_children() {
        let g = guard(Some("sleep 1000 &\necho $! > \"$T/pid\"\nwait\n"));
        let start = Instant::now();
        let status = bootstrap::ensure_with_grace(&g.root, 1, &g.env, Duration::from_millis(500));
        let took = start.elapsed();
        assert_eq!(status, GuardBinaryStatus::Failed(FAILED_TIMED_OUT.into()));
        assert!(took >= Duration::from_millis(1400), "{took:?}");
        assert!(took < Duration::from_secs(4), "{took:?}");
        let pid = fs::read_to_string(g.tmp.join("pid")).expect("pid");
        assert!(wait_gone(&pid), "the bootstrap's sleep {pid} outlived the deadline");
    }

    #[test]
    fn the_default_grace_bounds_the_wait() {
        // Budget 1 s + GRACE_SECS: a bootstrap that never ends returns in about 6 s.
        let g = guard(Some("while :; do sleep 1; done\n"));
        let start = Instant::now();
        let status = bootstrap::ensure_live_guard_binary(&g.root, 1, &g.env);
        let took = start.elapsed();
        assert_eq!(status, GuardBinaryStatus::Failed(FAILED_TIMED_OUT.into()));
        let bound = Duration::from_secs(1 + bootstrap::GRACE_SECS);
        assert!(took >= bound, "{took:?}");
        assert!(took < bound + Duration::from_secs(2), "{took:?}");
    }

    #[test]
    fn a_child_holding_stdout_does_not_hold_the_caller() {
        let g = guard(Some(
            "sleep 1000 &\necho $! > \"$T/pid\"\necho status=present\nexit 0\n",
        ));
        let start = Instant::now();
        let status = bootstrap::ensure_live_guard_binary(&g.root, 20, &g.env);
        assert_eq!(status, GuardBinaryStatus::Present);
        assert!(start.elapsed() < Duration::from_secs(3), "{:?}", start.elapsed());
        let pid = fs::read_to_string(g.tmp.join("pid")).expect("pid");
        assert!(wait_gone(&pid), "the straggler {pid} was left running");
    }

    #[test]
    fn a_fast_bootstrap_returns_at_once() {
        let g = guard(Some("echo status=present\n"));
        let start = Instant::now();
        bootstrap::ensure_live_guard_binary(&g.root, 20, &g.env);
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}

mod anthology_binary_check {
    use super::*;

    const VERSION: &str = "0.1.0";
    const LINUX: (&str, &str) = ("Linux", "x86_64");
    const BINARY: &[u8] = b"\x7fELF fake anthology binary";

    fn the_lock(tarball_sha: &str) -> Lock {
        let mut sha256 = std::collections::BTreeMap::new();
        for t in Target::ALL {
            let d = if t == Target::LinuxX86_64Musl {
                tarball_sha.to_string()
            } else {
                sha256_hex(t.triple().as_bytes())
            };
            sha256.insert(t, d);
        }
        Lock {
            version: VERSION.into(),
            tag: format!("anthology-bin--v{VERSION}"),
            repo: "example/anthology".into(),
            sha256,
        }
    }

    const TARBALL_SHA: &str = "ab";

    fn tarball_sha() -> String {
        sha256_hex(TARBALL_SHA.as_bytes())
    }

    /// A guard root with a valid lock and a current install; returns the version folder.
    fn installed(g: &Guard) -> PathBuf {
        let lock = the_lock(&tarball_sha());
        fs::write(g.root.join("anthology.lock"), lock::render_anthology(&lock)).expect("lock");
        let vdir = g.tmp.join("config").join("anthology").join("bin").join(VERSION);
        fs::create_dir_all(&vdir).expect("vdir");
        let bin = vdir.join("anthology");
        fs::write(&bin, BINARY).expect("bin");
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).expect("chmod");
        fs::write(vdir.join(".bin-sha256"), format!("{}\n", sha256_hex(BINARY))).expect("bin sha");
        fs::write(vdir.join(".sha256"), format!("{}\n", tarball_sha())).expect("sha");
        vdir
    }

    fn check(g: &Guard) -> (Option<bool>, String) {
        bootstrap::anthology_binary_check_with(Some(&g.root), &g.env, LINUX)
    }

    fn not_installed() -> (Option<bool>, String) {
        (
            Some(false),
            format!(
                "the guardrails' anthology binary ({VERSION}) is not installed; the guard blocks \
                 shell commands, file edits and MCP tools until it is; run /anthology:setup"
            ),
        )
    }

    fn mismatch() -> (Option<bool>, String) {
        (
            Some(false),
            format!(
                "the guardrails' anthology binary ({VERSION}) does not match its install marker; \
                 start a new session to reinstall it"
            ),
        )
    }

    #[test]
    fn no_guardrails_is_nothing_to_check() {
        let g = guard(None);
        assert_eq!(
            bootstrap::anthology_binary_check(None, &g.env),
            (None, CHECK_NOT_FOUND.to_string())
        );
    }

    #[test]
    fn a_current_install_matches() {
        let g = guard(None);
        installed(&g);
        assert_eq!(check(&g), (Some(true), CHECK_OK.to_string()));
    }

    #[test]
    fn a_missing_or_invalid_lock_is_reported() {
        let g = guard(None);
        installed(&g);
        let lock_path = g.root.join("anthology.lock");
        let text = fs::read_to_string(&lock_path).expect("lock");
        // Valid JSON, but not the design §8.2 layout.
        let compact = text.replace("\n", "").replace("  ", "");
        fs::write(&lock_path, compact).expect("compact");
        assert_eq!(check(&g), (Some(false), CHECK_BAD_LOCK.to_string()));
        fs::write(&lock_path, text.replace("anthology-bin--v", "rtk--v")).expect("rtk tag");
        assert_eq!(check(&g), (Some(false), CHECK_BAD_LOCK.to_string()));
        fs::remove_file(&lock_path).expect("unlink");
        assert_eq!(check(&g), (Some(false), CHECK_BAD_LOCK.to_string()));
    }

    #[test]
    fn an_unsupported_platform_is_not_applicable() {
        let g = guard(None);
        installed(&g);
        let got = bootstrap::anthology_binary_check_with(Some(&g.root), &g.env, ("FreeBSD", "amd64"));
        assert_eq!(
            got,
            (
                None,
                "there is no guard binary for this platform (FreeBSD/amd64)".to_string()
            )
        );
    }

    #[test]
    fn each_installed_rule_is_checked() {
        type Break = fn(&Path);
        let cases: [(&str, Break); 6] = [
            ("no folder", |v| fs::remove_dir_all(v).expect("rm")),
            ("empty binary", |v| fs::write(v.join("anthology"), b"").expect("empty")),
            ("not executable", |v| {
                fs::set_permissions(v.join("anthology"), fs::Permissions::from_mode(0o644))
                    .expect("chmod")
            }),
            ("no marker", |v| fs::remove_file(v.join(".sha256")).expect("rm")),
            ("empty marker", |v| fs::write(v.join(".sha256"), b"").expect("empty")),
            ("binary is a symlink", |v| {
                let real = v.with_file_name("real-anthology");
                fs::rename(v.join("anthology"), &real).expect("mv");
                std::os::unix::fs::symlink(&real, v.join("anthology")).expect("ln");
            }),
        ];
        for (name, breaker) in cases {
            let g = guard(None);
            let vdir = installed(&g);
            breaker(&vdir);
            assert_eq!(check(&g), not_installed(), "{name}");
        }
        // The version folder is a symlink to a good install.
        let g = guard(None);
        let vdir = installed(&g);
        let moved = vdir.with_file_name("moved");
        fs::rename(&vdir, &moved).expect("mv");
        std::os::unix::fs::symlink(&moved, &vdir).expect("ln");
        assert_eq!(check(&g), not_installed(), "symlinked version folder");
    }

    #[test]
    fn a_marker_that_disagrees_is_a_mismatch() {
        let g = guard(None);
        let vdir = installed(&g);
        fs::write(vdir.join(".sha256"), format!("{}\n", "0".repeat(64))).expect("marker");
        assert_eq!(check(&g), mismatch());

        let g = guard(None);
        let vdir = installed(&g);
        fs::write(vdir.join("anthology"), b"\x7fELF swapped").expect("swap");
        assert_eq!(check(&g), mismatch(), "the binary no longer matches .bin-sha256");

        let g = guard(None);
        let vdir = installed(&g);
        fs::remove_file(vdir.join(".bin-sha256")).expect("rm");
        assert_eq!(check(&g), mismatch(), "no .bin-sha256");
    }
}
