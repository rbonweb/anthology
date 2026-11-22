//! Every plan §1.3 command is registered (F1). A command that is not ported yet exits 70 with
//! the stub line; a ported one may do anything except fall through to "unknown command".

use std::process::{Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_anthology");

/// Plan §1.3, `anthology`.
const COMMANDS: &[&[&str]] = &[
    &["guard", "hook"],
    &["hook", "session-start"],
    &["setup", "preview"],
    &["setup", "apply"],
    &["setup", "remove"],
    &["doctor"],
    &["rtk-install"],
    &["mem", "search"],
    &["mem", "related"],
    &["mem", "orphans"],
    &["mem", "stale"],
    &["mem", "rebuild"],
    &["mem", "save"],
    &["memory", "export"],
    &["memory", "import"],
];

/// An option no command accepts, so a ported command stops at its usage error instead of
/// acting; the run is sandboxed as well.
const PROBE: &str = "--f1-registration-probe";

fn run(args: &[&str]) -> (i32, String, String) {
    let sandbox = tempfile::tempdir().expect("temp dir");
    let out = Command::new(BIN)
        .args(args)
        .env_clear()
        .env("HOME", sandbox.path())
        .env("CLAUDE_CONFIG_DIR", sandbox.path().join("config"))
        .env("CLAUDE_PLUGIN_DATA", sandbox.path().join("data"))
        .env("PATH", "/usr/bin:/bin")
        .current_dir(sandbox.path())
        .stdin(Stdio::null())
        .output()
        .expect("run anthology");
    (
        out.status.code().expect("exit code"),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn version_names_the_package_version_and_the_target() {
    let (code, stdout, stderr) = run(&["--version"]);
    assert_eq!(code, 0, "{stderr}");
    let want = format!("anthology {} (", env!("CARGO_PKG_VERSION"));
    assert!(stdout.starts_with(&want), "{stdout:?}");
    assert!(stdout.ends_with(")\n"), "{stdout:?}");
    let target = &stdout[want.len()..stdout.len() - 2];
    assert!(
        target.split('-').count() >= 3,
        "not a target triple: {target:?}"
    );
}

#[test]
fn every_command_is_registered() {
    for args in COMMANDS {
        let (code, _, stderr) = run(&[args, &[PROBE][..]].concat());
        assert!(!stderr.contains("unknown command"), "{args:?}: {stderr}");
        if code == 70 {
            assert_eq!(
                stderr,
                format!("anthology: {} is not ported yet\n", args.join(" ")),
                "{args:?}"
            );
        }
    }
}

#[test]
fn an_unknown_command_is_a_usage_error() {
    let (code, stdout, stderr) = run(&["no-such-command"]);
    assert_eq!(code, 2);
    assert_eq!(stdout, "");
    assert!(
        stderr.starts_with("anthology: unknown command: no-such-command\n"),
        "{stderr}"
    );
    let (code, _, _) = run(&[]);
    assert_eq!(code, 2);
}
