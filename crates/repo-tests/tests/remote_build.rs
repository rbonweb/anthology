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

/// A docker with state in three files, one item per line: `containers` (`<id> <volume> <running|
/// exited> <image>`), `volumes` (`<name> <created, RFC 3339> <ours|->`, `ours` meaning it carries
/// our label) and `images` (a reference). It answers the calls the script makes. Anything the
/// script does that could touch more than ours (a prune, a `system` call, a forced image removal)
/// is a failure here.
const FAKE_DOCKER: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$FAKE_STATE/docker.log"
touch "$FAKE_STATE/containers" "$FAKE_STATE/volumes" "$FAKE_STATE/images"
drop_line() { # drop_line FILE KEY: remove the lines whose first word is KEY
    grep -v "^$2 " "$1" > "$1.new" || true
    mv "$1.new" "$1"
}
case $1 in
    ps)
        shift
        all=
        vol=
        img=
        status=
        for a in "$@"; do
            case $a in
                -a | -aq) all=1 ;;
                volume=*) vol=${a#volume=} ;;
                ancestor=*) img=${a#ancestor=} ;;
                status=*) status="$status ${a#status=}" ;;
            esac
        done
        awk -v vol="$vol" -v img="$img" -v st="$status" -v all="$all" '
            BEGIN { n = split(st, s, " ") }
            (vol == "" || $2 == vol) && (img == "" || $4 == img) {
                ok = 0
                if (n == 0) ok = (all == 1 || $3 == "running")
                else for (i = 1; i <= n; i++) if ($3 == s[i]) ok = 1
                if (ok) print $1
            }' "$FAKE_STATE/containers"
        ;;
    rm)
        shift
        for a in "$@"; do
            case $a in -*) ;; *) drop_line "$FAKE_STATE/containers" "$a" ;; esac
        done
        ;;
    inspect)
        # A container: its volume, as `{{range .Mounts}}{{.Name}}` would list it.
        for last in "$@"; do :; done
        awk -v id="$last" '$1 == id { print $2 }' "$FAKE_STATE/containers"
        ;;
    volume)
        case $2 in
            ls)
                name=
                label=
                for a in "$@"; do
                    case $a in
                        name=*) name=${a#name=} ;;
                        label=*) label=${a#label=} ;;
                    esac
                done
                awk -v n="$name" -v l="$label" '
                    (n == "" || index($1, n) > 0) && (l == "" || ($3 == "ours" && l == "anthology.remote-build=1")) { print $1 }
                    ' "$FAKE_STATE/volumes"
                ;;
            create)
                tag=-
                for a in "$@"; do
                    [ "$a" != "anthology.remote-build=1" ] || tag=ours
                    name=$a
                done
                echo "$name $(date -u +%Y-%m-%dT%H:%M:%SZ) $tag" >> "$FAKE_STATE/volumes"
                ;;
            inspect)
                for last in "$@"; do :; done
                line=$(awk -v v="$last" '$1 == v { print $2 }' "$FAKE_STATE/volumes")
                [ -z "$line" ] || echo "$line"
                [ -n "$line" ]
                ;;
            rm) drop_line "$FAKE_STATE/volumes" "$3" ;;
            prune) echo "fake docker: volume prune refused" >&2; exit 1 ;;
        esac
        ;;
    image)
        [ "$2" != prune ] || { echo "fake docker: image prune refused" >&2; exit 1; }
        for last in "$@"; do :; done
        grep -qxF -- "$last" "$FAKE_STATE/images"
        ;;
    rmi)
        shift
        for a in "$@"; do
            case $a in -*) echo "fake docker: rmi flag $a refused" >&2; exit 1 ;; esac
            grep -vxF -- "$a" "$FAKE_STATE/images" > "$FAKE_STATE/images.new" || true
            mv "$FAKE_STATE/images.new" "$FAKE_STATE/images"
        done
        ;;
    system | prune | *prune) echo "fake docker: $1 refused" >&2; exit 1 ;;
    info) printf '%s\n' "$FAKE_DOCKER_ROOT" ;;
esac
"#;

/// `df -Pk PATH`: the free space is `FAKE_DF_KB`, or `FAKE_DF_DOCKER_KB` for the Docker data dir.
const FAKE_DF: &str = r#"#!/bin/sh
for last; do :; done
kb=${FAKE_DF_KB:-999999999}
[ "$last" != "$FAKE_DOCKER_ROOT" ] || kb=${FAKE_DF_DOCKER_KB:-$kb}
echo "Filesystem 1024-blocks Used Available Capacity Mounted on"
echo "fake 1000000000 1 $kb 1% /"
"#;

/// Starts a container on the volume the real runner would name: the cksum of its root path.
const FAKE_RUST_TEST: &str = r#"#!/bin/sh
echo "rust-test args: $*"
echo "rust-test cwd: $(pwd)"
[ -z "$(find "$HOME/anthology-remote/.last-used" -mmin -5 2>/dev/null)" ] || echo "last-used is fresh"
[ -z "${FAKE_AGE_LAST_USED:-}" ] || touch -d '5 hours ago' "$HOME/anthology-remote/.last-used"
root=$(cd "$(dirname "$0")/.." && pwd)
vol=anthology-rust-target-$(printf '%s' "$root" | cksum | cut -d' ' -f1)
image=$(awk '$1 == "FROM" {print $NF; exit}' "$root/ci/images/rust.Dockerfile")
rustup=anthology-rustup-$(printf '%s' "${image##*@sha256:}" | cut -c1-12)
# `docker run` pulls a missing image and creates a missing volume, with no label.
touch "$FAKE_STATE/images" "$FAKE_STATE/volumes"
grep -qxF -- "$image" "$FAKE_STATE/images" || echo "$image" >> "$FAKE_STATE/images"
for v in "$vol" anthology-cargo "$rustup"; do
    grep -q "^$v " "$FAKE_STATE/volumes" || echo "$v $(date -u +%Y-%m-%dT%H:%M:%SZ) -" >> "$FAKE_STATE/volumes"
done
echo "cont1 $vol running $image" >> "$FAKE_STATE/containers"
[ -z "${FAKE_HANG:-}" ] || exec sleep 30
exit "${FAKE_EXIT:-0}"
"#;

const DOCKERFILE: &str = "FROM rust@sha256:0123456789abcdef\n";
const IMAGE: &str = "rust@sha256:0123456789abcdef";

/// The fake host is this machine, so the remote half needs GNU tools (`stat -c`, `date -d`, `find
/// -printf`, `sort -z`): the Linux test container has them, macOS does not. Skip there.
fn gnu_userland() -> bool {
    let ok = |cmd: &str| {
        Command::new("sh")
            .args(["-c", cmd])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };
    ok(
        "date -d '1 hour ago' +%s && stat -c %Y . && find . -maxdepth 0 -printf x && printf 'a\\0' | sort -z",
    )
}

/// A rig, or an early return from the test where the fake host cannot work.
macro_rules! rig {
    () => {{
        if !gnu_userland() {
            eprintln!("skipped: needs GNU userland for the fake host");
            return;
        }
        Rig::new()
    }};
}

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
        for d in [&repo, &state, &home.join("docker-root")] {
            fs::create_dir_all(d).unwrap();
        }
        executable(&bin.join("ssh"), FAKE_SSH);
        executable(&bin.join("docker"), FAKE_DOCKER);
        executable(&bin.join("df"), FAKE_DF);
        fs::create_dir_all(repo.join("ci/images")).unwrap();
        fs::write(repo.join("ci/images/rust.Dockerfile"), DOCKERFILE).unwrap();
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
            // The tests look at the remote folder after a run; the default removes it.
            .env("ANTHOLOGY_KEEP_REMOTE", "1")
            .env("FAKE_DOCKER_ROOT", self.home.join("docker-root"))
            .env("FAKE_STATE", &self.state)
            .env("FAKE_REMOTE_HOME", &self.home)
            .stdin(Stdio::null());
        cmd
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("script ran")
    }

    fn remote_root(&self) -> PathBuf {
        self.home.join("anthology-remote")
    }

    /// The folders under `~/anthology-remote` on the fake host (not the `.stamp` files).
    fn remote_dirs(&self) -> Vec<PathBuf> {
        let Ok(entries) = fs::read_dir(self.remote_root()) else {
            return Vec::new();
        };
        let mut dirs = entries
            .map(|e| e.unwrap().path())
            .filter(|p| p.is_dir())
            .collect::<Vec<_>>();
        dirs.sort();
        dirs
    }

    /// The one checkout folder on the fake host.
    fn remote_dir(&self) -> PathBuf {
        let mut dirs = self.remote_dirs();
        assert_eq!(dirs.len(), 1, "{dirs:?}");
        dirs.remove(0)
    }

    /// What is under `~/anthology-remote` besides the `.last-used` file.
    fn left_under_root(&self) -> Vec<String> {
        let Ok(entries) = fs::read_dir(self.remote_root()) else {
            return Vec::new();
        };
        entries
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n != ".last-used" && n != ".pulled-images")
            .collect()
    }

    /// The `.last-used` file of the shared caches, last touched `hours_ago` hours ago.
    fn seed_last_used(&self, hours_ago: u32) -> PathBuf {
        fs::create_dir_all(self.remote_root()).unwrap();
        let file = self.remote_root().join(".last-used");
        fs::write(&file, "").unwrap();
        sh(&format!("touch -d '{hours_ago} hours ago' \"$0\""), &file);
        file
    }

    /// Where this checkout's folder is, or will be, on the fake host.
    fn own_remote_dir(&self) -> PathBuf {
        let sum = sh(r#"printf '%s' "$0" | cksum | cut -d' ' -f1"#, &self.repo);
        self.remote_root().join(sum)
    }

    fn log(&self, name: &str) -> String {
        fs::read_to_string(self.state.join(name)).unwrap_or_default()
    }

    fn containers(&self) -> String {
        self.log("containers")
    }

    fn volumes(&self) -> String {
        self.log("volumes")
    }

    /// A folder on the fake host with a `.stamp` file last touched `hours_ago` hours ago.
    fn seed_dir(&self, name: &str, hours_ago: u32) -> PathBuf {
        let dir = self.remote_root().join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("f.txt"), "x\n").unwrap();
        let stamp = self.remote_root().join(format!("{name}.stamp"));
        fs::write(&stamp, "").unwrap();
        sh(&format!("touch -d '{hours_ago} hours ago' \"$0\""), &stamp);
        dir
    }

    /// A volume with our label, created `hours_ago` hours ago.
    fn seed_volume(&self, name: &str, hours_ago: u32) {
        self.seed_volume_tagged(name, hours_ago, "ours");
    }

    /// A volume without our label, whatever its name.
    fn seed_foreign_volume(&self, name: &str, hours_ago: u32) {
        self.seed_volume_tagged(name, hours_ago, "-");
    }

    fn seed_volume_tagged(&self, name: &str, hours_ago: u32, tag: &str) {
        let created = sh(
            &format!("date -u -d '{hours_ago} hours ago' +%Y-%m-%dT%H:%M:%SZ"),
            Path::new("."),
        );
        self.append("volumes", &format!("{name} {created} {tag}\n"));
    }

    fn seed_container(&self, id: &str, volume: &str, status: &str) {
        self.seed_container_of(id, volume, status, "-");
    }

    fn seed_container_of(&self, id: &str, volume: &str, status: &str, image: &str) {
        self.append("containers", &format!("{id} {volume} {status} {image}\n"));
    }

    /// The Rust image is on the host, and an earlier run of ours recorded that it pulled it.
    fn seed_pulled_image(&self) {
        fs::create_dir_all(self.remote_root()).unwrap();
        fs::write(
            self.remote_root().join(".pulled-images"),
            format!("{IMAGE}\n"),
        )
        .unwrap();
        self.append("images", &format!("{IMAGE}\n"));
    }

    fn images(&self) -> String {
        self.log("images")
    }

    fn pulled_images(&self) -> String {
        fs::read_to_string(self.remote_root().join(".pulled-images")).unwrap_or_default()
    }

    fn append(&self, file: &str, line: &str) {
        let mut text = self.log(file);
        text.push_str(line);
        fs::write(self.state.join(file), text).unwrap();
    }
}

/// Runs a shell snippet with `$0` set to `arg`; its trimmed stdout.
fn sh(script: &str, arg: &Path) -> String {
    let out = Command::new("sh")
        .args(["-c", script])
        .arg(arg)
        .output()
        .unwrap();
    assert!(out.status.success(), "{script}: {}", text(&out.stderr));
    text(&out.stdout).trim().to_owned()
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
    let rig = rig!();
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
    let rig = rig!();
    let out = rig
        .command(&["lint"])
        .env("ANTHOLOGY_BUILD_HOST", "")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn a_host_that_looks_like_an_option_is_refused() {
    let rig = rig!();
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
    let rig = rig!();
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
    let rig = rig!();
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
    let rig = rig!();
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
    let rig = rig!();
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
    let rig = rig!();
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
    let rig = rig!();
    for code in ["0", "3"] {
        fs::write(rig.state.join("containers"), "").unwrap();
        rig.command(&["test"])
            .env("FAKE_EXIT", code)
            .output()
            .unwrap();
        assert_eq!(rig.containers().trim(), "", "exit {code}");
        let docker = rig.log("docker.log");
        assert!(docker.contains("rm -f cont1"), "{docker}");
        let volume = volume_for(&rig.own_remote_dir());
        assert!(docker.contains(&format!("volume={volume}")), "{docker}");
    }
}

#[test]
fn a_container_left_by_a_dropped_run_is_removed_before_the_next_one() {
    let rig = rig!();
    let volume = volume_for(&rig.own_remote_dir());
    rig.seed_container("stale1", &volume, "running");
    assert_eq!(rig.run(&["lint"]).status.code(), Some(0));
    assert!(rig.log("docker.log").contains("rm -f stale1"));
}

#[test]
fn a_dropped_ssh_link_stops_the_run_and_removes_its_containers() {
    let rig = rig!();
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
    let rig = rig!();
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
    let rig = rig!();
    let out = rig.run(&["--clean"]);
    assert_eq!(out.status.code(), Some(0), "{}", all_output(&out));
}

#[test]
fn clean_takes_no_other_arguments() {
    let rig = rig!();
    let out = rig.run(&["--clean", "lint"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(rig.log("ssh.log").is_empty(), "ssh was called");
}

#[test]
fn the_alias_never_appears_in_output() {
    let rig = rig!();
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

/// The `.stamp` file next to a checkout folder: when the folder was last used.
fn stamp_of(dir: &Path) -> PathBuf {
    PathBuf::from(format!("{}.stamp", dir.display()))
}

#[test]
fn a_run_removes_its_folder_stamp_and_volume_by_default() {
    let rig = rig!();
    for code in ["0", "5"] {
        let out = rig
            .command(&["test"])
            .env_remove("ANTHOLOGY_KEEP_REMOTE")
            .env("FAKE_EXIT", code)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), code.parse::<i32>().ok(), "exit {code}");
        let left = rig.left_under_root();
        assert!(left.is_empty(), "left under ~/anthology-remote: {left:?}");
        let volume = volume_for(&rig.own_remote_dir());
        let docker = rig.log("docker.log");
        assert!(docker.contains(&format!("volume rm {volume}")), "{docker}");
        assert_eq!(rig.containers().trim(), "");
    }
}

#[test]
fn a_dropped_link_removes_the_folder_by_default_too() {
    let rig = rig!();
    let out = rig
        .command(&["test"])
        .env_remove("ANTHOLOGY_KEEP_REMOTE")
        .env("FAKE_SSH_DROP", "1")
        .env("FAKE_HANG", "1")
        .output()
        .unwrap();
    assert_ne!(out.status.code(), Some(0));
    assert_eq!(rig.left_under_root(), Vec::<String>::new());
}

#[test]
fn keep_leaves_the_folder_stamp_and_volume_for_the_next_run() {
    let rig = rig!();
    assert_eq!(rig.run(&["lint"]).status.code(), Some(0));
    let dir = rig.remote_dir();
    assert!(dir.join("src/lib.rs").is_file());
    assert!(stamp_of(&dir).is_file(), "the stamp marks the last use");
    let docker = rig.log("docker.log");
    assert!(!docker.contains("volume rm"), "{docker}");
}

#[test]
fn the_start_of_a_run_sweeps_only_our_old_leftovers() {
    let rig = rig!();
    let old = rig.seed_dir("1111", 13);
    let recent = rig.seed_dir("2222", 1);
    let busy = rig.seed_dir("3333", 20);
    let (v_old, v_recent, v_busy) = (volume_for(&old), volume_for(&recent), volume_for(&busy));
    let orphan_old = "anthology-rust-target-9999";
    let orphan_new = "anthology-rust-target-8888";
    for (name, age) in [
        (v_old.as_str(), 20),
        (v_recent.as_str(), 20),
        (v_busy.as_str(), 20),
        (orphan_old, 13),
        (orphan_new, 2),
        ("other-project-data", 50),
        ("anthology-cargo", 50),
    ] {
        rig.seed_volume(name, age);
    }
    rig.seed_container("c-old", &v_old, "exited");
    rig.seed_container("c-busy", &v_busy, "running");
    rig.seed_container("c-recent-stopped", &v_recent, "exited");
    rig.seed_container("c-orphan-stopped", orphan_old, "exited");
    rig.seed_container("c-foreign", "other-project-data", "exited");

    let out = rig.run(&["lint"]);
    assert_eq!(out.status.code(), Some(0), "{}", all_output(&out));

    assert!(!old.exists() && !stamp_of(&old).exists(), "13 hours old");
    assert!(recent.exists() && stamp_of(&recent).exists(), "1 hour old");
    assert!(busy.exists(), "old, but a container uses its volume");
    let volumes = rig.volumes();
    for gone in [v_old.as_str(), orphan_old] {
        assert!(!volumes.contains(gone), "{gone} is still there:\n{volumes}");
    }
    for kept in [
        v_recent.as_str(),
        v_busy.as_str(),
        orphan_new,
        "other-project-data",
        "anthology-cargo",
    ] {
        assert!(volumes.contains(kept), "{kept} was removed:\n{volumes}");
    }
    let containers = rig.containers();
    for gone in ["c-old", "c-recent-stopped", "c-orphan-stopped"] {
        assert!(!containers.contains(gone), "{gone}: {containers}");
    }
    for kept in ["c-busy", "c-foreign"] {
        assert!(containers.contains(kept), "{kept}: {containers}");
    }
    let docker = rig.log("docker.log");
    for untouched in ["volume rm other-project-data", "volume rm anthology-cargo"] {
        assert!(!docker.contains(untouched), "{untouched}");
    }
}

#[test]
fn a_host_with_under_20_gb_free_refuses_to_start() {
    let rig = rig!();
    let old = rig.seed_dir("1111", 13);
    for (var, kb) in [
        ("FAKE_DF_KB", "1000"),
        ("FAKE_DF_KB", "20971519"),
        ("FAKE_DF_DOCKER_KB", "20971519"),
    ] {
        let out = rig.command(&["test"]).env(var, kb).output().unwrap();
        let shown = all_output(&out);
        assert_ne!(out.status.code(), Some(0), "{var}={kb}: {shown}");
        assert!(text(&out.stderr).contains("20 GB"), "{shown}");
        assert!(text(&out.stderr).contains("free"), "{shown}");
        assert!(
            !text(&out.stdout).contains("rust-test args"),
            "ran: {shown}"
        );
        assert!(!shown.contains(ALIAS), "{shown}");
    }
    assert!(!old.exists(), "the sweep runs first, to make room");
    assert!(
        rig.remote_dirs().is_empty(),
        "nothing was copied: {:?}",
        rig.remote_dirs()
    );
}

#[test]
fn exactly_20_gb_free_is_enough() {
    let rig = rig!();
    let out = rig
        .command(&["lint"])
        .env("FAKE_DF_KB", "20971520")
        .env("FAKE_DF_DOCKER_KB", "20971520")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", all_output(&out));
}

#[test]
fn clean_ours_removes_what_we_created_and_the_image_we_pulled() {
    let rig = rig!();
    rig.seed_dir("1111", 13);
    rig.seed_dir("2222", 1);
    for name in [
        "anthology-rust-target-1",
        "anthology-rust-target-2",
        "anthology-cargo",
        "anthology-rustup-abc",
    ] {
        rig.seed_volume(name, 1);
    }
    // A volume with our label but not our name, and ours by name but without the label.
    rig.seed_volume("labelled-but-not-ours", 1);
    rig.seed_foreign_volume("anthology-rust-target-3", 1);
    rig.seed_container("c1", "anthology-rust-target-1", "running");
    rig.seed_container("c2", "labelled-but-not-ours", "running");
    // Our run pulls the image: the first run records it.
    assert_eq!(rig.run(&["lint"]).status.code(), Some(0));
    assert!(
        rig.pulled_images().contains(IMAGE),
        "{}",
        rig.pulled_images()
    );

    let out = rig.run(&["--clean-ours"]);
    assert_eq!(out.status.code(), Some(0), "{}", all_output(&out));
    assert!(!rig.remote_root().exists(), "~/anthology-remote is gone");
    let volumes = rig.volumes();
    let left: Vec<&str> = volumes
        .lines()
        .filter_map(|l| l.split(' ').next())
        .collect();
    assert_eq!(
        left,
        ["labelled-but-not-ours", "anthology-rust-target-3"],
        "{volumes}"
    );
    assert!(!rig.containers().contains("c1 "), "{}", rig.containers());
    assert!(rig.containers().contains("c2 "), "{}", rig.containers());
    let docker = rig.log("docker.log");
    assert!(docker.contains(&format!("rmi {IMAGE}")), "{docker}");
    assert!(!rig.images().contains(IMAGE), "{}", rig.images());
    assert!(!all_output(&out).contains(ALIAS));
}

#[test]
fn clean_ours_before_any_run_succeeds_and_takes_no_other_arguments() {
    let rig = rig!();
    assert_eq!(rig.run(&["--clean-ours"]).status.code(), Some(0));
    let out = rig.run(&["--clean-ours", "lint"]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn the_old_name_clean_all_is_refused_not_passed_on() {
    let rig = rig!();
    let out = rig.run(&["--clean-all"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("--clean-ours"));
    assert!(rig.log("ssh.log").is_empty(), "ssh was called");
}

/// Our caches, 100 hours old, an unrelated volume, and the Rust image as we pulled it.
fn seed_caches(rig: &Rig) {
    for name in ["anthology-cargo", "anthology-rustup-abc"] {
        rig.seed_volume(name, 100);
    }
    rig.seed_foreign_volume("other-project-data", 100);
    rig.seed_pulled_image();
}

#[test]
fn caches_unused_for_48_hours_are_swept_with_the_image() {
    let rig = rig!();
    seed_caches(&rig);
    let last_used = rig.seed_last_used(49);
    let out = rig.run(&["lint"]);
    assert_eq!(out.status.code(), Some(0), "{}", all_output(&out));
    let docker = rig.log("docker.log");
    // The run then creates what it needs again, so the proof is in the calls.
    assert!(docker.contains("volume rm anthology-cargo"), "{docker}");
    assert!(
        docker.contains("volume rm anthology-rustup-abc"),
        "{docker}"
    );
    assert!(docker.contains(&format!("rmi {IMAGE}")), "{docker}");
    assert!(!docker.contains("volume rm other-project-data"), "{docker}");
    assert!(rig.volumes().contains("other-project-data"));
    assert!(
        text(&out.stdout).contains("last-used is fresh"),
        "the file is written at the start: {}",
        all_output(&out)
    );
    assert!(!sh("find \"$0\" -mmin -5", &last_used).is_empty());
}

#[test]
fn caches_used_within_48_hours_or_with_no_record_are_kept() {
    for hours in [Some(47), None] {
        let rig = rig!();
        seed_caches(&rig);
        if let Some(h) = hours {
            rig.seed_last_used(h);
        }
        let out = rig.run(&["lint"]);
        assert_eq!(out.status.code(), Some(0), "{hours:?}");
        let volumes = rig.volumes();
        assert!(volumes.contains("anthology-cargo"), "{hours:?}: {volumes}");
        assert!(volumes.contains("anthology-rustup-abc"), "{hours:?}");
        let docker = rig.log("docker.log");
        assert!(!docker.contains("rmi"), "{hours:?}: {docker}");
        assert!(
            rig.remote_root().join(".last-used").is_file(),
            "{hours:?}: the clock starts"
        );
    }
}

#[test]
fn a_cache_a_running_container_uses_is_kept() {
    let rig = rig!();
    seed_caches(&rig);
    rig.seed_last_used(49);
    rig.seed_container("busy", "anthology-cargo", "running");
    assert_eq!(rig.run(&["lint"]).status.code(), Some(0));
    assert!(rig.volumes().contains("anthology-cargo"));
    assert!(rig.containers().contains("busy"));
}

#[test]
fn the_end_of_a_run_writes_last_used_even_when_the_folder_is_removed() {
    let rig = rig!();
    let last_used = rig.seed_last_used(10);
    // The stand-in run ages the file, so only a write at the end can make it fresh again.
    let out = rig
        .command(&["lint"])
        .env_remove("ANTHOLOGY_KEEP_REMOTE")
        .env("FAKE_AGE_LAST_USED", "1")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", all_output(&out));
    assert!(rig.left_under_root().is_empty());
    assert!(!sh("find \"$0\" -mmin -5", &last_used).is_empty());
}

#[test]
fn what_we_create_carries_our_label() {
    let rig = rig!();
    assert_eq!(rig.run(&["lint"]).status.code(), Some(0));
    let volumes = rig.volumes();
    let names = [
        volume_for(&rig.own_remote_dir()),
        "anthology-cargo".to_owned(),
        "anthology-rustup-0123456789ab".to_owned(),
    ];
    for name in names {
        let line = volumes.lines().find(|l| l.starts_with(&format!("{name} ")));
        let line = line.unwrap_or_else(|| panic!("no volume {name}:\n{volumes}"));
        assert!(line.ends_with(" ours"), "{name} has no label: {line}");
    }
    let docker = rig.log("docker.log");
    assert!(docker.contains("volume create --label anthology.remote-build=1"));
}

#[test]
fn nothing_unlabelled_or_unrelated_is_ever_touched() {
    let rig = rig!();
    // Same names as ours but no label, and things with no connection to us.
    for name in [
        "anthology-cargo",
        "anthology-rustup-zzz",
        "anthology-rust-target-777",
        "unrelated-data",
    ] {
        rig.seed_foreign_volume(name, 100);
    }
    rig.seed_volume("labelled-other", 100);
    // The Rust image was on the host already (no record that we pulled it), beside another one.
    rig.append("images", &format!("{IMAGE}\nrust:unrelated\n"));
    rig.seed_container_of("u1", "unrelated-data", "running", "rust:unrelated");
    rig.seed_container_of("u2", "anthology-cargo", "exited", "rust:unrelated");
    rig.seed_container("u3", "anthology-rust-target-777", "exited");
    rig.seed_last_used(100);
    rig.seed_dir("1111", 100);
    let outside = rig.home.join("not-ours");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("file"), "x").unwrap();

    for args in [&["lint"][..], &["--clean-ours"]] {
        let out = rig
            .command(args)
            .env_remove("ANTHOLOGY_KEEP_REMOTE")
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(0), "{args:?}: {}", all_output(&out));
    }

    let volumes = rig.volumes();
    for kept in [
        "anthology-cargo ",
        "anthology-rustup-zzz ",
        "anthology-rust-target-777 ",
        "unrelated-data ",
        "labelled-other ",
    ] {
        assert!(volumes.contains(kept), "{kept}was removed:\n{volumes}");
    }
    let images = rig.images();
    assert!(images.contains(IMAGE), "an image we did not pull: {images}");
    assert!(images.contains("rust:unrelated"), "{images}");
    let containers = rig.containers();
    for kept in ["u1 ", "u2 ", "u3 "] {
        assert!(containers.contains(kept), "{kept}was removed: {containers}");
    }
    assert!(outside.join("file").is_file(), "outside ~/anthology-remote");
    let docker = rig.log("docker.log");
    for never in ["rmi", "prune", "system", "rm -f u", "rm -f --"] {
        assert!(
            !docker.contains(never),
            "docker was called with {never}:\n{docker}"
        );
    }
    for never in [
        "volume rm anthology-cargo",
        "volume rm anthology-rustup-zzz",
        "volume rm anthology-rust-target-777",
        "volume rm unrelated-data",
        "volume rm labelled-other",
    ] {
        assert!(!docker.contains(never), "{never}:\n{docker}");
    }
}

#[test]
fn an_image_we_pulled_stays_while_a_container_that_is_not_ours_uses_it() {
    let rig = rig!();
    rig.seed_pulled_image();
    rig.seed_container_of("theirs", "their-data", "running", IMAGE);
    let out = rig.run(&["--clean-ours"]);
    assert_eq!(out.status.code(), Some(0), "{}", all_output(&out));
    assert!(rig.images().contains(IMAGE), "{}", rig.images());
    assert!(!rig.log("docker.log").contains("rmi"));
}

#[test]
fn an_image_that_was_already_there_is_not_recorded_as_pulled() {
    let rig = rig!();
    rig.append("images", &format!("{IMAGE}\n"));
    assert_eq!(rig.run(&["lint"]).status.code(), Some(0));
    assert!(!rig.pulled_images().contains(IMAGE));
}

#[test]
fn the_script_never_prunes_or_forces_a_removal() {
    let script = repo::read(SCRIPT);
    for word in [
        "prune",
        "docker system",
        "rmi -f",
        "rmi --force",
        "volume rm -f",
    ] {
        assert!(!script.contains(word), "{SCRIPT} mentions {word}");
    }
}
