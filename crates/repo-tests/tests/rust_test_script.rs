//! `scripts/rust-test.sh test`: which cargo scope the container runs. A stub `docker` on PATH
//! records its argv, so the script runs without a container.

use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

use repo_tests::repo;
use tempfile::TempDir;

const SCRIPT: &str = "scripts/rust-test.sh";
const SCOPE: &str = "RUST_TEST_SCOPE=";

/// What the stub `docker` was called with.
struct Call {
    args: Vec<String>,
}

impl Call {
    /// The scope the script hands to the container shell: `--workspace`, or empty.
    fn scope(&self) -> &str {
        let entry = self.args.iter().find_map(|a| a.strip_prefix(SCOPE));
        entry.unwrap_or_else(|| panic!("no {SCOPE} argument in {:?}", self.args))
    }

    /// The container shell script (the argument after `sh -c`).
    fn inner(&self) -> &str {
        let at = self.args.iter().position(|a| a == "-c").unwrap();
        &self.args[at + 1]
    }

    /// The mode and cargo arguments after the script name `rust-test`.
    fn passed(&self) -> Vec<&str> {
        let at = self.args.iter().position(|a| a == "rust-test").unwrap();
        self.args[at + 1..].iter().map(String::as_str).collect()
    }
}

fn run(args: &[&str]) -> Call {
    let dir: TempDir = tempfile::tempdir().unwrap();
    let stub = dir.path().join("docker");
    fs::write(
        &stub,
        "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\0' \"$a\"; done > \"$DOCKER_ARGV\"\n",
    )
    .unwrap();
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
    let argv = dir.path().join("argv");

    let mut path = vec![dir.path().to_path_buf()];
    path.extend(env::split_paths(&env::var_os("PATH").unwrap()));
    let out = Command::new(repo::path(SCRIPT))
        .args(args)
        .env("PATH", env::join_paths(path).unwrap())
        .env("DOCKER_ARGV", &argv)
        .output()
        .expect("script ran");
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let raw = fs::read_to_string(&argv).expect("docker stub was called");
    let args = raw
        .split_terminator('\0')
        .map(str::to_owned)
        .collect::<Vec<_>>();
    Call { args }
}

#[test]
fn a_named_package_drops_workspace() {
    let call = run(&["test", "-p", "foo"]);
    assert_eq!(call.scope(), "");
    assert_eq!(call.passed(), ["test", "-p", "foo"]);
}

#[test]
fn no_package_keeps_workspace() {
    let call = run(&["test"]);
    assert_eq!(call.scope(), "--workspace");
    assert_eq!(call.passed(), ["test"]);
}

#[test]
fn package_equals_drops_workspace() {
    let call = run(&["test", "--package=foo"]);
    assert_eq!(call.scope(), "");
    assert_eq!(call.passed(), ["test", "--package=foo"]);
}

#[test]
fn every_package_spelling_drops_workspace() {
    for spelling in [
        &["-p", "foo"][..],
        &["--package", "foo"],
        &["-pfoo"],
        &["--package=foo"],
        &["--locked", "-p", "foo", "--test", "bar"],
    ] {
        let mut args = vec!["test"];
        args.extend_from_slice(spelling);
        assert_eq!(run(&args).scope(), "", "{spelling:?}");
    }
}

#[test]
fn a_flag_after_the_separator_is_not_a_package() {
    let call = run(&["test", "--", "-p"]);
    assert_eq!(call.scope(), "--workspace");
}

#[test]
fn a_filter_or_other_flag_keeps_workspace() {
    for args in [&["test", "--profile", "dev"][..], &["test", "plain_name"]] {
        assert_eq!(run(args).scope(), "--workspace", "{args:?}");
    }
}

#[test]
fn the_plain_and_all_features_runs_both_take_the_scope() {
    let call = run(&["test", "-p", "foo"]);
    let inner = call.inner();
    // The quotes and `$@` are escaped in the script text: the container shell expands them later.
    let plain = r#"cargo test $RUST_TEST_SCOPE --locked \"\$@\""#;
    let all = r#"cargo test $RUST_TEST_SCOPE --locked --all-features \"\$@\""#;
    assert!(inner.contains(&format!("{plain} && {all}")), "{inner}");
}

#[test]
fn lint_and_all_ignore_the_package_scope() {
    // `all` takes no cargo arguments and `lint` always checks the whole workspace.
    for mode in ["lint", "all"] {
        let call = run(&[mode]);
        assert_eq!(call.scope(), "--workspace", "{mode}");
        assert!(call.inner().contains("cargo clippy --workspace"), "{mode}");
    }
}

// ---- sessions: session-start, session-stop and ANTHOLOGY_RUST_SESSION ---------------------

/// The result of one script run against a stub `docker` that logs every call.
struct Session {
    status: i32,
    stderr: String,
    /// Each docker call's arguments, in order.
    calls: Vec<Vec<String>>,
}

impl Session {
    /// The first argument (the docker subcommand) of every call.
    fn verbs(&self) -> Vec<&str> {
        self.calls.iter().map(|c| c[0].as_str()).collect()
    }
}

const NAME: &str = "anthology-release-123-456";

/// Runs the script with `args` against a stub `docker` that records each call and fails
/// `docker exec` when `fail_exec` is set. `env` is added to the script's environment.
fn session(args: &[&str], env: &[(&str, &str)], fail_exec: bool) -> Session {
    let dir: TempDir = tempfile::tempdir().unwrap();
    let stub = dir.path().join("docker");
    fs::write(
        &stub,
        "#!/bin/sh\n\
         n=$(( $(ls \"$DOCKER_LOG\" | wc -l) ))\n\
         for a in \"$@\"; do printf '%s\\0' \"$a\"; done > \"$DOCKER_LOG/$(printf '%03d' \"$n\")\"\n\
         if [ \"$1\" = exec ] && [ -n \"${DOCKER_FAIL_EXEC:-}\" ]; then exit 1; fi\n\
         exit 0\n",
    )
    .unwrap();
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
    let log = dir.path().join("log");
    fs::create_dir(&log).unwrap();

    let mut path = vec![dir.path().to_path_buf()];
    path.extend(env::split_paths(&env::var_os("PATH").unwrap()));
    let mut cmd = Command::new(repo::path(SCRIPT));
    cmd.args(args)
        .env("PATH", env::join_paths(path).unwrap())
        .env("DOCKER_LOG", &log)
        .env_remove("ANTHOLOGY_RUST_SESSION")
        .envs(env.iter().copied());
    if fail_exec {
        cmd.env("DOCKER_FAIL_EXEC", "1");
    }
    let out = cmd.output().expect("script ran");

    let mut files: Vec<_> = fs::read_dir(&log)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    let calls = files
        .iter()
        .map(|f| {
            fs::read_to_string(f)
                .unwrap()
                .split_terminator('\0')
                .map(str::to_owned)
                .collect()
        })
        .collect();
    Session {
        status: out.status.code().unwrap_or(-1),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        calls,
    }
}

#[test]
fn a_session_name_must_be_one_of_ours() {
    for mode in ["session-start", "session-stop"] {
        for bad in [
            "",
            "foo",
            "anthology-release-",
            "anthology-rust-target-1",
            "anthology-cargo",
            "release-1-2",
            "anthology-release-a b",
            "anthology-release-x;y",
            "anthology-release-$(id)",
            "anthology-release-../x",
        ] {
            let s = session(&[mode, bad], &[], false);
            assert_eq!(s.status, 2, "{mode} {bad:?}: {}", s.stderr);
            assert!(s.calls.is_empty(), "{mode} {bad:?}: docker was called");
        }
        let s = session(&[mode], &[], false);
        assert_eq!(s.status, 2, "{mode} without a name");
        assert!(s.calls.is_empty());
    }
}

#[test]
fn session_start_runs_one_detached_container_then_sets_it_up_with_exec() {
    let s = session(&["session-start", NAME], &[], false);
    assert_eq!(s.status, 0, "{}", s.stderr);
    assert_eq!(s.verbs(), ["run", "exec"]);
    let run = &s.calls[0];
    assert!(run.contains(&"-d".to_owned()), "{run:?}");
    assert!(run.contains(&"--rm".to_owned()), "{run:?}");
    let at = run.iter().position(|a| a == "--name").unwrap();
    assert_eq!(run[at + 1], NAME);
    // It waits for a bounded time: a container nobody removes ends by itself.
    assert_eq!(&run[run.len() - 2..], ["sleep", "14400"], "{run:?}");
    let exec = &s.calls[1];
    assert_eq!(&exec[..2], ["exec", NAME]);
    let setup = exec.last().unwrap();
    assert!(setup.contains("apt-get install"), "{setup}");
    assert!(setup.contains("rustup toolchain install"), "{setup}");
    assert!(
        !setup.contains("runuser"),
        "the setup does not launch a command"
    );
}

#[test]
fn a_failed_setup_removes_the_container_and_fails() {
    let s = session(&["session-start", NAME], &[], true);
    assert_ne!(s.status, 0);
    assert_eq!(s.verbs(), ["run", "exec", "rm"]);
    assert_eq!(s.calls[2], ["rm", "-f", NAME]);
}

#[test]
fn session_stop_removes_that_container_only() {
    let s = session(&["session-stop", NAME], &[], false);
    assert_eq!(s.status, 0, "{}", s.stderr);
    assert_eq!(s.calls, [["rm", "-f", NAME]]);
}

#[test]
fn with_a_session_every_mode_is_one_exec_without_the_setup() {
    for args in [
        &["run", "echo", "hi"][..],
        &["test", "-p", "foo"],
        &["lint"],
    ] {
        let s = session(args, &[("ANTHOLOGY_RUST_SESSION", NAME)], false);
        assert_eq!(s.status, 0, "{args:?}: {}", s.stderr);
        assert_eq!(s.verbs(), ["exec"], "{args:?}");
        let call = &s.calls[0];
        assert!(call.contains(&NAME.to_owned()), "{args:?}: {call:?}");
        let script = &call[call.iter().position(|a| a == "-c").unwrap() + 1];
        assert!(!script.contains("apt-get"), "{args:?}: the setup ran again");
        assert!(script.contains("runuser"), "{args:?}");
        let at = call.iter().position(|a| a == "rust-test").unwrap();
        assert_eq!(call[at + 1..], *args, "{args:?}");
        // Only `run` reads its stdin.
        assert_eq!(
            call.contains(&"-i".to_owned()),
            args[0] == "run",
            "{args:?}"
        );
    }
}

#[test]
fn without_a_session_run_is_still_one_fresh_container_with_the_setup() {
    let s = session(&["run", "echo", "hi"], &[], false);
    assert_eq!(s.verbs(), ["run"]);
    let call = &s.calls[0];
    let script = &call[call.iter().position(|a| a == "-c").unwrap() + 1];
    assert!(script.contains("apt-get install") && script.contains("runuser"));
}
