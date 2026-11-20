//! `anthology-dev ledger check` fails on every design §13.3 case (a missing row, a missing or
//! `#[ignore]`d Rust target, a lower row count, a `todo` row), on a stale id list and on a regex
//! pattern missing from the inventory; `ledger ids --write` syncs the per-module files.

use std::ffi::OsString;
use std::fs;
use std::path::Path;

use anthology_dev::ledger::{self, CheckArgs, IdsArgs};

/// A minimal repository: two Python modules, one Rust test file, one data file.
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path();
    let put = |rel: &str, text: &str| {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, text).expect("write");
    };
    put(
        "port/python-test-ids.txt",
        "test_alpha.AlphaTest.test_one\n\
         test_alpha.AlphaTest.test_table\n\
         test_alpha.AlphaTest.test_two\n\
         test_beta.BetaTest.test_three\n",
    );
    put(
        "port/python-test-rows.tsv",
        "test_alpha.AlphaTest.test_table\t3\n",
    );
    put(
        "port/regex-inventory.tsv",
        "# file\twhere\tconstructs\tpattern\n\
         scripts/a.py\t<module>:_X\tlookahead\t(?=x)\n",
    );
    put(
        "port/ledger/test_alpha.tsv",
        "# python_id\tstatus\trust_test\tnote\n\
         test_alpha.AlphaTest.test_one\tported\tcrates/alpha/tests/alpha.rs::test_one\n\
         test_alpha.AlphaTest.test_table\tported\tcrates/alpha/tests/alpha.rs::tables::test_table\n\
         test_alpha.AlphaTest.test_two\tmerged:test_alpha.AlphaTest.test_one\n",
    );
    put(
        "port/ledger/test_beta.tsv",
        "test_beta.BetaTest.test_three\tdropped:superseded:F-3\t\treplaced by shims.rs\n",
    );
    put(
        "crates/alpha/tests/alpha.rs",
        "#[test]\nfn test_one() {}\n\nmod tables {\n    #[test]\n    fn test_table() {}\n}\n",
    );
    put(
        "crates/alpha/tests/data/test_alpha.jsonl",
        "{\"py\": \"test_alpha.AlphaTest.test_table\", \"row\": 0}\n\
         {\"py\": \"test_alpha.AlphaTest.test_table\", \"row\": 1}\n\
         {\"py\": \"test_alpha.AlphaTest.test_table\", \"row\": 2}\n",
    );
    dir
}

fn write(root: &Path, rel: &str, text: &str) {
    fs::write(root.join(rel), text).expect("write");
}

fn replace(root: &Path, rel: &str, from: &str, to: &str) {
    let path = root.join(rel);
    let text = fs::read_to_string(&path).expect("read");
    assert!(text.contains(from), "{rel} has no {from:?}");
    fs::write(path, text.replace(from, to)).expect("write");
}

fn args(root: &Path, extra: &[&str]) -> Vec<OsString> {
    let mut out: Vec<OsString> = vec!["--root".into(), root.as_os_str().to_owned()];
    out.extend(extra.iter().map(OsString::from));
    out
}

/// `(exit code, stdout, stderr)` of `ledger check --root <root> <extra>`.
fn check(root: &Path, extra: &[&str]) -> (i32, String, String) {
    let parsed = CheckArgs::parse(&args(root, extra)).expect("arguments");
    let out = ledger::check(&parsed);
    (
        out.code,
        String::from_utf8(out.stdout).expect("utf-8"),
        String::from_utf8(out.stderr).expect("utf-8"),
    )
}

fn assert_fails(root: &Path, extra: &[&str], needle: &str) {
    let (code, _, stderr) = check(root, extra);
    assert_eq!(
        code, 1,
        "expected a failure naming {needle:?}; stderr:\n{stderr}"
    );
    assert!(stderr.contains(needle), "no {needle:?} in:\n{stderr}");
}

#[test]
fn a_complete_ledger_passes() {
    let dir = repo();
    let (code, stdout, stderr) = check(dir.path(), &[]);
    assert_eq!(code, 0, "{stderr}");
    assert!(
        stdout.contains("ledger: ok (4 ids in 2 modules)"),
        "{stdout}"
    );
}

#[test]
fn a_missing_row_fails() {
    let dir = repo();
    replace(
        dir.path(),
        "port/ledger/test_alpha.tsv",
        "test_alpha.AlphaTest.test_two\tmerged:test_alpha.AlphaTest.test_one\n",
        "",
    );
    assert_fails(
        dir.path(),
        &[],
        "test_alpha.AlphaTest.test_two: missing row",
    );
}

#[test]
fn a_missing_ledger_file_fails() {
    let dir = repo();
    fs::remove_file(dir.path().join("port/ledger/test_beta.tsv")).expect("rm");
    assert_fails(dir.path(), &[], "port/ledger/test_beta.tsv: missing");
}

#[test]
fn a_missing_rust_target_fails() {
    let dir = repo();
    write(
        dir.path(),
        "crates/alpha/tests/alpha.rs",
        "mod tables {\n    #[test]\n    fn test_table() {}\n}\n",
    );
    assert_fails(
        dir.path(),
        &[],
        "crates/alpha/tests/alpha.rs::test_one is missing",
    );
    fs::remove_file(dir.path().join("crates/alpha/tests/alpha.rs")).expect("rm");
    assert_fails(
        dir.path(),
        &[],
        "Rust test file crates/alpha/tests/alpha.rs is missing",
    );
}

#[test]
fn a_helper_without_the_test_attribute_is_not_a_target() {
    let dir = repo();
    replace(
        dir.path(),
        "crates/alpha/tests/alpha.rs",
        "#[test]\nfn test_one",
        "fn test_one",
    );
    assert_fails(dir.path(), &[], "test_one is missing (no #[test] fn there)");
}

#[test]
fn an_ignored_rust_target_fails() {
    let dir = repo();
    replace(
        dir.path(),
        "crates/alpha/tests/alpha.rs",
        "#[test]\nfn test_one",
        "#[test]\n#[ignore]\nfn test_one",
    );
    assert_fails(dir.path(), &[], "test_one is #[ignore]d");
    replace(
        dir.path(),
        "crates/alpha/tests/alpha.rs",
        "#[ignore]",
        "#[cfg_attr(target_os = \"macos\", ignore)]",
    );
    assert_fails(dir.path(), &[], "test_one is #[ignore]d");
}

#[test]
fn a_platform_gated_rust_target_fails() {
    let dir = repo();
    replace(
        dir.path(),
        "crates/alpha/tests/alpha.rs",
        "mod tables {",
        "#[cfg(target_os = \"linux\")]\nmod tables {",
    );
    assert_fails(dir.path(), &[], "behind a platform cfg");
}

#[test]
fn a_rust_test_must_keep_the_python_method_name() {
    let dir = repo();
    replace(
        dir.path(),
        "port/ledger/test_alpha.tsv",
        "alpha.rs::test_one\n",
        "alpha.rs::renamed\n",
    );
    assert_fails(dir.path(), &[], "must keep the Python method name test_one");
}

#[test]
fn a_rust_test_path_must_stay_under_crates() {
    let dir = repo();
    replace(
        dir.path(),
        "port/ledger/test_alpha.tsv",
        "crates/alpha/tests/alpha.rs::test_one",
        "crates/../../elsewhere.rs::test_one",
    );
    assert_fails(dir.path(), &[], "is not a path under crates/");
}

#[test]
fn a_lower_row_count_fails_unless_the_rows_are_computed() {
    let dir = repo();
    write(
        dir.path(),
        "crates/alpha/tests/data/test_alpha.jsonl",
        "{\"py\": \"test_alpha.AlphaTest.test_table\", \"row\": 0}\n",
    );
    assert_fails(dir.path(), &[], "1 Rust rows tagged \"py\"");
    replace(
        dir.path(),
        "port/ledger/test_alpha.tsv",
        "tables::test_table\n",
        "tables::test_table\trows:computed (iterates the same files)\n",
    );
    let (code, _, stderr) = check(dir.path(), &[]);
    assert_eq!(code, 0, "{stderr}");
}

#[test]
fn a_merged_test_with_rows_needs_its_rows_too() {
    let dir = repo();
    write(
        dir.path(),
        "port/python-test-rows.tsv",
        "test_alpha.AlphaTest.test_table\t3\ntest_alpha.AlphaTest.test_two\t2\n",
    );
    assert_fails(dir.path(), &[], "test_two: 0 Rust rows");
}

#[test]
fn a_todo_row_fails() {
    let dir = repo();
    replace(
        dir.path(),
        "port/ledger/test_beta.tsv",
        "dropped:superseded:F-3\t\treplaced by shims.rs",
        "todo",
    );
    assert_fails(dir.path(), &[], "test_three: still todo");
    assert_fails(dir.path(), &["--files", "tests/test_beta.py"], "still todo");
    // Another lane's file does not block a per-lane check …
    let (code, _, stderr) = check(dir.path(), &["--files", "tests/test_alpha.py"]);
    assert_eq!(code, 0, "{stderr}");
    // … nor the CI report while nobody has started that module.
    let (code, stdout, stderr) = check(dir.path(), &["--report"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains("test_beta"), "{stdout}");
}

#[test]
fn a_todo_row_in_a_started_module_fails_the_report() {
    let dir = repo();
    replace(
        dir.path(),
        "port/ledger/test_alpha.tsv",
        "test_alpha.AlphaTest.test_two\tmerged:test_alpha.AlphaTest.test_one",
        "test_alpha.AlphaTest.test_two\ttodo",
    );
    assert_fails(dir.path(), &["--report"], "test_two: still todo");
}

#[test]
fn a_row_deferred_to_a_named_task_blocks_only_that_task_and_the_full_check() {
    let dir = repo();
    replace(
        dir.path(),
        "port/ledger/test_alpha.tsv",
        "test_alpha.AlphaTest.test_two\tmerged:test_alpha.AlphaTest.test_one",
        "test_alpha.AlphaTest.test_two\ttodo:MS2",
    );
    let (code, _, stderr) = check(
        dir.path(),
        &["--files", "tests/test_alpha.py", "--task", "MS1"],
    );
    assert_eq!(code, 0, "{stderr}");
    let (code, _, stderr) = check(dir.path(), &["--report"]);
    assert_eq!(code, 0, "{stderr}");
    assert_fails(
        dir.path(),
        &["--files", "tests/test_alpha.py", "--task", "MS2"],
        "still todo",
    );
    assert_fails(dir.path(), &[], "test_two: still todo");
}

#[test]
fn merged_must_point_at_a_ported_row() {
    let dir = repo();
    replace(
        dir.path(),
        "port/ledger/test_alpha.tsv",
        "merged:test_alpha.AlphaTest.test_one",
        "merged:test_beta.BetaTest.test_three",
    );
    assert_fails(dir.path(), &[], "which is not ported");
}

#[test]
fn a_dropped_row_needs_a_note() {
    let dir = repo();
    replace(
        dir.path(),
        "port/ledger/test_beta.tsv",
        "\t\treplaced by shims.rs",
        "",
    );
    assert_fails(dir.path(), &[], "a dropped row needs a note");
}

#[test]
fn a_golden_row_needs_a_known_corpus() {
    let dir = repo();
    replace(
        dir.path(),
        "port/ledger/test_beta.tsv",
        "dropped:superseded:F-3\t\treplaced by shims.rs",
        "golden:guard.check",
    );
    assert_fails(dir.path(), &[], "golden corpus guard.check");
    let surfaces = dir.path().join("crates/difftest/src/surfaces");
    fs::create_dir_all(&surfaces).expect("mkdir");
    fs::write(
        surfaces.join("guard_check.rs"),
        "pub const NAME: &str = \"guard.check\";\n",
    )
    .expect("write");
    let (code, _, stderr) = check(dir.path(), &[]);
    assert_eq!(code, 0, "{stderr}");
}

#[test]
fn stale_and_misplaced_rows_fail() {
    let dir = repo();
    let path = dir.path().join("port/ledger/test_beta.tsv");
    let mut text = fs::read_to_string(&path).expect("read");
    text.push_str("test_beta.BetaTest.test_gone\ttodo\n");
    text.push_str("test_alpha.AlphaTest.test_one\ttodo\n");
    fs::write(&path, text).expect("write");
    assert_fails(
        dir.path(),
        &[],
        "test_gone is not in port/python-test-ids.txt (a stale row)",
    );
    assert_fails(dir.path(), &[], "belongs in port/ledger/test_alpha.tsv");
    write(dir.path(), "port/ledger/test_gamma.tsv", "");
    assert_fails(
        dir.path(),
        &[],
        "test_gamma.tsv: no ids of module test_gamma",
    );
}

#[test]
fn malformed_rows_fail() {
    let dir = repo();
    let path = dir.path().join("port/ledger/test_beta.tsv");
    let mut text = fs::read_to_string(&path).expect("read");
    text.push_str("test_beta.BetaTest.test_three\tdone\n");
    fs::write(&path, text).expect("write");
    assert_fails(dir.path(), &[], "unknown status \"done\"");
}

/// A fresh extraction equal to the committed files.
fn fresh_copy(root: &Path) -> tempfile::TempDir {
    let fresh = tempfile::tempdir().expect("temp dir");
    for (from, to) in [
        (ledger::IDS_FILE, ledger::FRESH_IDS),
        (ledger::ROWS_FILE, ledger::FRESH_ROWS),
        (ledger::INVENTORY_FILE, ledger::FRESH_INVENTORY),
    ] {
        fs::copy(root.join(from), fresh.path().join(to)).expect("copy");
    }
    fresh
}

#[test]
fn a_fresh_extraction_equal_to_the_committed_files_passes() {
    let dir = repo();
    let fresh = fresh_copy(dir.path());
    let f = fresh.path().to_str().expect("utf-8");
    let (code, _, stderr) = check(dir.path(), &["--fresh", f]);
    assert_eq!(code, 0, "{stderr}");
    assert!(
        !stderr.contains("not compared with a fresh extraction"),
        "{stderr}"
    );
}

#[test]
fn a_python_test_missing_from_the_id_list_fails() {
    let dir = repo();
    let fresh = fresh_copy(dir.path());
    let mut ids = fs::read_to_string(fresh.path().join(ledger::FRESH_IDS)).expect("read");
    ids.push_str("test_beta.BetaTest.test_zeta\n");
    fs::write(fresh.path().join(ledger::FRESH_IDS), ids).expect("write");
    let f = fresh.path().to_str().expect("utf-8");
    assert_fails(
        dir.path(),
        &["--fresh", f],
        "Python test not in the id list: test_beta.BetaTest.test_zeta",
    );
}

#[test]
fn an_unlisted_lookaround_pattern_fails() {
    let dir = repo();
    let fresh = fresh_copy(dir.path());
    let path = fresh.path().join(ledger::FRESH_INVENTORY);
    let mut inventory = fs::read_to_string(&path).expect("read");
    inventory.push_str("scripts/b.py\t<module>:_Y\tneg-lookbehind\t(?<!y)z\n");
    fs::write(&path, inventory).expect("write");
    let f = fresh.path().to_str().expect("utf-8");
    assert_fails(
        dir.path(),
        &["--fresh", f],
        "lookaround or backreference pattern not in the inventory: scripts/b.py",
    );
}

#[test]
fn a_changed_row_count_fails() {
    let dir = repo();
    let fresh = fresh_copy(dir.path());
    fs::write(
        fresh.path().join(ledger::FRESH_ROWS),
        "test_alpha.AlphaTest.test_table\t4\n",
    )
    .expect("write");
    let f = fresh.path().to_str().expect("utf-8");
    assert_fails(dir.path(), &["--fresh", f], "row count not committed");
}

#[test]
fn ids_write_commits_the_fresh_files_and_syncs_every_ledger() {
    let dir = repo();
    let fresh = fresh_copy(dir.path());
    fs::write(
        fresh.path().join(ledger::FRESH_IDS),
        "test_alpha.AlphaTest.test_one\n\
         test_alpha.AlphaTest.test_table\n\
         test_alpha.AlphaTest.test_two\n\
         test_beta.BetaTest.test_three\n\
         test_delta.DeltaTest.test_four\n",
    )
    .expect("write");
    let f = fresh.path().to_str().expect("utf-8");
    let a = IdsArgs::parse(&args(dir.path(), &["--fresh", f])).expect("arguments");
    assert_eq!(ledger::ids(&a).code, 1, "a stale list is reported");
    let a = IdsArgs::parse(&args(dir.path(), &["--fresh", f, "--write"])).expect("arguments");
    let out = ledger::ids(&a);
    assert_eq!(out.code, 0, "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(
        fs::read_to_string(dir.path().join("port/ledger/test_delta.tsv")).expect("read"),
        "# python_id\tstatus\trust_test\tnote\ntest_delta.DeltaTest.test_four\ttodo\n"
    );
    // The new id is todo, so the full check fails on it and on nothing else.
    let (code, _, stderr) = check(dir.path(), &["--fresh", f]);
    assert_eq!(code, 1);
    assert!(stderr.contains("test_four: still todo"), "{stderr}");
    assert!(
        stderr.contains("anthology-dev ledger: 1 problems"),
        "{stderr}"
    );
}

#[test]
fn usage_errors_exit_2() {
    let dispatch = |a: &[&str]| {
        let a: Vec<OsString> = a.iter().map(OsString::from).collect();
        anthology_dev::dispatch(&a, &mut std::io::empty())
    };
    assert_eq!(dispatch(&["ledger"]).code, 2);
    assert_eq!(dispatch(&["ledger", "nope"]).code, 2);
    assert_eq!(dispatch(&["ledger", "check", "--bogus"]).code, 2);
    assert_eq!(dispatch(&["ledger", "check", "--files"]).code, 2);
    assert_eq!(dispatch(&["ledger", "check", "--task", "lane-x"]).code, 2);
    assert_eq!(dispatch(&["ledger", "ids", "--write"]).code, 2);
}

#[test]
fn the_committed_ledger_is_structurally_sound() {
    // The repository's own ledger: every id has a row, and nothing but `todo` is pending.
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let (code, stdout, stderr) = check(&root, &["--report"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains("total"), "{stdout}");
}
