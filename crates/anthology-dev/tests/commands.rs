//! Every plan §1.3 `anthology-dev` command is registered (F1). A command that is not ported yet
//! exits 70 with the stub line; a ported one may do anything except fall through to "unknown
//! command".

use std::process::{Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_anthology-dev");

/// Plan §1.3, `anthology-dev`.
const COMMANDS: &[&[&str]] = &[
    &["vendor", "fetch"],
    &["vendor", "build"],
    &["vendor", "check"],
    &["vendor", "sync"],
    &["vendor", "diff"],
    &["vendor", "set-rtk-upstream"],
    &["version-bump"],
    &["release-edit", "plugin-version"],
    &["release-edit", "check-version"],
    &["release-edit", "check-dependents"],
    &["release-edit", "changelog-check"],
    &["release-edit", "changelog-move"],
    &["release-edit", "set-version"],
    &["release-edit", "set-source-version"],
    &["memory-diff"],
    &["startup-budget"],
    &["eval-baseline", "record"],
    &["eval-baseline", "compare"],
    &["eval-baseline", "candidate"],
    &["upstream-watch"],
    &["rtk-trim"],
    &["smoke"],
    &["lock", "write"],
    &["lock", "check"],
    &["lock", "check-fresh"],
    &["seed"],
    &["ledger", "check"],
    &["ledger", "ids"],
    &["golden", "bless"],
    &["golden", "check"],
    &["gen-unicode"],
    &["gen-errno"],
    &["replay", "collect"],
    &["replay", "run"],
    &["replay", "report"],
];

/// An option no command accepts, so a ported command stops at its usage error instead of
/// acting; the run is sandboxed as well.
const PROBE: &str = "--f1-registration-probe";

fn run(args: &[&str]) -> (i32, String) {
    let sandbox = tempfile::tempdir().expect("temp dir");
    let out = Command::new(BIN)
        .args(args)
        .env_clear()
        .env("HOME", sandbox.path())
        .env("PATH", "/usr/bin:/bin")
        .current_dir(sandbox.path())
        .stdin(Stdio::null())
        .output()
        .expect("run anthology-dev");
    (
        out.status.code().expect("exit code"),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn every_command_is_registered() {
    for args in COMMANDS {
        let (code, stderr) = run(&[args, &[PROBE][..]].concat());
        assert!(!stderr.contains("unknown command"), "{args:?}: {stderr}");
        if code == 70 {
            assert_eq!(
                stderr,
                format!("anthology-dev: {} is not ported yet\n", args.join(" ")),
                "{args:?}"
            );
        }
    }
}

#[test]
fn an_unknown_command_is_a_usage_error() {
    let (code, stderr) = run(&["no-such-command"]);
    assert_eq!(code, 2);
    assert!(
        stderr.starts_with("anthology-dev: unknown command: no-such-command\n"),
        "{stderr}"
    );
}
