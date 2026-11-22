//! Fork change 2 (rbonweb/rtk cedececf): `atl read` defaults to `--level minimal`
//! (comments stripped), while the `head`/`tail` rewrites ask for `--level none`
//! so a line window stays byte-exact.

mod common;

const SOURCE: &str = "import os\n# FORK2-COMMENT strip me\ndef go():\n    return os.sep\n";

fn read(args: &[&str], dir: &std::path::Path) -> String {
    let out = common::atl_command()
        .arg("read")
        .args(args)
        .current_dir(dir)
        .env("ATL_TELEMETRY_DISABLED", "1")
        .output()
        .expect("spawn atl read");
    assert!(out.status.success(), "atl read {args:?}: {out:?}");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn rewritten(command: &str) -> String {
    let out = common::atl_command()
        .args(["rewrite", command])
        .output()
        .expect("spawn atl rewrite");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn fork_2_read_strips_comments_by_default() {
    let dir = common::tempdir();
    std::fs::write(dir.path().join("sample.py"), SOURCE).expect("write sample");

    let default = read(&["sample.py"], dir.path());
    assert!(
        !default.contains("FORK2-COMMENT"),
        "the default level is minimal, which drops comments: {default}"
    );
    assert!(default.contains("def go():"), "{default}");

    let none = read(&["--level", "none", "sample.py"], dir.path());
    assert!(
        none.contains("FORK2-COMMENT"),
        "--level none keeps everything: {none}"
    );
}

#[test]
fn fork_2_head_and_tail_rewrites_stay_byte_exact() {
    for (raw, expected) in [
        (
            "head -20 src/main.rs",
            "atl read src/main.rs --level none --head-lines 20",
        ),
        (
            "tail -n 5 src/main.rs",
            "atl read src/main.rs --level none --tail-lines 5",
        ),
    ] {
        assert_eq!(rewritten(raw), expected, "rewrite of `{raw}`");
    }
}
