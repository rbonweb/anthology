//! The workspace and rtk build with the same Rust channel (GC-18, m-17). Only the channel must be
//! equal; the components and targets may differ.

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The `channel` of a rust-toolchain.toml's `[toolchain]` table.
fn channel(path: &Path) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut in_toolchain = false;
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') {
            in_toolchain = line == "[toolchain]";
        } else if in_toolchain
            && let Some((key, value)) = line.split_once('=')
            && key.trim() == "channel"
        {
            let value = value.trim();
            assert!(
                value.len() >= 2 && value.starts_with('"') && value.ends_with('"'),
                "{}: channel is not a basic string: {value}",
                path.display()
            );
            return value[1..value.len() - 1].to_string();
        }
    }
    panic!("{}: no [toolchain] channel", path.display());
}

#[test]
fn the_workspace_channel_equals_rtks() {
    let root = repo_root();
    let ours = channel(&root.join("rust-toolchain.toml"));
    let rtk = channel(&root.join("rtk/rust-toolchain.toml"));
    assert_eq!(ours, rtk, "rust-toolchain.toml and rtk/rust-toolchain.toml");
}

#[test]
fn the_parser_reads_only_the_toolchain_table() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rust-toolchain.toml");
    std::fs::write(
        &path,
        "[other]\nchannel = \"no\"\n\n[toolchain]\n# channel = \"comment\"\nchannel = \"1.2.3\" # pinned\n",
    )
    .unwrap();
    assert_eq!(channel(&path), "1.2.3");
}
