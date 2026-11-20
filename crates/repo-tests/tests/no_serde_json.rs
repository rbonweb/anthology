//! The shipped binary has no `serde_json`: JSON goes through `pycompat::json` only (GC-16, m-11).
//! cargo-deny bans are graph-wide, so this checks the normal-dependency tree of anthology-cli.

use std::path::Path;
use std::process::Command;

/// The release targets (design §10).
const SHIPPED: [&str; 4] = [
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
];

#[test]
fn the_anthology_cli_tree_has_no_serde_json() {
    let cargo = option_env!("CARGO").unwrap_or("cargo");
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    // Every shipped target; cargo may download the other targets' crates (not built).
    let out = Command::new(cargo)
        .args(["tree", "--locked", "-p", "anthology-cli", "-e", "normal"])
        .args(SHIPPED.iter().flat_map(|t| ["--target", t]))
        .args(["--prefix", "none"])
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
