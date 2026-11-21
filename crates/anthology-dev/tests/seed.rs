//! `anthology-dev seed` with the testkit's fake binary (RL1, N-I4; design §8.3, §8.4, §14.2).
//!
//! The live check with the real binary is EV1b's. Here the fake stands in for it: seed probes
//! its `--version`, installs it as "current", pins it in a scratch plugin tree, and the guard
//! shim then runs it without quarantining. The shim is BS1's staging `guard.sh` once it exists,
//! and until then a minimal shim with GC-17's hot-path classification ([`SHIM`]).

use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use anthology_core::platform;
use anthology_testkit::rl::{
    FAKE_BLOCK_LINE, FAKE_BLOCK_MARKER, fake_anthology, fake_anthology_with, settle_exec,
};
use sha2::{Digest, Sha256};

const BIN: &str = env!("CARGO_BIN_EXE_anthology-dev");
const LINUX: &str = "x86_64-unknown-linux-musl";
const BLOCK_PREFIX: &str = "Blocked by anthology-guardrails (";

/// BS1's staging guard shim, relative to the repository root.
const STAGING_GUARD: &str = "port/staging/plugins/anthology-guardrails/hooks/guard.sh";

/// GC-17's hot path, as a stand-in until BS1's `guard.sh` exists: the version from the lock,
/// "installed" by builtins only, `err=$("$bin" guard hook --shim 2>&1 >/dev/null)`, allow only on
/// exit 0 plus the exact token, pass an exit 2 block through, quarantine everything else.
const SHIM: &str = r#"#!/bin/sh
root=$CLAUDE_PLUGIN_ROOT
ah=$CLAUDE_CONFIG_DIR/anthology
block() { printf 'Blocked by anthology-guardrails (setup): %s\n' "$1" >&2; exit 2; }
version=$(sed -n 's/^  "version": "\([^"]*\)",$/\1/p' "$root/anthology.lock")
[ -n "$version" ] || block "anthology.lock is not valid"
dir=$ah/bin/$version
bin=$dir/anthology
[ -d "$dir" ] && [ -f "$bin" ] && [ -x "$bin" ] && [ -s "$bin" ] && [ -s "$dir/.sha256" ] \
    || block "the guard binary is not installed"
payload=$(mktemp) || block "mktemp failed"
cat >"$payload"
err=$("$bin" guard hook --shim <"$payload" 2>&1 >/dev/null)
rc=$?
rm -f "$payload"
if [ "$rc" -eq 0 ] && [ "$err" = "anthology-guard:allow:v1" ]; then
    exit 0
fi
case "$rc:$err" in
    "2:Blocked by anthology-guardrails ("*)
        printf '%s\n' "$err" >&2
        exit 2
        ;;
esac
mv "$dir" "$ah/bin/.broken-$version-$(date +%s)-$$" 2>/dev/null
block "the guard binary did not work (exit $rc) and was set aside"
"#;

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn zeros() -> String {
    "0".repeat(64)
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A scratch tree: `plugins/{anthology,anthology-guardrails}`, a config dir and a bin dir.
struct Scratch {
    _tmp: tempfile::TempDir,
    root: PathBuf,
}

impl Scratch {
    fn new() -> Scratch {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        for p in ["anthology", "anthology-guardrails"] {
            std::fs::create_dir_all(root.join("plugins").join(p)).unwrap();
        }
        std::fs::create_dir_all(root.join("config")).unwrap();
        Scratch { _tmp: tmp, root }
    }

    fn plugins(&self) -> PathBuf {
        self.root.join("plugins")
    }

    fn config(&self) -> PathBuf {
        self.root.join("config")
    }

    fn bin_dir(&self) -> PathBuf {
        self.config().join("anthology/bin")
    }

    fn fake(&self, version: &str, target: &str) -> PathBuf {
        fake_anthology(&self.root.join("build"), version, target)
    }

    fn seed(&self, bin: &Path, extra: &[&str]) -> Output {
        let mut args = vec![
            "seed".to_string(),
            "--bin".into(),
            bin.display().to_string(),
            "--plugins-dir".into(),
            self.plugins().display().to_string(),
            "--config-dir".into(),
            self.config().display().to_string(),
        ];
        args.extend(extra.iter().map(|s| s.to_string()));
        Command::new(BIN)
            .args(&args)
            .current_dir(&self.root)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .stdin(Stdio::null())
            .output()
            .expect("run anthology-dev seed")
    }

    fn lock(&self, plugin: &str) -> String {
        std::fs::read_to_string(self.plugins().join(plugin).join("anthology.lock")).unwrap()
    }
}

fn lock_with(version: &str, hashes: [&str; 4]) -> String {
    format!(
        "{{\n  \"version\": \"{version}\",\n  \"tag\": \"anthology-bin--v{version}\",\n  \"repo\": \"rbonweb/anthology\",\n  \"sha256\": {{\n    \"aarch64-apple-darwin\": \"{}\",\n    \"x86_64-apple-darwin\": \"{}\",\n    \"x86_64-unknown-linux-musl\": \"{}\",\n    \"aarch64-unknown-linux-musl\": \"{}\"\n  }}\n}}\n",
        hashes[0], hashes[1], hashes[2], hashes[3]
    )
}

/// The members of a .tar.gz: (path, mode, bytes).
fn members(archive: &[u8]) -> Vec<(String, u32, Vec<u8>)> {
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    tar.entries()
        .unwrap()
        .map(|e| {
            let mut e = e.unwrap();
            let path = e.path().unwrap().display().to_string();
            let mode = e.header().mode().unwrap();
            let mut bytes = Vec::new();
            e.read_to_end(&mut bytes).unwrap();
            (path, mode, bytes)
        })
        .collect()
}

#[test]
fn seed_installs_the_current_layout_and_pins_it() {
    let s = Scratch::new();
    let fake = s.fake("0.1.0", LINUX);
    let out = s.seed(&fake, &["--target", LINUX]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let dir = s.bin_dir().join("0.1.0");
    assert_eq!(
        text(&out.stdout),
        format!("seeded anthology 0.1.0 ({LINUX}) into {}\n", dir.display())
    );

    let fake_bytes = std::fs::read(&fake).unwrap();
    let installed = dir.join("anthology");
    assert_eq!(std::fs::read(&installed).unwrap(), fake_bytes);
    let meta = std::fs::symlink_metadata(&installed).unwrap();
    assert!(meta.file_type().is_file());
    assert_eq!(meta.permissions().mode() & 0o777, 0o755);
    assert_eq!(
        std::fs::read_to_string(dir.join(".bin-sha256")).unwrap(),
        format!("{}\n", sha(&fake_bytes))
    );

    let archive = std::fs::read(s.bin_dir().join(format!("anthology-{LINUX}.tar.gz"))).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join(".sha256")).unwrap(),
        format!("{}\n", sha(&archive))
    );
    assert_eq!(
        members(&archive),
        [("anthology".to_string(), 0o755, fake_bytes)]
    );

    let want = lock_with("0.1.0", [&zeros(), &zeros(), &sha(&archive), &zeros()]);
    assert_eq!(s.lock("anthology"), want);
    assert_eq!(s.lock("anthology-guardrails"), want);
    let check = Command::new(BIN)
        .args(["lock", "check", "--plugins-dir"])
        .arg(s.plugins())
        .output()
        .unwrap();
    assert_eq!(check.status.code(), Some(0), "{}", text(&check.stderr));

    // Only the version folder and the seed archive: no staging folder is left behind.
    let mut names: Vec<String> = std::fs::read_dir(s.bin_dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["0.1.0".to_string(), format!("anthology-{LINUX}.tar.gz")]
    );

    // Seeding again replaces the install.
    let fake2 = fake_anthology_with(&s.root.join("build2"), "0.1.0", LINUX, "other-token");
    let out = s.seed(&fake2, &["--target", LINUX]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(
        std::fs::read(&installed).unwrap(),
        std::fs::read(&fake2).unwrap()
    );
}

#[test]
fn seed_defaults_to_the_host_target() {
    let (sys, machine) = platform::uname();
    let Ok(host) = platform::detect_with((&sys, &machine)) else {
        eprintln!("skipped: unsupported host {sys}/{machine}");
        return;
    };
    let s = Scratch::new();
    let fake = s.fake("0.1.0", host.triple());
    let out = s.seed(&fake, &[]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(
        s.bin_dir()
            .join(format!("anthology-{}.tar.gz", host.triple()))
            .is_file()
    );
}

#[test]
fn seed_keeps_the_other_targets_of_a_lock_with_the_same_version() {
    let s = Scratch::new();
    let a: String = "a".repeat(64);
    let b: String = "b".repeat(64);
    let c: String = "c".repeat(64);
    let d: String = "d".repeat(64);
    for p in ["anthology", "anthology-guardrails"] {
        std::fs::write(
            s.plugins().join(p).join("anthology.lock"),
            lock_with("0.1.0", [&a, &b, &c, &d]),
        )
        .unwrap();
    }
    let out = s.seed(&s.fake("0.1.0", LINUX), &["--target", LINUX]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let archive = std::fs::read(s.bin_dir().join(format!("anthology-{LINUX}.tar.gz"))).unwrap();
    assert_eq!(
        s.lock("anthology"),
        lock_with("0.1.0", [&a, &b, &sha(&archive), &d])
    );

    // Another version: the other targets are zeros.
    let out = s.seed(&s.fake("0.2.0-rc.1", LINUX), &["--target", LINUX]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let archive = std::fs::read(s.bin_dir().join(format!("anthology-{LINUX}.tar.gz"))).unwrap();
    assert_eq!(
        s.lock("anthology-guardrails"),
        lock_with("0.2.0-rc.1", [&zeros(), &zeros(), &sha(&archive), &zeros()])
    );
}

#[test]
fn seed_writes_into_the_plugins_that_exist() {
    let s = Scratch::new();
    std::fs::remove_dir(s.plugins().join("anthology")).unwrap();
    let out = s.seed(&s.fake("0.1.0", LINUX), &["--target", LINUX]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(
        s.plugins()
            .join("anthology-guardrails/anthology.lock")
            .is_file()
    );
    assert!(!s.plugins().join("anthology").exists());
}

/// Writes an executable `#!/bin/sh` script.
fn script(path: &Path, body: &str) -> PathBuf {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(format!("#!/bin/sh\n{body}\n").as_bytes())
        .unwrap();
    drop(f);
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    settle_exec(path, &[]);
    path.to_path_buf()
}

/// Seed fails with exit 1 and writes nothing at all.
fn assert_refused(s: &Scratch, bin: &Path, extra: &[&str], needle: &str) {
    let out = s.seed(bin, extra);
    let err = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{extra:?}: {err}");
    assert!(err.contains(needle), "{needle:?} not in {err}");
    assert!(
        !s.plugins().join("anthology/anthology.lock").exists(),
        "{err}"
    );
    assert!(!s.bin_dir().join("0.1.0").exists(), "{err}");
}

#[test]
fn seed_refuses_a_binary_whose_version_probe_fails() {
    let s = Scratch::new();
    let other = s.fake("0.1.0", "aarch64-apple-darwin");
    assert_refused(&s, &other, &["--target", LINUX], "--version printed");
    let bad_version = s.fake("0.1", LINUX);
    assert_refused(&s, &bad_version, &["--target", LINUX], "invalid version");
    let fails = script(
        &s.root.join("x/fails"),
        &format!("echo 'anthology 0.1.0 ({LINUX})'; exit 1"),
    );
    assert_refused(&s, &fails, &["--target", LINUX], "--version failed");
    let chatty = script(
        &s.root.join("x/chatty"),
        &format!("echo 'anthology 0.1.0 ({LINUX})'; echo more"),
    );
    assert_refused(&s, &chatty, &["--target", LINUX], "--version printed");
    let empty = script(&s.root.join("x/empty"), "exit 0");
    assert_refused(&s, &empty, &["--target", LINUX], "--version printed");
    assert_refused(
        &s,
        &s.root.join("x/missing"),
        &["--target", LINUX],
        "cannot run",
    );
}

#[test]
fn seed_refuses_a_tracked_plugin_tree() {
    if Command::new("git").arg("--version").output().is_err() {
        eprintln!("skipped: git is not installed");
        return;
    }
    let s = Scratch::new();
    let fake = s.fake("0.1.0", LINUX);
    std::fs::write(s.plugins().join("anthology/README.md"), "x\n").unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .arg("-C")
            .arg(s.plugins())
            .args(args)
            .env_remove("GIT_DIR")
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", text(&out.stderr));
    };
    git(&["init", "-q"]);
    git(&["add", "anthology/README.md"]);
    assert_refused(&s, &fake, &["--target", LINUX], "tracked by git");
}

#[test]
fn seed_refuses_a_symlinked_bin_folder() {
    let s = Scratch::new();
    let fake = s.fake("0.1.0", LINUX);
    let elsewhere = s.root.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::create_dir_all(s.config().join("anthology")).unwrap();
    std::os::unix::fs::symlink(&elsewhere, s.bin_dir()).unwrap();
    assert_refused(&s, &fake, &["--target", LINUX], "is a symlink");
    assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0);
}

#[test]
fn seed_needs_a_plugin_folder_and_its_options() {
    let s = Scratch::new();
    let fake = s.fake("0.1.0", LINUX);
    for p in ["anthology", "anthology-guardrails"] {
        std::fs::remove_dir(s.plugins().join(p)).unwrap();
    }
    let out = s.seed(&fake, &["--target", LINUX]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    for args in [
        &["seed"][..],
        &["seed", "--bin", "x", "--plugins-dir", "p"],
        &[
            "seed",
            "--bin",
            "x",
            "--plugins-dir",
            "p",
            "--config-dir",
            "c",
            "--target",
            "x86_64-pc-windows-msvc",
        ],
        &["seed", "--f1-registration-probe"],
    ] {
        let out = Command::new(BIN)
            .args(args)
            .current_dir(&s.root)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
    }
}

// ------------------------------------------------------------------------------------------
// The guard shim runs the seeded fake

/// The scratch guardrails plugin's guard shim: BS1's `guard.sh` (with the rest of its hooks
/// folder) when it exists, else [`SHIM`].
fn install_shim(s: &Scratch) -> PathBuf {
    let hooks = s.plugins().join("anthology-guardrails/hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    let staging = repo_root().join(STAGING_GUARD);
    if staging.is_file() {
        for entry in std::fs::read_dir(staging.parent().unwrap()).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_file() {
                std::fs::copy(entry.path(), hooks.join(entry.file_name())).unwrap();
            }
        }
        hooks.join("guard.sh")
    } else {
        script(
            &hooks.join("guard.sh"),
            SHIM.trim_start_matches("#!/bin/sh\n"),
        )
    }
}

fn run_shim(s: &Scratch, shim: &Path, command: &str) -> Output {
    let payload =
        format!("{{\"tool_name\":\"Bash\",\"tool_input\":{{\"command\":\"{command}\"}}}}");
    let mut child = Command::new("/bin/sh")
        .arg(shim)
        .env(
            "CLAUDE_PLUGIN_ROOT",
            s.plugins().join("anthology-guardrails"),
        )
        .env("CLAUDE_CONFIG_DIR", s.config())
        .env("HOME", s.root.join("home"))
        .env("ANTHOLOGY_GUARD_NO_FETCH", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn broken(s: &Scratch) -> Vec<String> {
    std::fs::read_dir(s.bin_dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(".broken-"))
        .collect()
}

fn host_or_skip() -> Option<String> {
    let (sys, machine) = platform::uname();
    match platform::detect_with((&sys, &machine)) {
        Ok(t) => Some(t.triple().to_string()),
        Err(_) => {
            eprintln!("skipped: unsupported host {sys}/{machine}");
            None
        }
    }
}

#[test]
fn the_guard_shim_runs_the_seeded_fake_without_quarantining() {
    let Some(host) = host_or_skip() else { return };
    let s = Scratch::new();
    let out = s.seed(&s.fake("0.1.0", &host), &[]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let shim = install_shim(&s);

    let allow = run_shim(&s, &shim, "ls");
    assert_eq!(allow.status.code(), Some(0), "{}", text(&allow.stderr));

    let block = run_shim(&s, &shim, &format!("echo {FAKE_BLOCK_MARKER}"));
    let err = text(&block.stderr);
    assert_eq!(block.status.code(), Some(2), "{err}");
    assert!(err.starts_with(BLOCK_PREFIX), "{err}");
    assert!(err.contains(FAKE_BLOCK_LINE), "{err}");

    assert_eq!(broken(&s), Vec::<String>::new());
    assert!(s.bin_dir().join("0.1.0/anthology").is_file());
}

#[test]
fn the_guard_shim_quarantines_a_seeded_binary_with_a_wrong_token() {
    // The negative control: the shim test above can fail.
    let Some(host) = host_or_skip() else { return };
    let s = Scratch::new();
    let fake = fake_anthology_with(
        &s.root.join("build"),
        "0.1.0",
        &host,
        "anthology-guard:allow:v2",
    );
    let out = s.seed(&fake, &[]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let shim = install_shim(&s);
    let out = run_shim(&s, &shim, "ls");
    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
    assert_eq!(broken(&s).len(), 1, "{}", text(&out.stderr));
}

#[test]
fn the_fake_answers_like_the_binary() {
    let s = Scratch::new();
    let fake = s.fake("1.2.3", LINUX);
    let run = |args: &[&str], stdin: &str| {
        let mut child = Command::new(&fake)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        let o = child.wait_with_output().unwrap();
        (o.status.code().unwrap(), text(&o.stdout), text(&o.stderr))
    };
    assert_eq!(
        run(&["--version"], ""),
        (0, format!("anthology 1.2.3 ({LINUX})\n"), String::new())
    );
    assert_eq!(
        run(&["guard", "hook", "--shim"], "{}"),
        (0, String::new(), "anthology-guard:allow:v1\n".into())
    );
    assert_eq!(
        run(&["guard", "hook"], "{}"),
        (0, String::new(), String::new())
    );
    let (code, _, err) = run(&["guard", "hook", "--shim"], FAKE_BLOCK_MARKER);
    assert_eq!((code, err), (2, format!("{FAKE_BLOCK_LINE}\n")));
    let (code, _, err) = run(&["guard", "hook"], FAKE_BLOCK_MARKER);
    assert_eq!((code, err), (2, format!("{FAKE_BLOCK_LINE}\n")));
    assert_eq!(run(&["doctor"], "").0, 64);
    assert_eq!(run(&["guard", "hook", "--other"], "").0, 64);
}

#[test]
fn seed_refuses_when_it_cannot_ask_git() {
    let s = Scratch::new();
    let fake = s.fake("0.1.0", LINUX);
    let empty = s.root.join("empty-path");
    std::fs::create_dir_all(&empty).unwrap();
    let out = Command::new(BIN)
        .args(["seed", "--target", LINUX, "--bin"])
        .arg(&fake)
        .arg("--plugins-dir")
        .arg(s.plugins())
        .arg("--config-dir")
        .arg(s.config())
        .env("PATH", &empty)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let err = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{err}");
    assert!(err.contains("cannot run git"), "{err}");
    assert!(!s.plugins().join("anthology/anthology.lock").exists());
    assert!(!s.bin_dir().exists());
}

#[test]
fn seed_never_touches_this_repositorys_own_plugins() {
    let s = Scratch::new();
    let fake = s.fake("0.1.0", LINUX);
    let own = std::fs::canonicalize(repo_root().join("plugins")).unwrap();
    let before = std::fs::read_dir(own.join("anthology")).unwrap().count();
    let out = Command::new(BIN)
        .args(["seed", "--target", LINUX, "--bin"])
        .arg(&fake)
        .arg("--plugins-dir")
        .arg(&own)
        .arg("--config-dir")
        .arg(s.config())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let err = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{err}");
    assert!(err.contains("this repository's plugins"), "{err}");
    assert_eq!(
        std::fs::read_dir(own.join("anthology")).unwrap().count(),
        before
    );
    assert!(!s.bin_dir().exists());
}

#[test]
fn seed_refuses_a_symlinked_home_or_version_folder() {
    let s = Scratch::new();
    let fake = s.fake("0.1.0", LINUX);
    let elsewhere = s.root.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, s.config().join("anthology")).unwrap();
    assert_refused(&s, &fake, &["--target", LINUX], "is a symlink");
    assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0);

    let s = Scratch::new();
    let fake = s.fake("0.1.0", LINUX);
    let elsewhere = s.root.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::create_dir_all(s.bin_dir()).unwrap();
    std::os::unix::fs::symlink(&elsewhere, s.bin_dir().join("0.1.0")).unwrap();
    let out = s.seed(&fake, &["--target", LINUX]);
    let err = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{err}");
    assert!(err.contains("is a symlink"), "{err}");
    assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0);
    assert!(!s.plugins().join("anthology/anthology.lock").exists());
}
