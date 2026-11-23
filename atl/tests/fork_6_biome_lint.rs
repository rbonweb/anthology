// Unix only: the stand-in `biome` is a shell script.
#![cfg(unix)]
//! Fork change 6 (rbonweb/rtk d6c1ca94): package lint scripts run as scripts
//! (`pnpm lint` -> `atl pnpm lint`), explicit biome forms keep the tool name
//! (`pnpm biome check` -> `atl lint biome check`), and the biome filter keeps
//! every diagnostic as one `file:line:col rule message` line with biome's counts.
//! `tests/package_script_lint_test.rs` covers the script runner in more depth.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

mod common;

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/biome_check_warnings_raw.txt"
);

fn rewritten(command: &str) -> String {
    let out = common::atl_command()
        .args(["rewrite", command])
        .output()
        .expect("spawn atl rewrite");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn path_with(dir: &Path) -> std::ffi::OsString {
    let mut paths = vec![dir.to_path_buf()];
    if let Some(current) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&current));
    }
    std::env::join_paths(paths).expect("join PATH")
}

#[test]
fn fork_6_lint_scripts_and_biome_forms_are_routed() {
    for (raw, expected) in [
        ("pnpm lint", "atl pnpm lint"),
        ("npm run lint", "atl npm run lint"),
        ("pnpm biome check", "atl lint biome check"),
        ("npx biome check src", "atl lint biome check src"),
    ] {
        assert_eq!(rewritten(raw), expected, "rewrite of `{raw}`");
    }
}

#[test]
fn fork_6_biome_filter_keeps_every_diagnostic_and_the_counts() {
    let bin = common::tempdir();
    let script = bin.path().join("biome");
    std::fs::write(&script, format!("#!/bin/sh\ncat '{FIXTURE}'\n")).expect("write biome");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let project = common::tempdir();

    let out = common::atl_command()
        .args(["lint", "biome", "check"])
        .current_dir(project.path())
        .env("PATH", path_with(bin.path()))
        .env("ATL_TELEMETRY_DISABLED", "1")
        .output()
        .expect("spawn atl lint biome");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Biome: 0 errors, 18 warnings"),
        "biome's own counts: {stdout}"
    );
    assert!(
        stdout.contains(
            "src/styles/cards.module.css:1260:21 lint/complexity/noImportantStyles \
             Avoid the use of the !important style."
        ),
        "one line per diagnostic: {stdout}"
    );
    assert!(!stdout.contains('━'), "header rules dropped: {stdout}");
    assert!(!stdout.contains(" │ "), "code frames dropped: {stdout}");
}
