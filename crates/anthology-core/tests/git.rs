//! `RealGit` (GC-2): the argv, the stripped environment, stdin, large output, exit codes,
//! a missing git and the timeout. Fake gits are shell scripts first on `PATH`; the real git
//! runs with its own `HOME` and `GIT_CONFIG_NOSYSTEM`, so no developer config applies.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anthology_core::Env;
use anthology_core::git::{GIT_TIMEOUT, Git, RealGit, STRIPPED_VARS, TimedGit};

/// A folder holding a fake `git` with `body` (after `#!/bin/sh`). The file is written by a
/// child `sh`, so this process never holds a writable descriptor to it while other tests fork
/// (which would make its exec fail with ETXTBSY).
fn fake_git(body: &str) -> (tempfile::TempDir, PathBuf) {
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
    let bin = dir.path().to_path_buf();
    (dir, bin)
}

fn env_with_path(bin: &Path) -> Env {
    Env::default().with_os("PATH", format!("{}:/usr/bin:/bin", bin.display()))
}

#[test]
fn the_argv_cwd_and_stdin_follow_memory_sync() {
    let (_d, bin) = fake_git(r#"printf '%s|' "$@"; echo; pwd -P; cat; echo "stdin done""#);
    let cwd = tempfile::tempdir().expect("cwd");
    let out = RealGit
        .run(
            &env_with_path(&bin),
            cwd.path(),
            &["status", "--porcelain", "a b"],
        )
        .expect("ran");
    let stdout = String::from_utf8(out.stdout).expect("utf-8");
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines[0], "-c|core.fsmonitor=false|status|--porcelain|a b|");
    let real = std::fs::canonicalize(cwd.path()).expect("canonical");
    assert_eq!(Path::new(lines[1]), real);
    assert_eq!(lines[2], "stdin done", "stdin is /dev/null (empty)");
    assert_eq!(out.code, 0);
    assert!(out.stderr.is_empty());
}

#[test]
fn the_child_gets_exactly_the_env_minus_the_git_location_vars() {
    let (_d, bin) = fake_git("env");
    let mut env = env_with_path(&bin)
        .with("KEEP_ME", "1")
        .with("GIT_AUTHOR_NAME", "kept")
        .with_os("RAW", std::ffi::OsStr::new("x"));
    for v in STRIPPED_VARS {
        env = env.with(v, "/bogus");
    }
    let out = RealGit.run(&env, Path::new("/"), &[]).expect("ran");
    let stdout = String::from_utf8(out.stdout).expect("utf-8");
    let names: Vec<&str> = stdout
        .lines()
        .filter_map(|l| l.split_once('=').map(|(k, _)| k))
        .collect();
    for v in STRIPPED_VARS {
        assert!(!names.contains(&v), "{v} was passed: {stdout}");
    }
    for v in ["PATH", "KEEP_ME", "GIT_AUTHOR_NAME", "RAW"] {
        assert!(names.contains(&v), "{v} is missing: {stdout}");
    }
    // Nothing from the real process environment leaks in (a shell may add PWD, SHLVL, _).
    for name in names {
        assert!(
            [
                "PATH",
                "KEEP_ME",
                "GIT_AUTHOR_NAME",
                "RAW",
                "PWD",
                "SHLVL",
                "_",
                "OLDPWD"
            ]
            .contains(&name),
            "{name} leaked into git's environment"
        );
    }
}

#[test]
fn large_output_on_both_pipes_does_not_deadlock() {
    let (_d, bin) = fake_git("head -c 3000000 /dev/zero; head -c 2000000 /dev/zero >&2; exit 3");
    let out = RealGit
        .run(&env_with_path(&bin), Path::new("/"), &[])
        .expect("ran");
    assert_eq!(out.stdout.len(), 3_000_000);
    assert_eq!(out.stderr.len(), 2_000_000);
    assert_eq!(out.code, 3);
}

#[test]
fn a_signal_is_a_negative_code_as_returncode() {
    let (_d, bin) = fake_git("kill -TERM $$");
    let out = RealGit
        .run(&env_with_path(&bin), Path::new("/"), &[])
        .expect("ran");
    assert_eq!(out.code, -15);
}

#[test]
fn a_missing_git_or_cwd_is_no_result() {
    let empty = tempfile::tempdir().expect("tempdir");
    let env = Env::default().with_os("PATH", empty.path());
    assert_eq!(RealGit.run(&env, Path::new("/"), &["--version"]), None);
    let (_d, bin) = fake_git("exit 0");
    let gone = empty.path().join("gone");
    assert_eq!(RealGit.run(&env_with_path(&bin), &gone, &[]), None);
}

#[test]
fn a_hung_git_is_killed_at_the_timeout() {
    assert_eq!(GIT_TIMEOUT, Duration::from_secs(10));
    assert_eq!(TimedGit::default().timeout, GIT_TIMEOUT);
    // `sleep` as a grandchild keeps the pipes open after the kill; `exec sleep` does not.
    for body in ["sleep 30", "exec sleep 30", "echo partial; sleep 30"] {
        let (_d, bin) = fake_git(body);
        let git = RealGit::with_timeout(Duration::from_millis(300));
        let start = Instant::now();
        assert_eq!(
            git.run(&env_with_path(&bin), Path::new("/"), &[]),
            None,
            "{body}"
        );
        let took = start.elapsed();
        assert!(took >= Duration::from_millis(300), "{body}: {took:?}");
        assert!(took < Duration::from_secs(5), "{body}: {took:?}");
    }
}

#[test]
fn a_child_that_exits_but_leaves_a_pipe_open_also_times_out() {
    // As communicate(timeout=...): git has exited, but a background child still holds stdout.
    let (_d, bin) = fake_git("(sleep 30) & echo started");
    let git = RealGit::with_timeout(Duration::from_millis(300));
    let start = Instant::now();
    assert_eq!(git.run(&env_with_path(&bin), Path::new("/"), &[]), None);
    assert!(start.elapsed() < Duration::from_secs(5));
}

/// The real git, if this machine has one, without any user or system config.
fn real_git_env(home: &Path) -> Option<Env> {
    let path = std::env::var_os("PATH")?;
    let env = Env::default()
        .with_os("PATH", path)
        .with_os("HOME", home)
        .with("GIT_CONFIG_NOSYSTEM", "1")
        .with("GIT_CONFIG_GLOBAL", "/dev/null")
        .with("LC_ALL", "C");
    let probe = RealGit.run(&env, home, &["--version"])?;
    (probe.code == 0).then_some(env)
}

#[test]
fn the_real_git_works_and_ignores_an_inherited_git_dir() {
    let home = tempfile::tempdir().expect("home");
    let Some(env) = real_git_env(home.path()) else {
        eprintln!("skipped: no git on PATH");
        return;
    };
    let repo = tempfile::tempdir().expect("repo");
    let init = RealGit
        .run(&env, repo.path(), &["init", "-q"])
        .expect("init");
    assert_eq!(init.code, 0, "{}", String::from_utf8_lossy(&init.stderr));
    // A GIT_DIR from a hook's environment would point git elsewhere; it is stripped.
    let env = env
        .with("GIT_DIR", "/nonexistent/.git")
        .with("GIT_WORK_TREE", "/nonexistent");
    let top = RealGit
        .run(&env, repo.path(), &["rev-parse", "--show-toplevel"])
        .expect("rev-parse");
    assert_eq!(top.code, 0, "{}", String::from_utf8_lossy(&top.stderr));
    let top = String::from_utf8(top.stdout).expect("utf-8");
    let real = std::fs::canonicalize(repo.path()).expect("canonical");
    assert_eq!(Path::new(top.trim_end()), real);
    let outside = tempfile::tempdir().expect("outside");
    let not_repo = RealGit
        .run(&env, outside.path(), &["rev-parse", "--show-toplevel"])
        .expect("ran");
    assert_ne!(not_repo.code, 0);
}
