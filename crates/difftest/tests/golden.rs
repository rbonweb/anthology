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
        .env_remove("CI")
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

/// A golden file's text (the files are gzip).
fn read_golden(path: &Path) -> String {
    use std::io::Read;
    let bytes = std::fs::read(path).expect("read the golden");
    let mut text = String::new();
    flate2::read::GzDecoder::new(bytes.as_slice())
        .read_to_string(&mut text)
        .expect("gunzip the golden");
    text
}

/// Writes `text` as a golden file (gzip).
fn write_golden(path: &Path, text: &str) {
    use std::io::Write;
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(text.as_bytes()).expect("gzip");
    std::fs::write(path, gz.finish().expect("gzip")).expect("write the golden");
}

/// core.lock's golden under a `--golden DIR` (the repository's layout).
fn lock_golden(golden: &str) -> PathBuf {
    Path::new(golden).join("crates/anthology-core/tests/golden/core.lock.jsonl.gz")
}

fn freeze_core_lock(python: &str, golden: &str) -> String {
    let (code, out) = difftest(&[
        "freeze",
        "--sample",
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
    read_golden(&path)
}

#[test]
fn a_frozen_golden_checks_clean_without_the_oracle_and_catches_a_diff() {
    let Some((_dir, python, golden)) = fake() else {
        return;
    };
    let text = freeze_core_lock(&python, &golden);
    assert!(text.is_ascii());
    let mut lines = text.lines();
    // The header names the day of the freeze, which the replay pins Rust's clock to.
    let header = lines.next().expect("header");
    let day = header
        .strip_prefix(
            "{\"difftest_golden\": 1, \"surface\": \"core.lock\", \"seed\": 20261121, \
             \"gen\": 0, \"cases\": 820, \"sample\": true, \"today\": \"",
        )
        .and_then(|rest| rest.strip_suffix("\"}"))
        .unwrap_or_else(|| panic!("header: {header}"));
    assert_eq!(day.len(), 10, "{header}");
    assert!(
        day.bytes().enumerate().all(|(i, b)| if i == 4 || i == 7 {
            b == b'-'
        } else {
            b.is_ascii_digit()
        }),
        "{header}"
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
    write_golden(&path, &text.replacen(second, &changed, 1));
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
    assert_eq!(read_golden(&path), text);
    let (code, out) = difftest(&check);
    assert_eq!(code, 0, "{out}");
}

#[test]
fn a_waived_case_pins_rust_and_drift_fails() {
    let Some((dir, python, golden)) = fake() else {
        return;
    };
    let text = freeze_core_lock(&python, &golden);
    let path = lock_golden(&golden);
    // As if Python had answered golden:2 differently and an entry waived it at the freeze:
    // the line keeps Python's answer as "expect" and Rust's own result as "rust".
    let second = text.lines().nth(2).expect("golden:2");
    let (head, expect) = second.split_once("\"expect\": ").expect("expect");
    let (expect, tail) = expect.split_once(", \"marks\": ").expect("marks");
    let python_said = expect.replacen("{\"exit\": ", "{\"exit\": 7", 1);
    let pinned = |rust: &str| {
        format!(
            "{head}\"expect\": {python_said}, \"marks\": {}, \"rust\": {rust}}}",
            tail.trim_end_matches('}')
        )
    };
    let div = dir.path().join("div");
    std::fs::create_dir_all(&div).expect("mkdir");
    let mut toml = std::fs::read_to_string(root().join("port/difftest/divergences/core.lock.toml"))
        .expect("read");
    toml.push_str("\n[[divergence]]\nclass = \"D-2\"\ncase = \"golden:2\"\nreason = \"t\"\n");
    std::fs::write(div.join("core.lock.toml"), toml).expect("write");
    let d = div.to_string_lossy().into_owned();
    let check = [
        "golden",
        "check",
        "--surface",
        "core.lock",
        "--golden",
        &golden,
        "--divergences",
        &d,
    ];
    write_golden(&path, &text.replacen(second, &pinned(expect), 1));
    let (code, out) = difftest(&check);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("core.lock: waived 1 diff(s) by "), "{out}");
    // Rust's result differs from the pinned one: the waiver no longer hides it.
    let drifted = expect.replacen("{\"exit\": ", "{\"exit\": 8", 1);
    write_golden(&path, &text.replacen(second, &pinned(&drifted), 1));
    let (code, out) = difftest(&check);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains(
            "FAIL core.lock golden:2: a waived diff: Rust's result changed since the freeze"
        ),
        "{out}"
    );
    // bless records Rust's result as the expectation and drops the pin.
    let bless = [
        "golden",
        "bless",
        "--surface",
        "core.lock",
        "--golden",
        &golden,
        "--divergences",
        &d,
    ];
    let (code, out) = difftest(&bless);
    assert_eq!(code, 0, "{out}");
    assert_eq!(read_golden(&path), text);
    // And bless refuses in CI.
    let out = Command::new(BIN)
        .args(bless)
        .env("CI", "true")
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("refuses to run in CI"));
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
    // core.lock's seed corpus stays after the cutover (the golden check counts the seeds).
    let seeds = cut.join("crates/anthology-core/tests/golden");
    std::fs::create_dir_all(&seeds).expect("mkdir");
    std::fs::copy(
        root().join("crates/anthology-core/tests/golden/lock.jsonl"),
        seeds.join("lock.jsonl"),
    )
    .expect("copy");
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
    // While the oracle exists, a surface without a golden is only skipped. The same scratch root
    // with an oracle driver stands for a checkout before the cutover, so this half also holds
    // after it.
    let driver = Path::new(&cut).join("port/difftest/oracle/_driver.py");
    std::fs::create_dir_all(driver.parent().expect("parent")).expect("mkdir");
    std::fs::write(&driver, "").expect("write");
    let (code, out) = difftest(&[
        "golden", "check", "--all", "--golden", &golden, "--root", &cut,
    ]);
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
        "--sample",
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
            &["freeze", "--all", "--gen", "999"][..],
            "freeze needs --gen 1000 or more",
        ),
        (&["--all", "--sample"][..], "--sample goes with freeze"),
        (
            &[
                "golden",
                "check",
                "--surface",
                "core.lock",
                "--golden",
                "/nonexistent",
            ][..],
            "core.lock: no golden at /nonexistent/crates/anthology-core/tests/golden/core.lock.jsonl.gz",
        ),
    ] {
        let (code, out) = difftest(args);
        assert_eq!(code, 2, "{args:?}: {out}");
        assert!(out.contains(want), "{args:?}: {out}");
    }
}

/// The committed goldens (`crates/*/tests/golden/<surface>.jsonl.gz`, written by `difftest freeze`
/// at CO2) replay clean. Before they exist every surface is a `skip`; after the cutover a ported
/// surface without one fails.
/// The committed goldens stay small enough for the repository (CO2-prep ruling): at most 5 MB
/// per file and 20 MB in all.
#[test]
fn the_committed_goldens_fit_the_size_caps() {
    const FILE_CAP: u64 = 5_000_000;
    const TOTAL_CAP: u64 = 20_000_000;
    let mut total = 0;
    for krate in std::fs::read_dir(root().join("crates")).expect("ls crates") {
        let dir = krate.expect("entry").path().join("tests/golden");
        let Ok(files) = std::fs::read_dir(&dir) else {
            continue;
        };
        for f in files {
            let p = f.expect("entry").path();
            if p.to_string_lossy().ends_with(".jsonl.gz") {
                let n = std::fs::metadata(&p).expect("stat").len();
                assert!(n <= FILE_CAP, "{}: {n} bytes", p.display());
                total += n;
            }
        }
    }
    assert!(total <= TOTAL_CAP, "the goldens hold {total} bytes");
}

#[cfg(target_os = "linux")]
#[test]
fn the_committed_goldens_replay_clean() {
    let (code, out) = difftest(&["golden", "check", "--all"]);
    assert_eq!(code, 0, "{out}");
}
