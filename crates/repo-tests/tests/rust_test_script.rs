//! `scripts/rust-test.sh`: which cargo scope the container runs, the sessions, and the target
//! volume clean-up (`gc`, `session-stop`, one-shot runs). A stub `docker` on PATH records its
//! argv, so the script runs without a container.

use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

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
        // The stub keeps only the last call: these tests read the cargo run, not the clean-up.
        .env("ANTHOLOGY_KEEP_TARGET", "1")
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

    /// The volumes `docker volume rm` was asked to remove, in order.
    fn removed(&self) -> Vec<&str> {
        self.calls
            .iter()
            .filter(|c| c[..2] == ["volume", "rm"])
            .map(|c| c.last().unwrap().as_str())
            .collect()
    }

    /// The source of the `-v SOURCE:/target` mount of the `docker run` call.
    fn target_volume(&self) -> &str {
        let run = self.calls.iter().find(|c| c[0] == "run").expect("a run");
        run.iter()
            .find_map(|a| a.strip_suffix(":/target"))
            .unwrap_or_else(|| panic!("no /target mount in {run:?}"))
    }
}

const NAME: &str = "anthology-release-123-456";
const TARGET: &str = "anthology-rust-target-";

/// What the stubs `docker` and `git` do besides recording the call. The default is a Docker
/// that succeeds, lists no volume, and a git that cannot list worktrees.
#[derive(Default)]
struct Rig<'a> {
    /// Added to the script's environment.
    env: &'a [(&'a str, &'a str)],
    /// `docker exec` fails.
    fail_exec: bool,
    /// `docker run` exits 3.
    fail_run: bool,
    /// `docker volume ls` fails.
    fail_ls: bool,
    /// `docker volume rm` fails (the volume is in use).
    fail_volume_rm: bool,
    /// The dangling volumes `docker volume ls` prints, each with its `CreatedAt`.
    volumes: &'a [(String, String)],
    /// The worktree paths `git worktree list --porcelain` prints; `None`: git fails.
    worktrees: Option<&'a [PathBuf]>,
}

fn session(args: &[&str], env: &[(&str, &str)], fail_exec: bool) -> Session {
    go(
        args,
        &Rig {
            env,
            fail_exec,
            ..Rig::default()
        },
    )
}

fn executable(path: &Path, text: &str) {
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// Runs the script with `args` against a stub `docker` that records each call, and a stub `git`.
fn go(args: &[&str], rig: &Rig) -> Session {
    let dir: TempDir = tempfile::tempdir().unwrap();
    executable(
        &dir.path().join("docker"),
        "#!/bin/sh\n\
         n=$(( $(ls \"$DOCKER_LOG\" | wc -l) ))\n\
         for a in \"$@\"; do printf '%s\\0' \"$a\"; done > \"$DOCKER_LOG/$(printf '%03d' \"$n\")\"\n\
         if [ \"$1\" = exec ] && [ -n \"${DOCKER_FAIL_EXEC:-}\" ]; then exit 1; fi\n\
         if [ \"$1\" = run ] && [ -n \"${DOCKER_FAIL_RUN:-}\" ]; then exit 3; fi\n\
         if [ \"$1 $2\" = 'volume ls' ]; then\n\
         \x20 [ -z \"${DOCKER_FAIL_LS:-}\" ] || exit 1\n\
         \x20 cut -f1 \"$DOCKER_VOLUMES\"; exit 0\n\
         fi\n\
         if [ \"$1 $2\" = 'volume inspect' ]; then\n\
         \x20 for last; do :; done\n\
         \x20 awk -F'\\t' -v n=\"$last\" '$1 == n {print $2}' \"$DOCKER_VOLUMES\"; exit 0\n\
         fi\n\
         if [ \"$1 $2\" = 'volume rm' ] && [ -n \"${DOCKER_FAIL_VOLUME_RM:-}\" ]; then\n\
         \x20 echo 'volume is in use' >&2; exit 1\n\
         fi\n\
         exit 0\n",
    );
    executable(
        &dir.path().join("git"),
        "#!/bin/sh\n\
         case \"$*\" in\n\
         \x20 *'worktree list --porcelain'*) [ -f \"$FAKE_WORKTREES\" ] || exit 128; cat \"$FAKE_WORKTREES\"; exit 0 ;;\n\
         esac\n\
         exit 1\n",
    );
    let log = dir.path().join("log");
    fs::create_dir(&log).unwrap();
    let volumes = dir.path().join("volumes");
    let listing: String = rig
        .volumes
        .iter()
        .map(|(name, created)| format!("{name}\t{created}\n"))
        .collect();
    fs::write(&volumes, listing).unwrap();
    let worktrees = dir.path().join("worktrees");
    if let Some(paths) = rig.worktrees {
        let porcelain: String = paths
            .iter()
            .map(|p| format!("worktree {}\nHEAD 0000\ndetached\n\n", p.display()))
            .collect();
        fs::write(&worktrees, porcelain).unwrap();
    }

    let mut path = vec![dir.path().to_path_buf()];
    path.extend(env::split_paths(&env::var_os("PATH").unwrap()));
    let mut cmd = Command::new(repo::path(SCRIPT));
    cmd.args(args)
        .env("PATH", env::join_paths(path).unwrap())
        .env("DOCKER_LOG", &log)
        .env("DOCKER_VOLUMES", &volumes)
        .env("FAKE_WORKTREES", &worktrees)
        .env_remove("ANTHOLOGY_RUST_SESSION")
        .env_remove("ANTHOLOGY_KEEP_TARGET")
        .env_remove("ANTHOLOGY_GC_HOURS")
        .env_remove("ANTHOLOGY_SCCACHE")
        .env_remove("CI")
        .envs(rig.env.iter().copied());
    for (on, var) in [
        (rig.fail_exec, "DOCKER_FAIL_EXEC"),
        (rig.fail_run, "DOCKER_FAIL_RUN"),
        (rig.fail_ls, "DOCKER_FAIL_LS"),
        (rig.fail_volume_rm, "DOCKER_FAIL_VOLUME_RM"),
    ] {
        if on {
            cmd.env(var, "1");
        }
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
    // The first call is the `gc` listing (its own test below).
    assert_eq!(s.verbs(), ["volume", "run", "exec"]);
    let run = &s.calls[1];
    assert!(run.contains(&"-d".to_owned()), "{run:?}");
    assert!(run.contains(&"--rm".to_owned()), "{run:?}");
    let at = run.iter().position(|a| a == "--name").unwrap();
    assert_eq!(run[at + 1], NAME);
    // It waits for a bounded time: a container nobody removes ends by itself.
    assert_eq!(&run[run.len() - 2..], ["sleep", "14400"], "{run:?}");
    let exec = &s.calls[2];
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
fn a_failed_setup_removes_the_container_and_its_target_volume_and_fails() {
    let s = session(&["session-start", NAME], &[], true);
    assert_ne!(s.status, 0);
    assert_eq!(s.verbs(), ["volume", "run", "exec", "rm", "volume"]);
    assert_eq!(s.calls[3], ["rm", "-f", NAME]);
    assert_eq!(s.removed(), [s.target_volume()]);
}

#[test]
fn session_stop_removes_that_container_then_its_target_volume() {
    let stop = session(&["session-stop", NAME], &[], false);
    assert_eq!(stop.status, 0, "{}", stop.stderr);
    assert_eq!(stop.verbs(), ["rm", "volume"]);
    assert_eq!(stop.calls[0], ["rm", "-f", NAME]);
    let volume = stop.removed()[0];
    // The volume `session-start` mounted for this worktree.
    let start = session(&["session-start", NAME], &[], false);
    assert_eq!(volume, start.target_volume());
    assert!(volume.starts_with(TARGET), "{volume}");
}

#[test]
fn session_stop_keeps_the_target_volume_on_request() {
    let s = session(
        &["session-stop", NAME],
        &[("ANTHOLOGY_KEEP_TARGET", "1")],
        false,
    );
    assert_eq!(s.status, 0, "{}", s.stderr);
    assert_eq!(s.calls, [["rm", "-f", NAME]]);
}

#[test]
fn session_stop_succeeds_when_the_volume_is_in_use() {
    // Another session of the same worktree still mounts it: Docker refuses, the stop is done.
    let s = go(
        &["session-stop", NAME],
        &Rig {
            fail_volume_rm: true,
            ..Rig::default()
        },
    );
    assert_eq!(s.status, 0, "{}", s.stderr);
    assert_eq!(s.verbs(), ["rm", "volume"]);
}

#[test]
fn session_start_runs_gc_before_it_starts_the_container() {
    let s = session(&["session-start", NAME], &[], false);
    assert_eq!(
        s.calls[0],
        ["volume", "ls", "--quiet", "--filter", "dangling=true"]
    );
    assert_eq!(s.calls[1][0], "run");
}

#[test]
fn session_start_goes_on_when_gc_cannot_list_volumes() {
    let s = go(
        &["session-start", NAME],
        &Rig {
            fail_ls: true,
            ..Rig::default()
        },
    );
    assert_eq!(s.status, 0, "{}", s.stderr);
    assert_eq!(s.verbs(), ["volume", "run", "exec"]);
}

#[test]
fn session_start_gc_removes_a_leaked_volume_before_the_container_starts() {
    let leaked = vec![(format!("{TARGET}1"), rfc3339(now() - 1, 0))];
    let s = go(
        &["session-start", NAME],
        &Rig {
            volumes: &leaked,
            worktrees: Some(&[]),
            ..Rig::default()
        },
    );
    assert_eq!(s.status, 0, "{}", s.stderr);
    let verbs = s.verbs();
    let rm = verbs.iter().position(|v| *v == "volume").unwrap();
    let run = verbs.iter().position(|v| *v == "run").unwrap();
    assert!(rm < run, "{verbs:?}");
    assert_eq!(s.removed(), [format!("{TARGET}1")]);
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
    let s = session(
        &["run", "echo", "hi"],
        &[("ANTHOLOGY_KEEP_TARGET", "1")],
        false,
    );
    assert_eq!(s.verbs(), ["run"]);
    let call = &s.calls[0];
    let script = &call[call.iter().position(|a| a == "-c").unwrap() + 1];
    assert!(script.contains("apt-get install") && script.contains("runuser"));
}

// ---- one-shot runs: the target volume goes when the run ends --------------------------------

#[test]
fn a_one_shot_run_removes_its_target_volume_when_it_ends() {
    for args in [
        &["run", "echo", "hi"][..],
        &["test", "-p", "foo"],
        &["lint"],
        &[],
    ] {
        let s = session(args, &[], false);
        assert_eq!(s.status, 0, "{args:?}: {}", s.stderr);
        assert_eq!(s.verbs(), ["run", "volume"], "{args:?}");
        let volume = s.target_volume();
        assert!(volume.starts_with(TARGET), "{args:?}: {volume}");
        assert_eq!(s.calls[1], ["volume", "rm", volume], "{args:?}");
    }
}

#[test]
fn a_one_shot_run_keeps_its_target_volume_on_request() {
    let s = session(&["test"], &[("ANTHOLOGY_KEEP_TARGET", "1")], false);
    assert_eq!(s.verbs(), ["run"]);
    // `0` and the empty string are not a request.
    for off in ["0", ""] {
        let s = session(&["test"], &[("ANTHOLOGY_KEEP_TARGET", off)], false);
        assert_eq!(s.verbs(), ["run", "volume"], "{off:?}");
    }
}

#[test]
fn a_failed_run_still_removes_the_volume_and_keeps_its_exit_status() {
    let s = go(
        &["test"],
        &Rig {
            fail_run: true,
            ..Rig::default()
        },
    );
    assert_eq!(s.status, 3, "{}", s.stderr);
    assert_eq!(s.verbs(), ["run", "volume"]);
}

#[test]
fn a_run_survives_docker_refusing_to_remove_the_volume() {
    let s = go(
        &["lint"],
        &Rig {
            fail_volume_rm: true,
            ..Rig::default()
        },
    );
    assert_eq!(s.status, 0, "{}", s.stderr);
    assert_eq!(s.verbs(), ["run", "volume"]);
}

#[test]
fn a_one_shot_run_does_not_run_gc() {
    let s = session(&["lint"], &[], false);
    assert!(!s.calls.iter().any(|c| c[..2] == ["volume", "ls"]));
}

// ---- sccache: one compile cache for the dependencies, shared by every checkout -------------

/// The `-e NAME=VALUE` environment the call passes to the container.
fn container_env<'a>(call: &'a [String], name: &str) -> Option<&'a str> {
    let prefix = format!("{name}=");
    call.windows(2)
        .find(|w| w[0] == "-e" && w[1].starts_with(&prefix))
        .map(|w| &w[1][prefix.len()..])
}

/// The container shell script of a `docker run` or `docker exec` call.
fn script_of(call: &[String]) -> &str {
    &call[call.iter().position(|a| a == "-c").unwrap() + 1]
}

#[test]
fn a_fresh_container_gets_the_shared_sccache_volume_and_the_wrapper() {
    let s = session(
        &["run", "echo", "hi"],
        &[("ANTHOLOGY_KEEP_TARGET", "1")],
        false,
    );
    let run = &s.calls[0];
    assert!(
        run.windows(2)
            .any(|w| w[0] == "-v" && w[1] == "anthology-sccache:/sccache"),
        "{run:?}"
    );
    assert_eq!(container_env(run, "SCCACHE_ON"), Some("1"), "{run:?}");
    let script = script_of(run);
    // The version is pinned to the upstream release; Debian's revision may move.
    assert!(
        script.contains(r#"apt-get install -y -qq --no-install-recommends "sccache=0.10.0-*""#),
        "{script}"
    );
    assert!(
        script.contains("find /sccache ! -uid"),
        "the volume is handed to the tester"
    );
    assert!(script.contains("RUSTC_WRAPPER=sccache"), "{script}");
    assert!(script.contains("SCCACHE_DIR=/sccache"), "{script}");
    // A session container set up without sccache must still run.
    assert!(script.contains("command -v sccache"), "{script}");
}

#[test]
fn sccache_leaves_the_target_and_incremental_builds_alone() {
    let s = session(&["test"], &[("ANTHOLOGY_KEEP_TARGET", "1")], false);
    let script = script_of(&s.calls[0]);
    assert!(script.contains("CARGO_TARGET_DIR=/target"), "{script}");
    assert!(!script.contains("CARGO_INCREMENTAL"), "{script}");
}

#[test]
fn sccache_is_off_when_asked_and_in_ci() {
    for env in [
        &[("ANTHOLOGY_SCCACHE", "0")][..],
        &[("CI", "true")],
        &[("CI", "true"), ("ANTHOLOGY_SCCACHE", "0")],
    ] {
        let mut all = vec![("ANTHOLOGY_KEEP_TARGET", "1")];
        all.extend_from_slice(env);
        let s = session(&["lint"], &all, false);
        let run = &s.calls[0];
        assert!(
            !run.iter().any(|a| a.contains("sccache:")),
            "{env:?}: {run:?}"
        );
        assert_eq!(container_env(run, "SCCACHE_ON"), Some(""), "{env:?}");
        assert!(
            !script_of(run).contains("apt-get install -y -qq --no-install-recommends \"sccache"),
            "{env:?}"
        );
    }
}

#[test]
fn sccache_can_be_asked_for_in_ci() {
    let s = session(
        &["lint"],
        &[
            ("ANTHOLOGY_KEEP_TARGET", "1"),
            ("CI", "true"),
            ("ANTHOLOGY_SCCACHE", "1"),
        ],
        false,
    );
    assert_eq!(container_env(&s.calls[0], "SCCACHE_ON"), Some("1"));
}

#[test]
fn a_session_sets_up_sccache_once_and_every_exec_names_it() {
    let start = session(&["session-start", NAME], &[], false);
    let run = &start.calls[1];
    assert!(
        run.contains(&"anthology-sccache:/sccache".to_owned()),
        "{run:?}"
    );
    let setup = start.calls[2].last().unwrap();
    assert!(setup.contains("\"sccache=0.10.0-*\""), "{setup}");
    let exec = session(&["lint"], &[("ANTHOLOGY_RUST_SESSION", NAME)], false);
    assert_eq!(container_env(&exec.calls[0], "SCCACHE_ON"), Some("1"));
}

// ---- gc ------------------------------------------------------------------------------------

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

const HOUR: i64 = 3600;

/// `epoch` as Docker prints `CreatedAt`: RFC 3339 with the wall clock at `offset_min` minutes
/// from UTC (`Z` for 0).
fn rfc3339(epoch: i64, offset_min: i64) -> String {
    let local = epoch + offset_min * 60;
    let (days, secs) = (local.div_euclid(86400), local.rem_euclid(86400));
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let zone = match offset_min {
        0 => "Z".to_owned(),
        m => format!(
            "{}{:02}:{:02}",
            if m < 0 { '-' } else { '+' },
            m.abs() / 60,
            m.abs() % 60
        ),
    };
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}{zone}",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

#[test]
fn the_test_timestamps_are_what_docker_prints() {
    assert_eq!(rfc3339(0, 0), "1970-01-01T00:00:00Z");
    assert_eq!(rfc3339(1_795_785_600, 0), "2026-11-27T13:20:00Z");
    assert_eq!(rfc3339(1_795_785_600, 330), "2026-11-27T18:50:00+05:30");
    assert_eq!(rfc3339(1_795_785_600, -480), "2026-11-27T05:20:00-08:00");
}

/// The id the script puts in a checkout's volume name: `printf %s PATH | cksum`.
fn checkout_id(path: &Path) -> String {
    let mut child = Command::new("cksum")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    std::io::Write::write_all(
        child.stdin.as_mut().unwrap(),
        path.to_str().unwrap().as_bytes(),
    )
    .unwrap();
    let out = child.wait_with_output().unwrap();
    String::from_utf8(out.stdout)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_owned()
}

fn target_of(dir: &Path) -> String {
    format!("{TARGET}{}", checkout_id(dir))
}

/// `gc` against dangling `volumes` (name, CreatedAt) and the live worktrees `live`.
fn gc(volumes: &[(String, String)], live: &[PathBuf], env: &[(&str, &str)]) -> Session {
    let s = go(
        &["gc"],
        &Rig {
            env,
            volumes,
            worktrees: Some(live),
            ..Rig::default()
        },
    );
    assert_eq!(s.status, 0, "{}", s.stderr);
    s
}

fn vol(name: &str, age_hours: i64) -> (String, String) {
    (name.to_owned(), rfc3339(now() - age_hours * HOUR, 0))
}

#[test]
fn gc_asks_docker_for_dangling_volumes_only() {
    let s = gc(&[], &[], &[]);
    assert_eq!(
        s.calls,
        [["volume", "ls", "--quiet", "--filter", "dangling=true"]]
    );
}

#[test]
fn gc_removes_a_volume_whose_worktree_is_gone_however_young() {
    let live = tempfile::tempdir().unwrap();
    let gone = tempfile::tempdir().unwrap();
    let gone_volume = target_of(gone.path());
    let live_volume = target_of(live.path());
    // The gone worktree is not in git's list; a stale entry whose folder is deleted counts as gone.
    let deleted = live.path().join("deleted");
    let stale = target_of(&deleted);
    let s = gc(
        &[vol(&gone_volume, 1), vol(&live_volume, 1), vol(&stale, 1)],
        &[live.path().to_path_buf(), deleted],
        &[],
    );
    assert_eq!(s.removed(), [gone_volume, stale]);
}

#[test]
fn gc_keeps_a_young_volume_of_a_live_worktree() {
    let live = tempfile::tempdir().unwrap();
    let s = gc(
        &[vol(&target_of(live.path()), 47)],
        &[live.path().to_path_buf()],
        &[],
    );
    assert!(s.removed().is_empty(), "{:?}", s.calls);
}

#[test]
fn gc_removes_an_old_dangling_volume_even_when_its_worktree_lives() {
    let live = tempfile::tempdir().unwrap();
    let name = target_of(live.path());
    let s = gc(&[vol(&name, 49)], &[live.path().to_path_buf()], &[]);
    assert_eq!(s.removed(), [name]);
}

#[test]
fn gc_hours_come_from_the_environment() {
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (old, young) = (target_of(a.path()), target_of(b.path()));
    let s = gc(
        &[vol(&old, 3), vol(&young, 1)],
        &[a.path().to_path_buf(), b.path().to_path_buf()],
        &[("ANTHOLOGY_GC_HOURS", "2")],
    );
    assert_eq!(s.removed(), [old]);
}

#[test]
fn gc_rejects_hours_that_are_not_a_number() {
    for bad in ["x", "1.5", "-3", "1;rm"] {
        let s = go(
            &["gc"],
            &Rig {
                env: &[("ANTHOLOGY_GC_HOURS", bad)],
                ..Rig::default()
            },
        );
        assert_eq!(s.status, 2, "{bad:?}: {}", s.stderr);
        assert!(s.calls.is_empty(), "{bad:?}");
    }
}

#[test]
fn gc_takes_the_atl_target_by_age_only() {
    let young = gc(&[vol("anthology-atl-target", 1)], &[], &[]);
    assert!(young.removed().is_empty(), "{:?}", young.calls);
    let old = gc(&[vol("anthology-atl-target", 49)], &[], &[]);
    assert_eq!(old.removed(), ["anthology-atl-target"]);
}

#[test]
fn gc_never_touches_other_volumes() {
    let names = [
        "anthology-cargo",
        "anthology-sccache",
        "anthology-rustup-0123456789ab",
        "anthology-atl-release",
        "anthology-atl-target-2",
        "anthology-rust-target",
        "anthology-rust-targets-1",
        "anthology-release-123-456",
        "other-rust-target-1",
        "my-anthology-rust-target-1",
        "anthology-rust-target-1;id",
        "anthology-rust-target-$(id)",
        "data",
    ];
    let volumes: Vec<_> = names.iter().map(|n| vol(n, 5000)).collect();
    let s = gc(&volumes, &[], &[]);
    assert!(s.removed().is_empty(), "{:?}", s.removed());
}

#[test]
fn gc_with_no_worktree_list_goes_by_age_only() {
    let (gone, old) = (
        vol(&format!("{TARGET}1"), 1),
        vol(&format!("{TARGET}2"), 49),
    );
    let s = go(
        &["gc"],
        &Rig {
            volumes: &[gone, old],
            worktrees: None,
            ..Rig::default()
        },
    );
    assert_eq!(s.status, 0, "{}", s.stderr);
    assert_eq!(s.removed(), [format!("{TARGET}2")]);
}

#[test]
fn gc_reads_zone_offsets_and_fractions_of_a_second() {
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    // 3 hours old, written as the wall clock 5:30 ahead of UTC with fractional seconds;
    // 1 hour old, written 8 hours behind UTC. Read as UTC, each would be judged the wrong way.
    let (old, young) = (target_of(a.path()), target_of(b.path()));
    let old_at = rfc3339(now() - 3 * HOUR, 330).replace('+', ".123456789+");
    let young_at = rfc3339(now() - HOUR, -480);
    let s = gc(
        &[(old.clone(), old_at), (young, young_at)],
        &[a.path().to_path_buf(), b.path().to_path_buf()],
        &[("ANTHOLOGY_GC_HOURS", "2")],
    );
    assert_eq!(s.removed(), [old]);
}

#[test]
fn gc_does_not_judge_an_age_it_cannot_read() {
    let live = tempfile::tempdir().unwrap();
    let name = target_of(live.path());
    let s = gc(
        &[(name, "garbage".to_owned())],
        &[live.path().to_path_buf()],
        &[],
    );
    assert!(s.removed().is_empty(), "{:?}", s.calls);
}

#[test]
fn gc_goes_on_when_docker_refuses_one_removal() {
    let volumes = [vol(&format!("{TARGET}1"), 1), vol(&format!("{TARGET}2"), 1)];
    let s = go(
        &["gc"],
        &Rig {
            volumes: &volumes,
            worktrees: Some(&[]),
            fail_volume_rm: true,
            ..Rig::default()
        },
    );
    assert_eq!(s.status, 0, "{}", s.stderr);
    // It asked for both, and the refusal of the first did not stop it.
    assert_eq!(s.removed(), [format!("{TARGET}1"), format!("{TARGET}2")]);
}

#[test]
fn gc_fails_quietly_when_docker_cannot_list() {
    let s = go(
        &["gc"],
        &Rig {
            fail_ls: true,
            ..Rig::default()
        },
    );
    assert_eq!(s.status, 0, "{}", s.stderr);
    assert!(s.removed().is_empty());
}

#[test]
fn gc_takes_no_arguments() {
    let s = session(&["gc", "extra"], &[], false);
    assert_eq!(s.status, 2, "{}", s.stderr);
    assert!(s.calls.is_empty());
}
