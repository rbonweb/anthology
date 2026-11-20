//! The shipped binary has no `serde_json`: JSON goes through `pycompat::json` only (GC-16, m-11).
//! cargo-deny bans are graph-wide, so this checks the normal-dependency tree of anthology-cli.

use std::path::Path;
use std::process::Command;

#[test]
fn the_anthology_cli_tree_has_no_serde_json() {
    let cargo = option_env!("CARGO").unwrap_or("cargo");
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out = Command::new(cargo)
        .args([
            "tree",
            "--locked",
            "--offline",
            "-p",
            "anthology-cli",
            "-e",
            "normal",
            "--target",
            "all",
            "--prefix",
            "none",
        ])
        .current_dir(&root)
        .output()
        .expect("run cargo tree");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "cargo tree failed: {stderr}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.lines().any(|l| l.starts_with("anthology-cli ")),
        "unexpected cargo tree output: {stdout}"
    );
    let hits: Vec<&str> = stdout
        .lines()
        .filter(|l| l.starts_with("serde_json "))
        .collect();
    assert!(hits.is_empty(), "serde_json in the shipped graph: {hits:?}");
}
