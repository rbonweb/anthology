//! `scripts/remote-build.sh`: run the Docker-based Rust checks on a remote Linux build host over
//! ssh. A fake `ssh` on PATH records its argv and runs the command locally with a temp dir as the
//! remote home; a fake `docker` records its argv; the temp repo's `scripts/rust-test.sh` is a
//! stand-in that prints its arguments and "starts" a container. No host, no Docker, no network.

use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
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
# Runs at once from several runs: one call at a time, as a docker daemon orders them.
exec 9>"$FAKE_STATE/.lock"
flock 9
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
    run)
        # The size read of a cache volume: `run --rm -v VOL:/v:ro --entrypoint du IMAGE -sk /v`.
        # `sizes` holds `<volume> <KB>` lines.
        for a in "$@"; do
            case $a in *:/v:ro) v=${a%%:*} ;; esac
        done
        awk -v v="$v" '$1 == v { printf "%s\t/v\n", $2 }' "$FAKE_STATE/sizes" 2>/dev/null
        ;;
    system | prune | *prune) echo "fake docker: $1 refused" >&2; exit 1 ;;
    info) printf '%s\n' "$FAKE_DOCKER_ROOT" ;;
esac
"#;

/// `df -Pk PATH`: the free space is `FAKE_DF_KB`, or `FAKE_DF_DOCKER_KB` for the Docker data dir.
/// The disk is `FAKE_DF_USE` percent full (1 when unset), and every volume or image the script has
/// removed since (in docker.log) frees 10 points, so a trim stops once it has made enough room.
const FAKE_DF: &str = r#"#!/bin/sh
for last; do :; done
kb=${FAKE_DF_KB:-999999999}
[ "$last" != "$FAKE_DOCKER_ROOT" ] || kb=${FAKE_DF_DOCKER_KB:-$kb}
removed=$(grep -c '^volume rm\|^rmi' "$FAKE_STATE/docker.log" 2>/dev/null || true)
use=$(( ${FAKE_DF_USE:-1} - ${removed:-0} * 10 ))
[ "$use" -ge 1 ] || use=1
echo "Filesystem 1024-blocks Used Available Capacity Mounted on"
echo "fake 1000000000 1 $kb ${use}% /"
"#;

/// Starts a container on the volume the real runner would name: the cksum of its root path.
const FAKE_RUST_TEST: &str = r#"#!/bin/sh
echo "rust-test args: $*"
# The script's toolchain install is `run true`: one line per call in `installs`, its start and end
# in `install-spans`, and a failure when `install-fail` exists (once).
if [ "$1" = run ]; then
    echo "install" >> "$FAKE_STATE/installs"
    echo "start $$ $(date +%s%N)" >> "$FAKE_STATE/install-spans"
    sleep "${FAKE_INSTALL_SECS:-0}"
    echo "end $$ $(date +%s%N)" >> "$FAKE_STATE/install-spans"
    [ ! -e "$FAKE_STATE/install-fail-always" ] || exit 1
    if [ -e "$FAKE_STATE/install-fail" ]; then rm -f "$FAKE_STATE/install-fail"; exit 1; fi
    exit 0
fi
echo "rust-test cwd: $(pwd)"
echo "keep-target=${ANTHOLOGY_KEEP_TARGET:-unset}"
echo "jobs=${CARGO_BUILD_JOBS:-unset}"
root=$(cd "$(dirname "$0")/.." && pwd)
vol=anthology-rust-target-$(printf '%s' "$root" | cksum | cut -d' ' -f1)
image=$(awk '$1 == "FROM" {print $NF; exit}' "$root/ci/images/rust.Dockerfile")
rustup=anthology-rustup-$(printf '%s' "${image##*@sha256:}" | cut -c1-12)
# `docker run` pulls a missing image and creates a missing volume, with no label. Runs at once
# share these files: one at a time.
exec 8>"$FAKE_STATE/.lock"
flock 8
touch "$FAKE_STATE/images" "$FAKE_STATE/volumes"
grep -qxF -- "$image" "$FAKE_STATE/images" || echo "$image" >> "$FAKE_STATE/images"
for v in "$vol" anthology-cargo anthology-sccache "$rustup"; do
    grep -q "^$v " "$FAKE_STATE/volumes" || echo "$v $(date -u +%Y-%m-%dT%H:%M:%SZ) -" >> "$FAKE_STATE/volumes"
done
echo "cont1 $vol running $image" >> "$FAKE_STATE/containers"
flock -u 8
echo "start $$ $(date +%s%N)" >> "$FAKE_STATE/spans"
[ -z "${FAKE_HANG:-}" ] || exec sleep 30
sleep "${FAKE_SLEEP:-0}"
echo "end $$ $(date +%s%N)" >> "$FAKE_STATE/spans"
exit "${FAKE_EXIT:-0}"
"#;

const DOCKERFILE: &str = "FROM rust@sha256:0123456789abcdef\n";
const IMAGE: &str = "rust@sha256:0123456789abcdef";

/// The fake host is this machine, so the remote half needs GNU tools (`stat -c`, `date -d`, `find
/// -printf`, `sort -z`) and `flock`: the Linux test container has them, macOS does not. Skip there.
fn gnu_userland() -> bool {
    let ok = |cmd: &str| {
        Command::new("sh")
            .args(["-c", cmd])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };
    ok(
        "date -d '1 hour ago' +%s && stat -c %Y . && find . -maxdepth 0 -printf x && printf 'a\\0' | sort -z && command -v flock",
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
            // The fake host is this machine, whatever its size: fix the caps.
            .env("ANTHOLOGY_REMOTE_SLOTS", "4")
            .env("ANTHOLOGY_REMOTE_JOBS", "2")
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

    /// The folders under `~/anthology-remote` on the fake host (not the `.stamp` and `.lock` files,
    /// not the hidden `.slots`).
    fn remote_dirs(&self) -> Vec<PathBuf> {
        let Ok(entries) = fs::read_dir(self.remote_root()) else {
            return Vec::new();
        };
        let mut dirs = entries
            .map(|e| e.unwrap().path())
            .filter(|p| p.is_dir() && !p.file_name().unwrap().to_string_lossy().starts_with('.'))
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

    /// What is under `~/anthology-remote` besides the hidden bookkeeping files (the locks, the
    /// build slots, the toolchain marker, the use records, the record of the pulled image).
    fn left_under_root(&self) -> Vec<String> {
        let Ok(entries) = fs::read_dir(self.remote_root()) else {
            return Vec::new();
        };
        entries
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| !n.starts_with('.'))
            .collect()
    }

    /// The record that a run used the kept item `name` (a volume, or `image_key()`), `hours_ago`
    /// hours ago: the trim under disk pressure removes the oldest first.
    fn seed_used(&self, name: &str, hours_ago: u32) -> PathBuf {
        let dir = self.remote_root().join(".used");
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join(name);
        fs::write(&file, "").unwrap();
        sh(&format!("touch -d '{hours_ago} hours ago' \"$0\""), &file);
        file
    }

    /// The size of a cache volume as the throwaway `du` container reports it, in KB.
    fn seed_size(&self, volume: &str, kb: u64) {
        self.append("sizes", &format!("{volume} {kb}\n"));
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

    /// The runs the stand-in `rust-test.sh` recorded in `file` (`spans` or `install-spans`): its
    /// start and, once it is over, its end, in nanoseconds.
    fn spans(&self, file: &str) -> Vec<(u128, Option<u128>)> {
        let mut by_pid: Vec<(String, u128, Option<u128>)> = Vec::new();
        for line in self.log(file).lines() {
            let mut words = line.split_whitespace();
            let (Some(kind), Some(pid), Some(ns)) = (words.next(), words.next(), words.next())
            else {
                continue;
            };
            let ns: u128 = ns.parse().unwrap();
            match kind {
                "start" => by_pid.push((pid.to_owned(), ns, None)),
                "end" => {
                    if let Some(span) = by_pid.iter_mut().find(|s| s.0 == pid) {
                        span.2 = Some(ns);
                    }
                }
                _ => {}
            }
        }
        by_pid.into_iter().map(|(_, s, e)| (s, e)).collect()
    }

    /// Starts the script in the background with its output in `<tag>.out` under the state folder.
    /// The run is not kept (the default of the script) unless `envs` says so.
    fn spawn_run(&self, tag: &str, envs: &[(&str, &str)], args: &[&str]) -> Child {
        let out = fs::File::create(self.state.join(format!("{tag}.out"))).unwrap();
        let err = out.try_clone().unwrap();
        let mut cmd = self.command(args);
        cmd.env_remove("ANTHOLOGY_KEEP_REMOTE");
        for (key, value) in envs {
            cmd.env(key, value);
        }
        cmd.stdout(out).stderr(err).spawn().expect("script started")
    }

    /// What a background run printed.
    fn output_of(&self, tag: &str) -> String {
        self.log(&format!("{tag}.out"))
    }

    /// Waits for a background run to end; its exit code.
    fn finish(&self, tag: &str, mut child: Child) -> Option<i32> {
        let status = child.wait().unwrap();
        eprintln!("{tag}: {}", self.output_of(tag));
        status.code()
    }
}

/// Polls `ok` for up to `secs` seconds.
fn wait_until(what: &str, secs: u64, mut ok: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(secs);
    while !ok() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(100));
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
        // The folder is this run's own, so its volume has a name of its own.
        let docker = rig.log("docker.log");
        assert!(
            docker.contains("volume rm anthology-rust-target-"),
            "{docker}"
        );
        assert!(
            !rig.volumes().contains("anthology-rust-target-"),
            "{}",
            rig.volumes()
        );
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
        let out = rig
            .command(&["test"])
            .env_remove("ANTHOLOGY_KEEP_REMOTE")
            .env(var, kb)
            .output()
            .unwrap();
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
fn clean_all_ours_removes_what_we_created_and_the_image_we_pulled() {
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

    let out = rig.run(&["--clean-all-ours", "--with-caches"]);
    assert_eq!(out.status.code(), Some(0), "{}", all_output(&out));
    let left: Vec<String> = fs::read_dir(rig.remote_root())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    // The runs lock stays: a run that is starting may already have it open.
    assert_eq!(left, [".runs.lock"], "~/anthology-remote is empty");
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
fn the_clean_commands_before_any_run_succeed_and_take_no_other_arguments() {
    let rig = rig!();
    for flag in ["--clean-ours", "--clean-all-ours"] {
        assert_eq!(rig.run(&[flag]).status.code(), Some(0), "{flag}");
        let out = rig.run(&[flag, "lint"]);
        assert_eq!(out.status.code(), Some(2), "{flag}");
    }
}

#[test]
fn the_old_name_clean_all_is_refused_not_passed_on() {
    let rig = rig!();
    let out = rig.run(&["--clean-all"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("--clean-all-ours"));
    assert!(rig.log("ssh.log").is_empty(), "ssh was called");
}

/// Our caches, 100 hours old, an unrelated volume, and the Rust image as we pulled it.
fn seed_caches(rig: &Rig) {
    for name in [
        "anthology-cargo",
        "anthology-sccache",
        "anthology-rustup-abc",
    ] {
        rig.seed_volume(name, 100);
    }
    rig.seed_foreign_volume("other-project-data", 100);
    rig.seed_pulled_image();
}

/// The name of the file under `.used` that records a use of `image`.
fn image_key(image: &str) -> String {
    format!(
        "image-{}",
        sh(r#"printf '%s' "$0" | cksum | cut -d' ' -f1"#, Path::new(image))
    )
}

/// The cache volumes and the image, the way the pressure tests seed them: `anthology-cargo` used
/// 5 days ago, the image 3 days ago, the toolchain 2 days ago and `anthology-sccache` 1 day ago.
fn seed_by_use(rig: &Rig) {
    seed_caches(rig);
    rig.seed_used("anthology-cargo", 120);
    rig.seed_used(&image_key(IMAGE), 72);
    rig.seed_used("anthology-rustup-abc", 48);
    rig.seed_used("anthology-sccache", 24);
}

/// The kept items the script removed so far, in order: cache volume names and `image`. The run
/// volumes (`anthology-rust-target-*`) are not among them: they are the run's own.
fn trimmed(rig: &Rig) -> Vec<String> {
    rig.log("docker.log")
        .lines()
        .filter_map(|l| match l.strip_prefix("volume rm ") {
            Some(v) if v.starts_with("anthology-rust-target-") => None,
            Some(v) => Some(v.to_owned()),
            None => l.starts_with("rmi ").then(|| "image".to_owned()),
        })
        .collect()
}

#[test]
fn a_run_and_clean_ours_keep_the_image_and_the_caches_however_old() {
    let rig = rig!();
    seed_caches(&rig);
    for volume in [
        "anthology-cargo",
        "anthology-sccache",
        "anthology-rustup-abc",
    ] {
        rig.seed_used(volume, 5000);
    }
    // A normal run, kept and not kept, and a clean-ours, in any order.
    for args in [
        &["lint"][..],
        &["--clean-ours"],
        &["test", "-p", "x"],
        &["--clean"],
    ] {
        let out = rig
            .command(args)
            .env_remove("ANTHOLOGY_KEEP_REMOTE")
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(0), "{args:?}: {}", all_output(&out));
        assert_eq!(trimmed(&rig), Vec::<String>::new(), "{args:?}");
    }
    let volumes = rig.volumes();
    for kept in ["anthology-cargo ", "anthology-sccache ", "anthology-rustup-abc "] {
        assert!(volumes.contains(kept), "{kept}was removed:\n{volumes}");
    }
    assert!(rig.images().contains(IMAGE), "{}", rig.images());
    assert!(rig.pulled_images().contains(IMAGE), "the record stays");
    // What the runs themselves made is gone: only the shared caches remain.
    assert!(!volumes.contains("anthology-rust-target-"), "{volumes}");
}

#[test]
fn a_run_records_what_it_used_for_the_trim_order() {
    let rig = rig!();
    assert_eq!(rig.run(&["lint"]).status.code(), Some(0));
    let used = rig.remote_root().join(".used");
    for name in [
        "anthology-cargo",
        "anthology-sccache",
        "anthology-rustup-0123456789ab",
        &image_key(IMAGE),
    ] {
        assert!(
            !sh("find \"$0\" -mmin -5", &used.join(name)).is_empty(),
            "{name}"
        );
    }
}

#[test]
fn a_disk_over_85_percent_trims_the_least_recently_used_first_and_stops_when_it_has_room() {
    let rig = rig!();
    seed_by_use(&rig);
    // 90% full; each removal frees 10 points in the stand-in df, so one removal is enough.
    let out = rig
        .command(&["lint"])
        .env("FAKE_DF_USE", "90")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", all_output(&out));
    assert!(
        text(&out.stderr).contains("over 85%"),
        "{}",
        all_output(&out)
    );
    // The cargo registry was used longest ago: it alone goes.
    assert_eq!(trimmed(&rig), ["anthology-cargo"]);
    // 115% (a stand-in): the next two go, in the order of their last use, and then the disk has
    // room, so the most recently used cache stays.
    let rig = rig!();
    seed_by_use(&rig);
    let out = rig
        .command(&["lint"])
        .env("FAKE_DF_USE", "115")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", all_output(&out));
    assert_eq!(
        trimmed(&rig),
        ["anthology-cargo", "image", "anthology-rustup-abc"]
    );
}

#[test]
fn a_disk_at_or_under_85_percent_trims_nothing() {
    let rig = rig!();
    seed_by_use(&rig);
    for use_pct in ["85", "40"] {
        let out = rig
            .command(&["lint"])
            .env("FAKE_DF_USE", use_pct)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(0), "{}", all_output(&out));
        assert_eq!(trimmed(&rig), Vec::<String>::new(), "{use_pct}");
    }
}

#[test]
fn the_trim_takes_only_our_labelled_volumes_and_the_image_we_pulled() {
    let rig = rig!();
    seed_by_use(&rig);
    // Names like ours without our label, and an unrelated volume.
    for name in ["anthology-rustup-zzz", "other-project-data"] {
        rig.seed_foreign_volume(name, 9000);
    }
    let out = rig
        .command(&["lint"])
        .env("FAKE_DF_USE", "300")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", all_output(&out));
    let volumes = rig.volumes();
    for kept in ["anthology-rustup-zzz ", "other-project-data "] {
        assert!(volumes.contains(kept), "{kept}was removed:\n{volumes}");
    }
    // Everything of ours that was a candidate went, nothing else.
    assert_eq!(
        trimmed(&rig)[..4],
        ["anthology-cargo", "image", "anthology-rustup-abc", "anthology-sccache"]
    );
    // An image that was on the host before us (no record that we pulled it) is never a candidate.
    let rig = rig!();
    seed_by_use(&rig);
    fs::remove_file(rig.remote_root().join(".pulled-images")).unwrap();
    let out = rig
        .command(&["lint"])
        .env("FAKE_DF_USE", "300")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", all_output(&out));
    assert!(!trimmed(&rig).contains(&"image".to_owned()), "{:?}", trimmed(&rig));
    assert!(rig.images().contains(IMAGE), "{}", rig.images());
    assert!(!rig.log("docker.log").contains("rmi"));
}

#[test]
fn a_cache_a_running_container_uses_is_kept_under_pressure() {
    let rig = rig!();
    seed_by_use(&rig);
    rig.seed_container("busy", "anthology-cargo", "running");
    let out = rig
        .command(&["lint"])
        .env("FAKE_DF_USE", "90")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", all_output(&out));
    // The oldest is in use, so the next oldest went instead.
    assert_eq!(trimmed(&rig), ["image"]);
    assert!(rig.containers().contains("busy"));
}

#[test]
fn nothing_is_trimmed_while_another_run_is_live() {
    let rig = rig!();
    seed_by_use(&rig);
    let first = rig.spawn_run("a", &[("FAKE_SLEEP", "6")], &["lint"]);
    wait_until("the build to start", 60, || !rig.spans("spans").is_empty());
    // This run starts, and ends, with the first one holding the runs lock: it must not trim.
    let out = rig
        .command(&["lint"])
        .env("FAKE_DF_USE", "300")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", all_output(&out));
    assert_eq!(trimmed(&rig), Vec::<String>::new());
    assert_eq!(rig.finish("a", first), Some(0));
}

#[test]
fn a_cache_volume_over_40_gb_is_removed_once_an_hour() {
    let rig = rig!();
    seed_by_use(&rig);
    let gb = 1024 * 1024;
    rig.seed_size("anthology-cargo", 41 * gb);
    rig.seed_size("anthology-sccache", 40 * gb);
    rig.seed_size("anthology-rustup-abc", 3 * gb);
    let out = rig.run(&["lint"]);
    assert_eq!(out.status.code(), Some(0), "{}", all_output(&out));
    assert!(
        text(&out.stderr).contains("over 40 GB"),
        "{}",
        all_output(&out)
    );
    // Only the one over the cap goes (40 GB exactly is within it); no disk is short.
    assert_eq!(trimmed(&rig), ["anthology-cargo"]);
    let size_reads = |rig: &Rig| {
        let docker = rig.log("docker.log");
        docker.lines().filter(|l| l.contains("--entrypoint du")).count()
    };
    assert_eq!(size_reads(&rig), 3, "{}", rig.log("docker.log"));
    // A second run within the hour does not read the sizes again.
    assert_eq!(rig.run(&["lint"]).status.code(), Some(0));
    assert_eq!(size_reads(&rig), 3, "{}", rig.log("docker.log"));
}

#[test]
fn clean_all_ours_keeps_the_caches_and_the_image_unless_asked_with_caches() {
    let rig = rig!();
    seed_caches(&rig);
    rig.seed_used("anthology-cargo", 1);
    rig.seed_dir("1111", 1);
    rig.seed_volume("anthology-rust-target-1", 1);
    rig.seed_container("c1", "anthology-rust-target-1", "running");
    let out = rig.run(&["--clean-all-ours"]);
    assert_eq!(out.status.code(), Some(0), "{}", all_output(&out));
    assert!(text(&out.stderr).contains("caches and image kept"));
    // The run folder and its volume and containers went; the caches and the image did not.
    assert!(rig.remote_dirs().is_empty());
    assert!(!rig.volumes().contains("anthology-rust-target-1"));
    assert!(!rig.containers().contains("c1 "));
    assert_eq!(trimmed(&rig), Vec::<String>::new());
    assert!(rig.images().contains(IMAGE));
    assert!(rig.pulled_images().contains(IMAGE));
    assert!(rig.remote_root().join(".used/anthology-cargo").is_file());

    // With the flag it is the full wipe.
    let out = rig.run(&["--clean-all-ours", "--with-caches"]);
    assert_eq!(out.status.code(), Some(0), "{}", all_output(&out));
    assert_eq!(
        trimmed(&rig),
        [
            "anthology-cargo",
            "anthology-sccache",
            "anthology-rustup-abc",
            "image"
        ]
    );
    assert!(!rig.images().contains(IMAGE), "{}", rig.images());
    assert!(rig.volumes().contains("other-project-data"));
}

#[test]
fn with_caches_belongs_to_clean_all_ours_alone() {
    let rig = rig!();
    for args in [
        &["--clean-ours", "--with-caches"][..],
        &["--clean", "--with-caches"],
        &["--clean-all-ours", "--bogus"],
        &["--clean-all-ours", "--with-caches", "x"],
    ] {
        let out = rig.run(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
    }
    assert!(rig.log("ssh.log").is_empty(), "ssh was called");
}

#[test]
fn what_we_create_carries_our_label() {
    let rig = rig!();
    assert_eq!(rig.run(&["lint"]).status.code(), Some(0));
    let volumes = rig.volumes();
    let names = [
        volume_for(&rig.own_remote_dir()),
        "anthology-cargo".to_owned(),
        "anthology-sccache".to_owned(),
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
    rig.seed_dir("1111", 100);
    let outside = rig.home.join("not-ours");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("file"), "x").unwrap();

    for args in [
        &["lint"][..],
        &["--clean-ours"],
        &["--clean-all-ours"],
        &["--clean-all-ours", "--with-caches"],
    ] {
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
    let out = rig.run(&["--clean-all-ours", "--with-caches"]);
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

#[test]
fn the_runner_keeps_its_target_volume_only_when_the_run_is_kept() {
    let rig = rig!();
    let kept = rig.run(&["lint"]);
    assert!(
        text(&kept.stdout).contains("keep-target=1"),
        "{}",
        all_output(&kept)
    );
    let dropped = rig
        .command(&["lint"])
        .env_remove("ANTHOLOGY_KEEP_REMOTE")
        .output()
        .unwrap();
    assert!(
        text(&dropped.stdout).contains("keep-target=0"),
        "{}",
        all_output(&dropped)
    );
}

/// The name of this checkout's folder (the hash of its path), as the script names it.
fn own_hash(rig: &Rig) -> String {
    rig.own_remote_dir()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

/// A folder name for a run, as the script makes them: the checkout's hash and a suffix.
fn seed_run_dir(rig: &Rig, name: &str, hours_ago: u32) -> PathBuf {
    let dir = rig.seed_dir(name, hours_ago);
    rig.seed_volume(&volume_for(&dir), 1);
    dir
}

fn lock_of(dir: &Path) -> PathBuf {
    PathBuf::from(format!("{}.lock", dir.display()))
}

#[test]
fn runs_at_once_get_a_folder_and_volume_each_and_clean_ours_leaves_a_live_one() {
    let rig = rig!();
    let envs = [("FAKE_SLEEP", "8")];
    let a = rig.spawn_run("a", &envs, &["test", "-p", "x"]);
    let b = rig.spawn_run("b", &envs, &["test", "-p", "x"]);
    wait_until("both builds to start", 60, || rig.spans("spans").len() == 2);

    // Each run has a folder and a target volume of its own, named for this checkout.
    let dirs = rig.remote_dirs();
    assert_eq!(dirs.len(), 2, "{dirs:?}");
    let hash = own_hash(&rig);
    for dir in &dirs {
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with(&format!("{hash}-")), "{name}");
    }
    let volumes: Vec<String> = dirs.iter().map(|d| volume_for(d)).collect();
    assert_ne!(volumes[0], volumes[1]);

    // Another agent cleans this checkout while both build: both runs and their volumes survive.
    let out = rig.run(&["--clean-ours"]);
    assert_eq!(out.status.code(), Some(0), "{}", all_output(&out));
    assert!(text(&out.stderr).contains("alone"), "{}", all_output(&out));
    assert_eq!(rig.remote_dirs(), dirs, "a live run's folder was removed");
    let have = rig.volumes();
    for volume in &volumes {
        assert!(have.contains(volume), "{volume} was removed:\n{have}");
    }
    assert!(rig.containers().contains("cont1"), "{}", rig.containers());

    assert_eq!(rig.finish("a", a), Some(0));
    assert_eq!(rig.finish("b", b), Some(0));
    // Each removed only its own: nothing is left, and each build ran to its end.
    assert_eq!(rig.left_under_root(), Vec::<String>::new());
    assert!(
        !rig.volumes().contains("anthology-rust-target-"),
        "{}",
        rig.volumes()
    );
    assert_eq!(rig.containers().trim(), "");
    assert!(rig.spans("spans").iter().all(|s| s.1.is_some()));
}

#[test]
fn clean_all_ours_refuses_while_a_run_is_live_and_works_after() {
    let rig = rig!();
    let run = rig.spawn_run("a", &[("FAKE_SLEEP", "6")], &["lint"]);
    wait_until("the build to start", 60, || !rig.spans("spans").is_empty());
    let out = rig.run(&["--clean-all-ours"]);
    assert_eq!(out.status.code(), Some(4), "{}", all_output(&out));
    assert!(
        text(&out.stderr).contains("refusing"),
        "{}",
        all_output(&out)
    );
    assert_eq!(rig.remote_dirs().len(), 1, "the live run's folder is gone");
    assert!(
        rig.volumes().contains("anthology-cargo"),
        "{}",
        rig.volumes()
    );
    assert!(!rig.log("docker.log").contains("volume rm"));
    assert_eq!(rig.finish("a", run), Some(0));

    let out = rig.run(&["--clean-all-ours", "--with-caches"]);
    assert_eq!(out.status.code(), Some(0), "{}", all_output(&out));
    assert!(!rig.volumes().contains("anthology-cargo"));
}

#[test]
fn clean_ours_removes_only_this_checkouts_folders_and_volumes() {
    let rig = rig!();
    let hash = own_hash(&rig);
    let mine = [
        rig.seed_dir(&hash, 1),
        seed_run_dir(&rig, &format!("{hash}-1-1-1"), 1),
    ];
    rig.seed_volume(&volume_for(&mine[0]), 1);
    // Another checkout, one of whose hash starts with ours, and shared caches.
    let theirs = [
        seed_run_dir(&rig, "7777-1-1-1", 1),
        seed_run_dir(&rig, &format!("{hash}9-1-1-1"), 1),
    ];
    for cache in ["anthology-cargo", "anthology-rustup-abc"] {
        rig.seed_volume(cache, 1);
    }
    rig.seed_pulled_image();

    let out = rig.run(&["--clean-ours"]);
    assert_eq!(out.status.code(), Some(0), "{}", all_output(&out));
    let volumes = rig.volumes();
    for dir in &mine {
        assert!(!dir.exists(), "{dir:?}");
        assert!(!lock_of(dir).exists() && !stamp_of(dir).exists());
        assert!(!volumes.contains(&volume_for(dir)), "{volumes}");
    }
    for dir in &theirs {
        assert!(dir.exists(), "{dir:?} belongs to another checkout");
        assert!(volumes.contains(&volume_for(dir)), "{volumes}");
    }
    for cache in ["anthology-cargo", "anthology-rustup-abc"] {
        assert!(volumes.contains(cache), "{volumes}");
    }
    assert!(rig.images().contains(IMAGE));
}

#[test]
fn a_dead_runs_leftovers_are_reclaimed_at_once_and_a_live_runs_are_not() {
    let rig = rig!();
    // Both folders were used a minute ago; only the lock tells a dead run from a live one.
    let dead = seed_run_dir(&rig, "4444-1-1-1", 0);
    let live = seed_run_dir(&rig, "5555-2-2-2", 0);
    fs::write(lock_of(&dead), "").unwrap();
    fs::write(lock_of(&live), "").unwrap();
    // A running run holds its lock with flock(1), as the script does; so does this stand-in.
    let mut holder = Command::new("flock")
        .arg("-x")
        .arg(lock_of(&live))
        .args(["sleep", "60"])
        .spawn()
        .unwrap();
    wait_until("the lock to be held", 10, || {
        !Command::new("flock")
            .arg("-n")
            .arg(lock_of(&live))
            .arg("true")
            .status()
            .unwrap()
            .success()
    });

    let out = rig.run(&["lint"]);
    holder.kill().unwrap();
    holder.wait().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", all_output(&out));
    assert!(!dead.exists(), "a dead run's folder stays");
    assert!(!lock_of(&dead).exists() && !stamp_of(&dead).exists());
    assert!(!rig.volumes().contains(&volume_for(&dead)));
    assert!(live.exists(), "a live run's folder was removed");
    assert!(rig.volumes().contains(&volume_for(&live)));
}

#[test]
fn a_dead_run_that_left_no_lock_waits_for_the_12_hour_rule() {
    let rig = rig!();
    let young = seed_run_dir(&rig, "4444-1-1-1", 1);
    let old = seed_run_dir(&rig, "5555-2-2-2", 13);
    assert_eq!(rig.run(&["lint"]).status.code(), Some(0));
    assert!(young.exists(), "no lock and 1 hour old");
    assert!(!old.exists(), "no lock and 13 hours old");
}

#[test]
fn a_slot_cap_makes_extra_runs_wait_and_every_run_gets_the_job_count() {
    let rig = rig!();
    let envs = [
        ("ANTHOLOGY_REMOTE_SLOTS", "1"),
        ("ANTHOLOGY_REMOTE_JOBS", "3"),
        ("FAKE_SLEEP", "1"),
    ];
    let runs: Vec<(String, Child)> = (0..3)
        .map(|i| {
            let tag = format!("r{i}");
            let child = rig.spawn_run(&tag, &envs, &["lint"]);
            (tag, child)
        })
        .collect();
    let mut waited = false;
    for (tag, child) in runs {
        assert_eq!(rig.finish(&tag, child), Some(0), "{tag}");
        let shown = rig.output_of(&tag);
        assert!(shown.contains("jobs=3"), "{tag}: {shown}");
        waited |= shown.contains("waiting for one");
    }
    assert!(waited, "no run had to wait for a slot");
    // One slot: the builds never overlap.
    let mut spans = rig.spans("spans");
    spans.sort();
    assert_eq!(spans.len(), 3);
    for pair in spans.windows(2) {
        assert!(
            pair[1].0 >= pair[0].1.unwrap(),
            "two builds overlapped: {spans:?}"
        );
    }
}

#[test]
fn the_host_works_out_a_job_count_and_a_slot_cap_when_none_is_given() {
    let rig = rig!();
    let out = rig
        .command(&["lint"])
        .env_remove("ANTHOLOGY_REMOTE_JOBS")
        .env_remove("ANTHOLOGY_REMOTE_SLOTS")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", all_output(&out));
    let stdout = text(&out.stdout);
    let jobs = stdout
        .lines()
        .find_map(|l| l.strip_prefix("jobs="))
        .unwrap_or_else(|| panic!("no jobs line: {stdout}"));
    assert!(jobs.parse::<u32>().is_ok_and(|n| n >= 1), "jobs={jobs}");
}

#[test]
fn a_bad_cap_is_refused_before_ssh() {
    let rig = rig!();
    for (var, value) in [
        ("ANTHOLOGY_REMOTE_JOBS", "0"),
        ("ANTHOLOGY_REMOTE_JOBS", "many"),
        ("ANTHOLOGY_REMOTE_SLOTS", "-1"),
    ] {
        let out = rig.command(&["lint"]).env(var, value).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{var}={value}");
    }
    assert!(rig.log("ssh.log").is_empty(), "ssh was called");
}

#[test]
fn the_toolchain_is_installed_once_however_many_runs_start_together() {
    let rig = rig!();
    fs::write(rig.repo.join("rust-toolchain.toml"), "channel = \"1\"\n").unwrap();
    let envs = [("FAKE_INSTALL_SECS", "2"), ("FAKE_SLEEP", "1")];
    let runs: Vec<(String, Child)> = (0..3)
        .map(|i| {
            let tag = format!("r{i}");
            let child = rig.spawn_run(&tag, &envs, &["lint"]);
            (tag, child)
        })
        .collect();
    for (tag, child) in runs {
        assert_eq!(rig.finish(&tag, child), Some(0), "{tag}");
    }
    let installs = || rig.log("installs").lines().count();
    assert_eq!(installs(), 1, "{}", rig.log("install-spans"));
    // A later run finds the marker; a new toolchain file makes one more install.
    assert_eq!(rig.run(&["lint"]).status.code(), Some(0));
    assert_eq!(installs(), 1);
    fs::write(rig.repo.join("rust-toolchain.toml"), "channel = \"2\"\n").unwrap();
    assert_eq!(rig.run(&["lint"]).status.code(), Some(0));
    assert_eq!(installs(), 2);
}

#[test]
fn a_failed_toolchain_install_is_tried_again_on_a_new_volume() {
    let rig = rig!();
    fs::write(rig.state.join("install-fail"), "").unwrap();
    let out = rig.run(&["lint"]);
    assert_eq!(out.status.code(), Some(0), "{}", all_output(&out));
    assert_eq!(rig.log("installs").lines().count(), 2);
    let docker = rig.log("docker.log");
    assert!(
        docker.contains("volume rm anthology-rustup-0123456789ab"),
        "{docker}"
    );
    assert!(
        docker.contains(
            "volume create --label anthology.remote-build=1 anthology-rustup-0123456789ab"
        ),
        "{docker}"
    );
    // The new volume is ours and has the label.
    let volumes = rig.volumes();
    let line = volumes
        .lines()
        .rfind(|l| l.starts_with("anthology-rustup-0123456789ab "))
        .unwrap();
    assert!(line.ends_with(" ours"), "{volumes}");
}

#[test]
fn an_install_that_fails_twice_stops_the_run() {
    let rig = rig!();
    fs::write(rig.state.join("install-fail-always"), "").unwrap();
    let out = rig.run(&["lint"]);
    assert_eq!(out.status.code(), Some(5), "{}", all_output(&out));
    assert!(
        text(&out.stderr).contains("could not be installed"),
        "{}",
        all_output(&out)
    );
    assert!(
        !text(&out.stdout).contains("rust-test args: lint"),
        "the run went on: {}",
        all_output(&out)
    );
    assert_eq!(
        rig.log("installs").lines().count(),
        2,
        "one try and one retry"
    );
}

#[test]
fn a_second_kept_run_of_one_checkout_is_refused_while_the_first_runs() {
    let rig = rig!();
    let first = rig.spawn_run(
        "a",
        &[("ANTHOLOGY_KEEP_REMOTE", "1"), ("FAKE_SLEEP", "10")],
        &["lint"],
    );
    wait_until("the build to start", 60, || !rig.spans("spans").is_empty());
    let out = rig.run(&["lint"]);
    assert_eq!(out.status.code(), Some(4), "{}", all_output(&out));
    assert!(
        text(&out.stderr).contains("kept folder"),
        "{}",
        all_output(&out)
    );
    assert_eq!(rig.finish("a", first), Some(0));
    assert!(
        rig.remote_dir().join("src/lib.rs").is_file(),
        "the kept folder"
    );
}

#[test]
fn a_killed_script_leaves_nothing_on_the_host() {
    let rig = rig!();
    let mut run = rig.spawn_run("a", &[("FAKE_HANG", "1")], &["lint"]);
    wait_until("the build to start", 60, || !rig.spans("spans").is_empty());
    assert_eq!(rig.remote_dirs().len(), 1);
    // No signal handler runs: only the host's own watch can clean up.
    run.kill().unwrap();
    run.wait().unwrap();
    wait_until("the host to clean up", 90, || {
        rig.remote_dirs().is_empty() && rig.containers().trim().is_empty()
    });
    assert!(
        !rig.volumes().contains("anthology-rust-target-"),
        "{}",
        rig.volumes()
    );
    assert_eq!(rig.left_under_root(), Vec::<String>::new());
}
