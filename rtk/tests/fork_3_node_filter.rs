// Unix only: the stand-in `node` is a shell script.
#![cfg(unix)]
//! Fork change 3 (rbonweb/rtk cedececf): `node …` is rewritten to `rtk node …`,
//! and the built-in `node` TOML filter drops blank lines and stack-frame noise.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

mod common;

const RAW: &str = "hello\n\n    at Object.<anonymous> (/srv/app.js:1:7)\nworld\n";

/// A directory holding an executable `name` that prints `output`.
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

#[test]
fn fork_3_node_is_rewritten() {
    let out = common::rtk_command()
        .args(["rewrite", "node app.js"])
        .output()
        .expect("spawn rtk rewrite");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "rtk node app.js"
    );
}

#[test]
fn fork_3_node_filter_drops_blank_lines_and_stack_frames() {
    let bin = common::tempdir();
    fake_tool(bin.path(), "node", RAW);
    let project = common::tempdir();

    let out = common::rtk_command()
        .args(["node", "app.js"])
        .current_dir(project.path())
        .env("PATH", path_with(bin.path()))
        .env("RTK_TELEMETRY_DISABLED", "1")
        .output()
        .expect("spawn rtk node");

    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("hello"), "{stdout}");
    assert!(stdout.contains("world"), "{stdout}");
    assert!(
        !stdout.contains("at Object."),
        "stack frames dropped: {stdout}"
    );
    assert!(!stdout.contains("\n\n"), "blank lines dropped: {stdout:?}");
}
