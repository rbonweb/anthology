//! `scripts/remote-build.sh`: run the Docker-based Rust checks on a remote Linux build host over
//! ssh. A fake `ssh` on PATH records its argv and runs the command locally with a temp dir as the
//! remote home; a fake `docker` records its argv; the temp repo's `scripts/rust-test.sh` is a
//! stand-in that prints its arguments and "starts" a container. No host, no Docker, no network.

use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use repo_tests::repo;
use tempfile::TempDir;

const SCRIPT: &str = "scripts/remote-build.sh";
/// Stands in for the build host's ssh alias; the script must never print it.
const ALIAS: &str = "sekrit-build-alias";

const FAKE_SSH: &str = r#"#!/bin/sh
printf 'ssh %s\n' "$*" >> "$FAKE_STATE/ssh.log"
while [ $# -gt 0 ]; do
    case $1 in
        -o) shift 2 ;;
        -*) shift ;;
        *) break ;;
    esac
done
shift
cd "$FAKE_REMOTE_HOME"
case $* in
    *rust-test.sh*)
        if [ -n "${FAKE_SSH_DROP:-}" ]; then
            # The link drops two seconds in: the command's stdin ends, as sshd's would.
            sleep 2 | HOME=$FAKE_REMOTE_HOME sh -c "$*"
            exit $?
        fi
        ;;
esac
HOME=$FAKE_REMOTE_HOME exec sh -c "$*"
"#;

const FAKE_DOCKER: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$FAKE_STATE/docker.log"
case $1 in
    ps) cat "$FAKE_STATE/containers" 2>/dev/null || true ;;
    rm) : > "$FAKE_STATE/containers" ;;
esac
"#;

const FAKE_RUST_TEST: &str = r#"#!/bin/sh
echo "rust-test args: $*"
echo "rust-test cwd: $(pwd)"
echo cont1 >> "$FAKE_STATE/containers"
[ -z "${FAKE_HANG:-}" ] || exec sleep 30
exit "${FAKE_EXIT:-0}"
"#;

fn executable(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn git(repo: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("git ran");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

struct Rig {
    _dir: TempDir,
    repo: PathBuf,
    bin: PathBuf,
    state: PathBuf,
    home: PathBuf,
}

impl Rig {
    /// A git checkout holding the real script and a fake `rust-test.sh`: a tracked file, a tracked
    /// file deleted from disk, an untracked one, and files the sync must leave out.
    fn new() -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("checkout");
        let bin = dir.path().join("bin");
        let state = dir.path().join("state");
        let home = dir.path().join("remote-home");
        for d in [&repo, &state, &home] {
            fs::create_dir_all(d).unwrap();
        }
        executable(&bin.join("ssh"), FAKE_SSH);
        executable(&bin.join("docker"), FAKE_DOCKER);
        executable(
            &repo.join(SCRIPT),
            &fs::read_to_string(repo::path(SCRIPT)).expect("scripts/remote-build.sh exists"),
        );
        executable(&repo.join("scripts/rust-test.sh"), FAKE_RUST_TEST);
        fs::write(repo.join(".gitignore"), "target/\n").unwrap();
        fs::create_dir_all(repo.join("src")).unwrap();
        fs::write(repo.join("src/lib.rs"), "v1\n").unwrap();
        fs::write(repo.join("gone.txt"), "x\n").unwrap();
        git(&repo, &["init", "-q"]);
        git(&repo, &["add", "."]);
        fs::remove_file(repo.join("gone.txt")).unwrap();
        fs::write(repo.join("new.txt"), "untracked\n").unwrap();
        for ignored in ["target/junk", ".claude/worktrees/w/file"] {
            let p = repo.join(ignored);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, "no\n").unwrap();
        }
        Rig {
            _dir: dir,
            repo,
            bin,
            state,
            home,
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut path = vec![self.bin.clone()];
        path.extend(env::split_paths(&env::var_os("PATH").unwrap()));
        let mut cmd = Command::new(self.repo.join(SCRIPT));
        cmd.args(args)
            .current_dir(&self.repo)
            .env("PATH", env::join_paths(path).unwrap())
            .env("ANTHOLOGY_BUILD_HOST", ALIAS)
            .env("ANTHOLOGY_REMOTE_SYNC", "tar")
            .env("FAKE_STATE", &self.state)
            .env("FAKE_REMOTE_HOME", &self.home)
            .stdin(Stdio::null());
        cmd
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("script ran")
    }

    /// The one checkout folder on the fake host.
    fn remote_dir(&self) -> PathBuf {
        let parent = self.home.join("anthology-remote");
        let mut dirs = fs::read_dir(&parent)
            .unwrap_or_else(|e| panic!("no {}: {e}", parent.display()))
            .map(|e| e.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(dirs.len(), 1, "{dirs:?}");
        dirs.remove(0)
    }

    fn log(&self, name: &str) -> String {
        fs::read_to_string(self.state.join(name)).unwrap_or_default()
    }

    fn containers(&self) -> String {
        self.log("containers")
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn all_output(out: &Output) -> String {
    format!("{}{}", text(&out.stdout), text(&out.stderr))
}

/// The cksum the Rust runner puts in the target volume name, for a folder path.
fn volume_for(path: &Path) -> String {
    let out = Command::new("sh")
        .args(["-c", r#"printf '%s' "$0" | cksum | cut -d' ' -f1"#])
        .arg(path)
        .output()
        .unwrap();
    format!("anthology-rust-target-{}", text(&out.stdout).trim())
}

#[test]
fn an_unset_host_prints_usage_and_exits_2() {
    let rig = Rig::new();
    let out = rig
        .command(&["test", "-p", "x"])
        .env_remove("ANTHOLOGY_BUILD_HOST")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("usage:"), "{}", all_output(&out));
    assert!(rig.log("ssh.log").is_empty(), "ssh was called");
}

#[test]
fn an_empty_host_is_unset() {
    let rig = Rig::new();
    let out = rig
        .command(&["lint"])
        .env("ANTHOLOGY_BUILD_HOST", "")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn a_host_that_looks_like_an_option_is_refused() {
    let rig = Rig::new();
    let out = rig
        .command(&["lint"])
        .env("ANTHOLOGY_BUILD_HOST", "-oProxyCommand=x")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(rig.log("ssh.log").is_empty(), "ssh was called");
    assert!(!all_output(&out).contains("ProxyCommand"));
}

#[test]
fn arguments_reach_rust_test_on_the_host_and_output_streams_back() {
    let rig = Rig::new();
    let out = rig.run(&["test", "-p", "some crate", "--", "it's"]);
    let shown = all_output(&out);
    assert_eq!(out.status.code(), Some(0), "{shown}");
    assert!(
        text(&out.stdout).contains("rust-test args: test -p some crate -- it's"),
        "{shown}"
    );
    assert!(text(&out.stdout).contains("/anthology-remote/"), "{shown}");
    assert!(
        rig.log("ssh.log").contains(ALIAS),
        "alias is the ssh target"
    );
    assert!(rig.log("ssh.log").contains("BatchMode=yes"));
}

#[test]
fn the_exit_code_of_the_remote_run_is_the_exit_code() {
    let rig = Rig::new();
    for code in ["1", "7", "101"] {
        let out = rig
            .command(&["lint"])
            .env("FAKE_EXIT", code)
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            code.parse::<i32>().ok(),
            "{}",
            all_output(&out)
        );
    }
}

#[test]
fn the_sync_sends_tracked_and_new_files_but_not_target_or_worktrees() {
    let rig = Rig::new();
    assert_eq!(rig.run(&["lint"]).status.code(), Some(0));
    let remote = rig.remote_dir();
    assert_eq!(
        fs::read_to_string(remote.join("src/lib.rs")).unwrap(),
        "v1\n"
    );
    assert!(
        remote.join("new.txt").is_file(),
        "an untracked file is sent"
    );
    assert!(remote.join("scripts/rust-test.sh").is_file());
    assert!(!remote.join("gone.txt").exists(), "a file deleted on disk");
    assert!(!remote.join("target").exists());
    assert!(!remote.join(".claude/worktrees").exists());
}

#[test]
fn a_rerun_sends_changes_and_drops_files_that_left_the_checkout() {
    let rig = Rig::new();
    assert_eq!(rig.run(&["lint"]).status.code(), Some(0));
    let remote = rig.remote_dir();
    fs::write(remote.join("stale.txt"), "left over\n").unwrap();
    fs::write(rig.repo.join("src/lib.rs"), "v2\n").unwrap();
    fs::remove_file(rig.repo.join("new.txt")).unwrap();
    assert_eq!(rig.run(&["lint"]).status.code(), Some(0));
    assert_eq!(
        fs::read_to_string(remote.join("src/lib.rs")).unwrap(),
        "v2\n"
    );
    assert!(!remote.join("new.txt").exists());
    assert!(!remote.join("stale.txt").exists());
}

#[test]
fn rsync_syncs_the_same_files_when_both_ends_have_it() {
    // Only GNU rsync 3: macOS ships openrsync, and the CI image has none.
    let version = Command::new("rsync").arg("--version").output();
    let gnu = version
        .map(|o| text(&o.stdout).contains("rsync  version 3"))
        .unwrap_or(false);
    if !gnu {
        eprintln!("skipped: no GNU rsync 3 on PATH");
        return;
    }
    let rig = Rig::new();
    let out = rig
        .command(&["lint"])
        .env_remove("ANTHOLOGY_REMOTE_SYNC")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", all_output(&out));
    let remote = rig.remote_dir();
    assert!(remote.join("src/lib.rs").is_file());
    assert!(remote.join("new.txt").is_file());
    assert!(!remote.join("target").exists());
    assert!(!remote.join("gone.txt").exists());
}

#[test]
fn containers_of_the_checkout_are_removed_when_the_run_ends() {
    let rig = Rig::new();
    for code in ["0", "3"] {
        fs::write(rig.state.join("containers"), "").unwrap();
        rig.command(&["test"])
            .env("FAKE_EXIT", code)
            .output()
            .unwrap();
        assert_eq!(rig.containers().trim(), "", "exit {code}");
        let docker = rig.log("docker.log");
        assert!(docker.contains("rm -f cont1"), "{docker}");
        let volume = volume_for(&rig.remote_dir());
        assert!(docker.contains(&format!("volume={volume}")), "{docker}");
    }
}

#[test]
fn a_container_left_by_a_dropped_run_is_removed_before_the_next_one() {
    let rig = Rig::new();
    fs::write(rig.state.join("containers"), "stale1\n").unwrap();
    assert_eq!(rig.run(&["lint"]).status.code(), Some(0));
    assert!(rig.log("docker.log").contains("rm -f stale1"));
}

#[test]
fn a_dropped_ssh_link_stops_the_run_and_removes_its_containers() {
    let rig = Rig::new();
    let started = Instant::now();
    let out = rig
        .command(&["test"])
        .env("FAKE_SSH_DROP", "1")
        .env("FAKE_HANG", "1")
        .output()
        .unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "the run was not stopped"
    );
    assert_ne!(out.status.code(), Some(0), "{}", all_output(&out));
    assert_eq!(rig.containers().trim(), "");
    assert!(rig.log("docker.log").contains("rm -f cont1"));
}

#[test]
fn clean_removes_the_remote_folder_and_the_target_volume() {
    let rig = Rig::new();
    assert_eq!(rig.run(&["lint"]).status.code(), Some(0));
    let remote = rig.remote_dir();
    let volume = volume_for(&remote);
    let out = rig.run(&["--clean"]);
    assert_eq!(out.status.code(), Some(0), "{}", all_output(&out));
    assert!(!remote.exists(), "remote folder is gone");
    assert!(
        rig.log("docker.log")
            .contains(&format!("volume rm {volume}")),
        "{}",
        rig.log("docker.log")
    );
    assert!(!all_output(&out).contains(ALIAS));
}

#[test]
fn clean_before_any_run_succeeds() {
    let rig = Rig::new();
    let out = rig.run(&["--clean"]);
    assert_eq!(out.status.code(), Some(0), "{}", all_output(&out));
}

#[test]
fn clean_takes_no_other_arguments() {
    let rig = Rig::new();
    let out = rig.run(&["--clean", "lint"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(rig.log("ssh.log").is_empty(), "ssh was called");
}

#[test]
fn the_alias_never_appears_in_output() {
    let rig = Rig::new();
    let runs = [
        rig.run(&["test", "-p", "x"]),
        rig.command(&["lint"])
            .env("FAKE_EXIT", "9")
            .output()
            .unwrap(),
        rig.run(&["--clean"]),
        rig.command(&["lint"])
            .env_remove("ANTHOLOGY_BUILD_HOST")
            .output()
            .unwrap(),
        rig.command(&["lint"])
            .env("FAKE_SSH_DROP", "1")
            .env("FAKE_HANG", "1")
            .output()
            .unwrap(),
    ];
    for out in runs {
        assert!(!all_output(&out).contains(ALIAS), "{}", all_output(&out));
    }
}

#[test]
fn the_script_asks_for_no_privilege_and_installs_nothing() {
    let script = repo::read(SCRIPT);
    for word in [
        "sudo",
        "apt-get",
        "apt install",
        "ssh-keygen",
        "authorized_keys",
        "sshd_config",
    ] {
        assert!(!script.contains(word), "{SCRIPT} mentions {word}");
    }
}
