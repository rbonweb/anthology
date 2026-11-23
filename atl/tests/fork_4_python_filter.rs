// Unix only: the stand-in `python` is a shell script.
#![cfg(unix)]
//! Fork change 4 (rbonweb/rtk cedececf): `python …` is rewritten to `atl python …`
//! (specific rules such as pytest and mypy still win), and the built-in `python`
//! TOML filter drops blank lines and the traceback banner.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

mod common;

const RAW: &str =
    "hello\n\nTraceback (most recent call last):\n  File \"script.py\", line 1\nValueError: boom\n";

fn fake_tool(dir: &Path, name: &str, output: &str) {
    std::fs::write(dir.join("out.txt"), output).expect("write output");
    let script = dir.join(name);
    std::fs::write(
        &script,
        format!("#!/bin/sh\ncat '{}'\n", dir.join("out.txt").display()),
    )
    .expect("write script");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
}

fn path_with(dir: &Path) -> std::ffi::OsString {
    let mut paths = vec![dir.to_path_buf()];
    if let Some(current) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&current));
    }
    std::env::join_paths(paths).expect("join PATH")
}

fn rewritten(command: &str) -> String {
    let out = common::atl_command()
        .args(["rewrite", command])
        .output()
        .expect("spawn atl rewrite");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn fork_4_python_is_rewritten_and_specific_rules_still_win() {
    assert_eq!(rewritten("python script.py"), "atl python script.py");
    assert_eq!(rewritten("python3 -m pytest"), "atl pytest");
}

#[test]
fn fork_4_python_filter_drops_blank_lines_and_the_traceback_banner() {
    let bin = common::tempdir();
    fake_tool(bin.path(), "python", RAW);
    let project = common::tempdir();

    let out = common::atl_command()
        .args(["python", "script.py"])
        .current_dir(project.path())
        .env("PATH", path_with(bin.path()))
        .env("ATL_TELEMETRY_DISABLED", "1")
        .output()
        .expect("spawn atl python");

    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("hello"), "{stdout}");
    assert!(
        stdout.contains("ValueError: boom"),
        "the error stays: {stdout}"
    );
    assert!(
        !stdout.contains("Traceback (most recent call last):"),
        "banner dropped: {stdout}"
    );
    assert!(!stdout.contains("\n\n"), "blank lines dropped: {stdout:?}");
}
