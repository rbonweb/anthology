//! `difftest freeze` and `difftest golden check|bless` end to end (CO2), without Python: the node
//! fake oracle of `tests/cli.rs` answers core.lock's seeds from the committed corpus. Also the
//! replay of the committed frozen goldens, which `cargo test` runs on Linux for good (design
//! §13.2: the goldens are frozen on Linux, in the oracle's 3.12 image).
#![forbid(unsafe_code)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_difftest");
const MANIFEST: &str = env!("CARGO_MANIFEST_DIR");

fn root() -> PathBuf {
    Path::new(MANIFEST).join("../..")
}

fn difftest(args: &[&str]) -> (i32, String) {
    let out = Command::new(BIN)
        .args(args)
        .current_dir(MANIFEST)
        .output()
        .expect("run difftest");
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.code().unwrap_or(-1), text)
}

/// A temporary folder with `python` (a wrapper running the fake oracle) and `golden/`; `None`
/// (skip) without node.
fn fake() -> Option<(tempfile::TempDir, String, String)> {
    let node = std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join("node"))
        .find(|p| p.is_file());
    let Some(node) = node else {
        eprintln!("skipped: node is not installed");
        return None;
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let script = Path::new(MANIFEST).join("tests/fake_oracle.js");
    let python = dir.path().join("python");
    std::fs::write(
        &python,
        format!(
            "#!/bin/sh\nexec '{}' '{}' \"$@\"\n",
            node.display(),
            script.display()
        ),
    )
    .expect("write");
    std::fs::set_permissions(&python, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let golden = dir.path().join("golden").to_string_lossy().into_owned();
    let python = python.to_string_lossy().into_owned();
    Some((dir, python, golden))
}

/// core.lock's golden under a `--golden DIR` (the repository's layout).
fn lock_golden(golden: &str) -> PathBuf {
    Path::new(golden).join("crates/anthology-core/tests/golden/core.lock.jsonl")
}

fn freeze_core_lock(python: &str, golden: &str) -> String {
    let (code, out) = difftest(&[
        "freeze",
        "--surface",
        "core.lock",
        "--gen",
        "0",
        "--python",
        python,
        "--golden",
        golden,
    ]);
    assert_eq!(code, 0, "{out}");
    let path = lock_golden(golden);
    assert!(
        out.contains(&format!("freeze: wrote {} (820 cases)", path.display())),
        "{out}"
    );
    std::fs::read_to_string(path).expect("golden")
}

#[test]
fn a_frozen_golden_checks_clean_without_the_oracle_and_catches_a_diff() {
    let Some((_dir, python, golden)) = fake() else {
        return;
    };
    let text = freeze_core_lock(&python, &golden);
    assert!(text.is_ascii());
    let mut lines = text.lines();
    assert_eq!(
        lines.next(),
        Some(
            "{\"difftest_golden\": 1, \"surface\": \"core.lock\", \"seed\": 20261121, \
             \"gen\": 0, \"cases\": 820}"
        )
    );
    assert!(
        lines
            .next()
            .is_some_and(|l| l.starts_with("{\"id\": \"golden:1\", \"case\": {"))
    );
    // Python is not needed any more: the interpreter does not even exist.
    let check = [
        "golden",
        "check",
        "--surface",
        "core.lock",
        "--golden",
        &golden,
    ];
    let (code, out) = difftest(&[&check[..], &["--python", "/nonexistent/python3"]].concat());
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains(
            "core.lock: 820 cases (820 seeds + 0 generated), 817 same, 3 explained (D-6 3)"
        ),
        "{out}"
    );
    let (code, out) = difftest(&[&check[..], &["--inject-diff", "core.lock"]].concat());
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("FAIL core.lock golden:1: unexplained diff"),
        "{out}"
    );
}

#[test]
fn bless_records_rust_for_a_changed_case_only() {
    let Some((_dir, python, golden)) = fake() else {
        return;
    };
    let text = freeze_core_lock(&python, &golden);
    let path = lock_golden(&golden);
    // An expectation that Rust no longer meets (as after a deliberate change).
    let second = text.lines().nth(2).expect("golden:2");
    assert!(second.ends_with("\"marks\": []}"), "{second}");
    let changed = second.replace("\"expect\": {\"exit\": ", "\"expect\": {\"exit\": 9");
    assert_ne!(changed, second);
    std::fs::write(&path, text.replacen(second, &changed, 1)).expect("write");
    let check = [
        "golden",
        "check",
        "--surface",
        "core.lock",
        "--golden",
        &golden,
    ];
    let (code, out) = difftest(&check);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("FAIL core.lock golden:2: unexplained diff"),
        "{out}"
    );
    let (code, out) = difftest(&[
        "golden",
        "bless",
        "--surface",
        "core.lock",
        "--golden",
        &golden,
    ]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("bless: core.lock: recorded Rust's result for 1 case(s)"),
        "{out}"
    );
    // Rust's result is what Python answered: the file is the frozen one again, byte for byte.
    assert_eq!(std::fs::read_to_string(&path).expect("read"), text);
    let (code, out) = difftest(&check);
    assert_eq!(code, 0, "{out}");
}

#[test]
fn after_the_cutover_every_ported_surface_needs_a_golden() {
    let Some((dir, python, golden)) = fake() else {
        return;
    };
    freeze_core_lock(&python, &golden);
    // A root without port/difftest/oracle/ (the Python is gone) but with the divergence files.
    let cut = dir.path().join("cut");
    let div = cut.join("port/difftest/divergences");
    std::fs::create_dir_all(&div).expect("mkdir");
    for e in std::fs::read_dir(root().join("port/difftest/divergences")).expect("ls") {
        let p = e.expect("entry").path();
        std::fs::copy(&p, div.join(p.file_name().expect("name"))).expect("copy");
    }
    let cut = cut.to_string_lossy().into_owned();
    let (code, out) = difftest(&[
        "golden", "check", "--all", "--golden", &golden, "--root", &cut,
    ]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("(no Python oracle: every ported surface needs a golden)"),
        "{out}"
    );
    assert!(out.contains("core.lock: 820 cases"), "{out}");
    assert!(out.contains("FAIL core.state: no golden at "), "{out}");
    // While the oracle exists, a surface without a golden is only skipped.
    let (code, out) = difftest(&["golden", "check", "--all", "--golden", &golden]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("skip core.state (no golden yet)"), "{out}");
}

#[test]
fn freeze_writes_nothing_for_a_surface_that_is_not_clean() {
    let Some((_dir, python, golden)) = fake() else {
        return;
    };
    // Generated cases are not in the fake's answers: oracle errors.
    let (code, out) = difftest(&[
        "freeze",
        "--surface",
        "core.lock",
        "--gen",
        "2",
        "--python",
        &python,
        "--golden",
        &golden,
    ]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("freeze: core.lock NOT frozen (2 unexplained, 2 oracle errors)"),
        "{out}"
    );
    assert!(!lock_golden(&golden).exists());
}

#[test]
fn golden_usage_errors_exit_2() {
    for (args, want) in [
        (&["golden", "--all"][..], "golden needs check or bless"),
        (
            &["golden", "check", "--all", "--gen", "5"][..],
            "--gen and --seed do not apply",
        ),
        (
            &["freeze", "--all", "--inject-diff", "core.lock"][..],
            "would freeze the diff",
        ),
        (&["--all", "--golden", "/tmp"][..], "--golden goes with"),
        (&["frieze", "--all"][..], "unexpected argument \"frieze\""),
        (&["freeze", "--list"][..], "exactly one of"),
        (
            &[
                "golden",
                "check",
                "--surface",
                "core.lock",
                "--golden",
                "/nonexistent",
            ][..],
            "core.lock: no golden at /nonexistent/crates/anthology-core/tests/golden/core.lock.jsonl",
        ),
    ] {
        let (code, out) = difftest(args);
        assert_eq!(code, 2, "{args:?}: {out}");
        assert!(out.contains(want), "{args:?}: {out}");
    }
}

/// The committed goldens (`crates/*/tests/golden/<surface>.jsonl`, written by `difftest freeze`
/// at CO2) replay clean. Before they exist every surface is a `skip`; after the cutover a ported
/// surface without one fails.
#[cfg(target_os = "linux")]
#[test]
fn the_committed_goldens_replay_clean() {
    let (code, out) = difftest(&["golden", "check", "--all"]);
    assert_eq!(code, 0, "{out}");
}
