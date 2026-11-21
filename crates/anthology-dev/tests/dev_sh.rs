//! `scripts/dev.sh` keeps stdout for the tool's own output (DV2; controller ruling). The real
//! script runs with a fake `scripts/rust-test.sh` (`ANTHOLOGY_RUST_TEST`) that, like the real
//! container, prints setup noise on stdout before it runs the command, and a fake `cargo` that
//! runs the anthology-dev binary under test. Not ledger targets.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Mutex;

/// dev.sh writes its stdout file under the repository's `target/`; one run at a time, so a
/// test can check that the file is gone afterwards.
static SERIAL: Mutex<()> = Mutex::new(());

fn stdout_files() -> Vec<String> {
    let Ok(dir) = fs::read_dir(repo().join("target")) else {
        return Vec::new();
    };
    let mut names: Vec<String> = dir
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("dev-stdout."))
        .collect();
    names.sort();
    names
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root")
}

fn script(path: &Path, text: &str) {
    fs::write(path, text).expect("write");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod");
    anthology_testkit::rl::settle_exec(path, &[]);
}

/// Runs `scripts/dev.sh args…`; the tool runs in `tool_cwd` (the repository by default). The
/// stdout file is removed afterwards.
fn dev_sh(args: &[&str], tool_cwd: Option<&Path>) -> Output {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let before = stdout_files();
    let out = dev_sh_once(args, tool_cwd);
    let after = stdout_files();
    assert!(
        after.iter().all(|n| before.contains(n)),
        "left behind: {after:?} (before: {before:?})"
    );
    out
}

fn dev_sh_once(args: &[&str], tool_cwd: Option<&Path>) -> Output {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin = tmp.path().join("bin");
    fs::create_dir(&bin).expect("mkdir");
    let runner = tmp.path().join("rust-test.sh");
    script(
        &runner,
        "#!/bin/sh\n\
         echo 'Get:1 http://deb.example stable InRelease (setup noise)'\n\
         echo 'info: syncing channel updates (setup noise)'\n\
         [ \"$1\" = run ] || exit 9\n\
         shift\n\
         cd \"$DEV_TEST_ROOT\"\n\
         PATH=\"$DEV_TEST_BIN:$PATH\" exec \"$@\"\n",
    );
    script(
        &bin.join("cargo"),
        "#!/bin/sh\n\
         echo '   Compiling anthology-dev (build noise)' >&2\n\
         while [ \"$#\" -gt 0 ] && [ \"$1\" != -- ]; do shift; done\n\
         shift\n\
         [ -z \"${DEV_TEST_CWD:-}\" ] || cd \"$DEV_TEST_CWD\"\n\
         exec \"$DEV_TEST_TOOL\" \"$@\"\n",
    );
    let mut cmd = Command::new("bash");
    cmd.arg(repo().join("scripts/dev.sh"))
        .args(args)
        .env("ANTHOLOGY_RUST_TEST", &runner)
        .env("DEV_TEST_ROOT", repo())
        .env("DEV_TEST_BIN", &bin)
        .env("DEV_TEST_TOOL", env!("CARGO_BIN_EXE_anthology-dev"))
        .stdin(Stdio::null());
    if let Some(cwd) = tool_cwd {
        cmd.env("DEV_TEST_CWD", cwd);
    }
    cmd.output().expect("bash runs")
}

#[test]
fn stdout_is_only_the_tools_output_and_the_exit_status_passes_through() {
    let out = dev_sh(
        &["release-edit", "check-version", "1.0.0", "1.0.1", "0"],
        None,
    );
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(out.stdout, b"bump\n");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("(setup noise)") && err.contains("(build noise)"),
        "{err}"
    );

    let out = dev_sh(
        &["release-edit", "check-version", "1.0.0", "0.9.0", "0"],
        None,
    );
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(out.stdout.is_empty(), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("release: 0.9.0 is lower"));

    // Arguments reach the tool intact, spaces included.
    let out = dev_sh(&["eval-baseline", "record", "a b.json", "c d.json"], None);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(out.stdout.is_empty(), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("cannot read the result a b.json: "),
        "{out:?}"
    );
}

/// The sync-rtk.sh contract: `old_pin=$(scripts/dev.sh vendor set-rtk-upstream …)` is exactly
/// `<old ref> <old commit>`. Until DV1's `set-rtk-upstream` lands the command exits 70 (not
/// ported), and the test checks only that dev.sh passes that through with a clean stdout.
#[test]
fn set_rtk_upstream_stdout_is_exactly_the_old_pin() {
    let fixture = tempfile::tempdir().expect("tempdir");
    fs::create_dir(fixture.path().join("vendor")).expect("mkdir");
    let sources = fs::read_to_string(repo().join("vendor/sources.json")).expect("sources.json");
    fs::write(fixture.path().join("vendor/sources.json"), &sources).expect("copy");
    let data = anthology_pycompat::json::loads(&sources).expect("json");
    let rtk = data
        .as_object()
        .and_then(|m| m.get("sources"))
        .and_then(|s| s.as_array())
        .and_then(|a| {
            a.iter().find(|e| {
                e.as_object()
                    .and_then(|o| o.get("name"))
                    .and_then(|n| n.as_str())
                    == Some("rtk")
            })
        })
        .and_then(|e| e.as_object())
        .and_then(|e| e.get("upstream"))
        .and_then(|u| u.as_object())
        .expect("the rtk entry");
    let field = |k: &str| rtk.get(k).and_then(|v| v.as_str()).expect(k).to_string();
    let old_pin = format!("{} {}\n", field("ref"), field("commit"));

    let out = dev_sh(
        &[
            "vendor",
            "set-rtk-upstream",
            "--ref",
            "v9.9.9",
            "--commit",
            "0123456789abcdef0123456789abcdef01234567",
        ],
        Some(fixture.path()),
    );
    if out.status.code() == Some(70) {
        assert!(out.stdout.is_empty(), "{out:?}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("vendor set-rtk-upstream is not ported")
        );
        return;
    }
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), old_pin);
}
