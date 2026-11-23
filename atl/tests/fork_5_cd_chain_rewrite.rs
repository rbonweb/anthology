//! Fork change 5 (rbonweb/rtk cedececf): a leading `cd` stays raw and does not stop
//! the rest of the chain from being rewritten.
//!
//! The fork's change here was a regression test only (`registry.rs`
//! `test_rewrite_cd_does_not_poison_next_segment`): upstream already rewrote the
//! segments after a `cd`. This pins that behaviour end to end, so an upstream
//! change that breaks it fails here.

mod common;

fn rewritten(command: &str) -> String {
    let out = common::atl_command()
        .args(["rewrite", command])
        .output()
        .expect("spawn atl rewrite");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn fork_5_cd_does_not_poison_the_next_segment() {
    for (raw, expected) in [
        ("cd /tmp && grep foo bar", "cd /tmp && atl grep foo bar"),
        ("cd /tmp; git status", "cd /tmp; atl git status"),
    ] {
        assert_eq!(rewritten(raw), expected, "rewrite of `{raw}`");
    }
}

#[test]
fn fork_5_a_bare_cd_is_left_alone() {
    assert_eq!(rewritten("cd /tmp"), "");
}
