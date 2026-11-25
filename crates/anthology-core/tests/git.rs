//! `RealGit` (GC-2): the argv, the stripped environment, stdin, large output, exit codes,
//! a missing git and the timeout. Fake gits are shell scripts first on `PATH`; the real git
//! runs with its own `HOME` and `GIT_CONFIG_NOSYSTEM`, so no developer config applies.
#![forbid(unsafe_code)]

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
fn a_git_in_the_cwd_is_never_run_through_a_relative_or_empty_path_entry() {
    // The cwd is a repository that holds a `git` of its own. `.`, an empty entry and `./`
    // resolve to it from there; the absolute entry after them holds the git that must run.
    let marker_dir = tempfile::tempdir().expect("marks");
    let marker = marker_dir.path().join("ran");
    let (planted, _) = fake_git(&format!(": > '{}'; echo planted", marker.display()));
    let (_good, good_bin) = fake_git("echo good");
    // `../` and `..` find the planted git from a folder below the one that holds it.
    let below = planted.path().join("sub");
    std::fs::create_dir(&below).expect("mkdir");
    for lead in [".", "", "./", ".:.", "../", ".."] {
        let cwd = if lead.starts_with("..") {
            &below
        } else {
            planted.path()
        };
        let path = format!("{lead}:{}:/usr/bin:/bin", good_bin.display());
        let env = Env::default().with_os("PATH", path);
        let out = RealGit.run(&env, cwd, &["x"]).expect("ran");
        assert_eq!(out.stdout, b"good\n", "lead {lead:?}");
        assert!(!marker.exists(), "the planted git ran (lead {lead:?})");
    }
}

#[test]
fn the_child_gets_a_path_of_the_absolute_entries_only() {
    let (_d, bin) = fake_git(r#"printf '%s\n' "$PATH""#);
    let path = format!(".::{}:bin:/usr/bin:/bin:", bin.display());
    let env = Env::default().with_os("PATH", path);
    let out = RealGit.run(&env, Path::new("/"), &[]).expect("ran");
    let want = format!("{}:/usr/bin:/bin\n", bin.display());
    assert_eq!(String::from_utf8_lossy(&out.stdout), want);
}

#[test]
fn a_link_to_a_git_is_found_and_a_dangling_link_or_one_to_a_folder_is_not() {
    use std::os::unix::fs::symlink;
    let (_real, real_bin) = fake_git("echo linked");
    let dirs = tempfile::tempdir().expect("dirs");
    let link = dirs.path().join("link");
    std::fs::create_dir(&link).expect("mkdir");
    symlink(real_bin.join("git"), link.join("git")).expect("symlink");
    let dangling = dirs.path().join("dangling");
    std::fs::create_dir(&dangling).expect("mkdir");
    symlink(dirs.path().join("nowhere"), dangling.join("git")).expect("symlink");
    let to_folder = dirs.path().join("to_folder");
    std::fs::create_dir(&to_folder).expect("mkdir");
    symlink(dirs.path(), to_folder.join("git")).expect("symlink");
    let skipped = [&dangling, &to_folder];
    let path = std::env::join_paths(skipped.iter().map(|p| p.as_path()).chain([link.as_path()]))
        .expect("join");
    let env = Env::default().with_os("PATH", &path);
    assert_eq!(
        anthology_core::git::resolve_git(&env),
        Some(link.join("git"))
    );
    let out = RealGit.run(&env, Path::new("/"), &[]).expect("ran");
    assert_eq!(out.stdout, b"linked\n");
    // Only the skipped ones: no git.
    let none = std::env::join_paths(skipped.iter().map(|p| p.as_path())).expect("join");
    let env = Env::default().with_os("PATH", &none);
    assert_eq!(anthology_core::git::resolve_git(&env), None);
}

#[test]
fn only_relative_or_empty_path_entries_find_no_git() {
    let marker_dir = tempfile::tempdir().expect("marks");
    let marker = marker_dir.path().join("ran");
    let (planted, _) = fake_git(&format!(": > '{}'", marker.display()));
    for path in [".", ":", "", ".::", "bin", "./git", "..:."] {
        let env = Env::default().with_os("PATH", path);
        assert_eq!(RealGit.run(&env, planted.path(), &["x"]), None, "{path:?}");
        assert!(!marker.exists(), "the planted git ran (PATH {path:?})");
    }
    // No PATH at all is a missing git, not a search of the cwd.
    assert_eq!(RealGit.run(&Env::default(), planted.path(), &[]), None);
    assert!(!marker.exists());
}

#[test]
fn the_search_takes_the_first_executable_file_on_an_absolute_entry() {
    use std::os::unix::fs::PermissionsExt;
    let (_a, a_bin) = fake_git("exit 0");
    let (_b, b_bin) = fake_git("exit 0");
    let dirs = tempfile::tempdir().expect("dirs");
    // `plain` holds a `git` that is not executable; `folder` holds a folder called `git`.
    let plain = dirs.path().join("plain");
    std::fs::create_dir(&plain).expect("mkdir");
    std::fs::write(plain.join("git"), "#!/bin/sh\n").expect("write");
    std::fs::set_permissions(plain.join("git"), std::fs::Permissions::from_mode(0o644))
        .expect("chmod");
    let folder = dirs.path().join("folder");
    std::fs::create_dir_all(folder.join("git")).expect("mkdir");
    let path = std::env::join_paths([
        Path::new("."),
        Path::new(""),
        &plain,
        &folder,
        &a_bin,
        &b_bin,
    ])
    .expect("join");
    let found = anthology_core::git::find_on_absolute_path("git", Some(&path));
    assert_eq!(found, Some(a_bin.join("git")));
    assert_eq!(
        anthology_core::git::find_on_absolute_path("git", None),
        None
    );
    let env = Env::default().with_os("PATH", &path);
    assert_eq!(
        anthology_core::git::resolve_git(&env),
        Some(a_bin.join("git"))
    );
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
fn concurrent_hung_gits_each_time_out_on_schedule() {
    // The macOS flake: a spawn on one thread blocked for as long as a long-lived grandchild of a
    // git spawned on another thread (std's pipes get FD_CLOEXEC in a second step there, so a
    // concurrent fork can inherit them). Every call must end near its timeout, however many
    // hung gits and grandchildren are about. This is a smoke test: it passes without the spawn
    // lock too, and the unit tests in `src/git.rs` are what pin the lock and the hand-over.
    let (_d, bin) = fake_git("(sleep 3) & sleep 3");
    let env = env_with_path(&bin);
    let workers: Vec<_> = (0..6)
        .map(|_| {
            let env = env.clone();
            std::thread::spawn(move || {
                let git = RealGit::with_timeout(Duration::from_millis(100));
                (0..8)
                    .map(|_| {
                        let start = Instant::now();
                        let out = git.run(&env, Path::new("/"), &[]);
                        (out, start.elapsed())
                    })
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    for worker in workers {
        for (out, took) in worker.join().expect("worker") {
            assert_eq!(out, None);
            assert!(took < Duration::from_millis(2500), "{took:?}");
        }
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

#[test]
fn after_a_timeout_a_streaming_grandchild_loses_its_pipe() {
    // Python's Popen closes the pipes after the kill, so a grandchild that keeps writing gets
    // EPIPE (SIGPIPE is ignored here so it can leave a mark).
    let (_d, bin) = fake_git(
        "(trap '' PIPE; while echo x; do sleep 0.05; done; touch \"$MARK\") & exec sleep 30",
    );
    let marks = tempfile::tempdir().expect("marks");
    let mark = marks.path().join("epipe");
    let env = env_with_path(&bin).with_os("MARK", &mark);
    let git = RealGit::with_timeout(Duration::from_millis(300));
    assert_eq!(git.run(&env, Path::new("/"), &[]), None);
    let start = Instant::now();
    while !mark.exists() && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(mark.exists(), "the grandchild never saw its pipe close");
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
