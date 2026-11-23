// Unix only: the stand-in `opencode` is a shell script, and `atl opencode run`
// merges the child's streams through `sh`.
#![cfg(unix)]
//! Fork change 7 (rbonweb/rtk f8e01c29): `opencode run …` is rewritten to
//! `atl opencode run …`, which keeps the model header, a step summary, the errors
//! and the final answer verbatim, and drops the step-by-step trace.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

mod common;

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/opencode_run_multistep_raw.txt"
);

fn path_with(dir: &Path) -> std::ffi::OsString {
    let mut paths = vec![dir.to_path_buf()];
    if let Some(current) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&current));
    }
    std::env::join_paths(paths).expect("join PATH")
}

#[test]
fn fork_7_opencode_run_is_rewritten() {
    let out = common::atl_command()
        .args(["rewrite", "opencode run \"fix the bug\""])
        .output()
        .expect("spawn atl rewrite");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "atl opencode run \"fix the bug\""
    );
}

#[test]
fn fork_7_opencode_run_keeps_the_answer_and_drops_the_trace() {
    let bin = common::tempdir();
    let script = bin.path().join("opencode");
    std::fs::write(&script, format!("#!/bin/sh\ncat '{FIXTURE}'\n")).expect("write opencode");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let project = common::tempdir();

    let out = common::atl_command()
        .args(["opencode", "run", "fix the bug"])
        .current_dir(project.path())
        .env("PATH", path_with(bin.path()))
        .env("ATL_TELEMETRY_DISABLED", "1")
        .output()
        .expect("spawn atl opencode run");

    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let raw = std::fs::read_to_string(FIXTURE).expect("read fixture");
    assert!(stdout.contains("> build · deepseek-v4.1-flash"), "{stdout}");
    assert!(stdout.contains("All six steps are done."), "{stdout}");
    assert!(
        stdout.contains("- **calc.py fixed** — `add` returned `a - b`; changed to `return a + b`."),
        "the final answer is verbatim: {stdout}"
    );
    assert!(
        !stdout.contains("drwxr-xr-x"),
        "tool output from the trace is dropped: {stdout}"
    );
    assert!(stdout.len() < raw.len() / 2, "{stdout}");
}
