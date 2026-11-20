//! The `difftest` binary end to end, without Python: `tests/fake_oracle.js` (node) stands in
//! for the oracle and answers from the committed core.lock and core.state goldens. Covers the
//! harness rules: a diff fails, a matching entry passes, a wrong class fails, a bad divergence
//! file is a usage error, an oracle error is never explained. Skipped where node is missing.
#![forbid(unsafe_code)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_difftest");
const MANIFEST: &str = env!("CARGO_MANIFEST_DIR");

fn root() -> PathBuf {
    Path::new(MANIFEST).join("../..")
}

fn difftest(args: &[&str]) -> (i32, String) {
    let out: Output = Command::new(BIN)
        .args(args)
        .current_dir(MANIFEST)
        .output()
        .expect("run difftest");
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.code().unwrap_or(-1), text)
}

fn which(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

/// A temporary folder with `python`, a wrapper running the fake oracle, and `div/`, a copy of
/// the real divergence files. `None` (skip) without node.
fn fake() -> Option<(tempfile::TempDir, String, PathBuf)> {
    let Some(node) = which("node") else {
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
    let div = dir.path().join("div");
    std::fs::create_dir(&div).expect("mkdir");
    for f in ["core.lock.toml", "core.state.toml"] {
        std::fs::copy(
            root().join("port/difftest/divergences").join(f),
            div.join(f),
        )
        .expect("copy");
    }
    let python = python.to_string_lossy().into_owned();
    Some((dir, python, div))
}

#[test]
fn list_prints_every_surface() {
    let (code, out) = difftest(&["--list"]);
    assert_eq!(code, 0);
    let names: Vec<&str> = out.lines().collect();
    assert_eq!(names.len(), 37);
    assert_eq!(names[0], "pycompat.json");
    assert!(names.contains(&"core.lock") && names.contains(&"dev.smoke"));
}

#[test]
fn usage_errors_exit_2() {
    for (args, want) in [
        (&[][..], "exactly one of"),
        (&["--all", "--list"][..], "exactly one of"),
        (&["--surface", "nope"][..], "unknown surface \"nope\""),
        (
            &["--surface", "pycompat.str"][..],
            "pycompat.str is not ported yet",
        ),
        (&["--all", "--gen", "x"][..], "--gen"),
        (&["--all", "--bogus"][..], "--bogus"),
        (
            &["--all", "--inject-diff", "nope"][..],
            "--inject-diff: unknown surface",
        ),
    ] {
        let (code, out) = difftest(args);
        assert_eq!(code, 2, "{args:?}: {out}");
        assert!(out.contains(want), "{args:?}: {out}");
    }
    let (code, out) = difftest(&["--help"]);
    assert_eq!(code, 0);
    assert!(out.starts_with("usage: difftest --list"));
}

#[test]
fn the_seeds_pass_against_the_golden_answers() {
    let Some((dir, python, div)) = fake() else {
        return;
    };
    let report = dir.path().join("report.json");
    let div = div.to_string_lossy().into_owned();
    let (code, out) = difftest(&[
        "--surface",
        "core.lock,core.state",
        "--gen",
        "0",
        "--python",
        &python,
        "--divergences",
        &div,
        "--report",
        &report.to_string_lossy(),
    ]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("core.lock: 820 cases"), "{out}");
    assert!(
        out.contains("(D-6 3), 0 unexplained, 0 oracle errors"),
        "{out}"
    );
    assert!(
        out.contains("difftest: 0 unexplained diff(s) in 2 surface(s)"),
        "{out}"
    );
    let json = std::fs::read_to_string(&report).expect("report");
    assert!(json.contains("\"surface\": \"core.state\""), "{json}");
    assert!(json.contains("\"unexplained\": 0"), "{json}");
}

#[test]
fn an_injected_diff_fails_and_a_matching_entry_passes() {
    let Some((_dir, python, div)) = fake() else {
        return;
    };
    let d = div.to_string_lossy().into_owned();
    let args = [
        "--surface",
        "core.lock",
        "--gen",
        "0",
        "--python",
        &python,
        "--divergences",
        &d,
        "--inject-diff",
        "core.lock",
    ];
    let (code, out) = difftest(&args);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("core.lock: injected a diff into case golden:1"),
        "{out}"
    );
    assert!(
        out.contains("FAIL core.lock golden:1: unexplained diff"),
        "{out}"
    );
    assert!(
        out.contains("  diff:   stdout: first difference at byte 0"),
        "{out}"
    );
    let mut toml = std::fs::read_to_string(div.join("core.lock.toml")).expect("read");
    toml.push_str(
        "\n[[divergence]]\nclass = \"D-2\"\ncase = \"golden:1\"\nreason = \"injected\"\n",
    );
    std::fs::write(div.join("core.lock.toml"), toml).expect("write");
    let (code, out) = difftest(&args);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("(D-2 1, D-6 3), 0 unexplained"), "{out}");
}

#[test]
fn a_wrong_class_fails_and_an_unknown_key_is_an_error() {
    let Some((_dir, python, div)) = fake() else {
        return;
    };
    let d = div.to_string_lossy().into_owned();
    let args = [
        "--surface",
        "core.lock",
        "--gen",
        "0",
        "--python",
        &python,
        "--divergences",
        &d,
    ];
    std::fs::write(
        div.join("core.lock.toml"),
        "[[divergence]]\nclass = \"D-1\"\nreason = \"the wrong class\"\n",
    )
    .expect("write");
    let (code, out) = difftest(&args);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("the oracle marked D-6, but") && out.contains("declares no D-6 entry"),
        "{out}"
    );
    assert!(out.contains("3 unexplained"), "{out}");
    assert!(out.contains("warning: unused divergence entry"), "{out}");
    std::fs::write(
        div.join("core.lock.toml"),
        "[[divergence]]\nclass = \"D-6\"\nreason = \"r\"\nwhy = \"x\"\n",
    )
    .expect("write");
    let (code, out) = difftest(&args);
    assert_eq!(code, 2, "{out}");
    assert!(
        out.contains("core.lock.toml:4: unknown key \"why\""),
        "{out}"
    );
    std::fs::write(
        div.join("core.lock.toml"),
        "[[divergence]]\nclass = \"D-8\"\nreason = \"r\"\n",
    )
    .expect("write");
    let (code, out) = difftest(&args);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("is not a GC-16 class"), "{out}");
}

#[test]
fn oracle_errors_are_never_explained() {
    let Some((_dir, python, div)) = fake() else {
        return;
    };
    // Generated cases are not in the golden answers: the fake answers {"error": ...}. A
    // surface-wide waiver does not hide them.
    let mut toml = std::fs::read_to_string(div.join("core.lock.toml")).expect("read");
    toml.push_str("\n[[divergence]]\nclass = \"D-2\"\ncase = \"*\"\nreason = \"everything\"\n");
    std::fs::write(div.join("core.lock.toml"), toml).expect("write");
    let d = div.to_string_lossy().into_owned();
    let (code, out) = difftest(&[
        "--surface",
        "core.lock",
        "--gen",
        "3",
        "--python",
        &python,
        "--divergences",
        &d,
    ]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("3 unexplained, 3 oracle errors"), "{out}");
    assert!(
        out.contains("FAIL core.lock gen:1: oracle error: Traceback: fake oracle"),
        "{out}"
    );
}

#[test]
fn a_missing_interpreter_is_an_oracle_error_on_every_case() {
    let (code, out) = difftest(&[
        "--surface",
        "core.lock",
        "--gen",
        "0",
        "--python",
        "/nonexistent/python3",
    ]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("820 unexplained, 820 oracle errors"), "{out}");
    assert!(out.contains("cannot start /nonexistent/python3"), "{out}");
}
