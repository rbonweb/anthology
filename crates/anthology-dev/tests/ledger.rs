//! `anthology-dev ledger check` fails on every design §13.3 case (a missing row, a missing or
//! `#[ignore]`d Rust target, a lower row count, a `todo` row), on a Rust target cargo does not
//! compile or run, on a stale id list and on a regex pattern missing from the inventory;
//! `ledger ids --write` syncs the per-module files.

use std::ffi::OsString;
use std::fs;
use std::path::Path;

use anthology_dev::ledger::{self, CheckArgs, IdsArgs};

/// The fixture's `cargo test -- --list` output (both listings), kept at this path in the fixture.
const LISTING: &str = "target/listing.txt";
const DEFAULT_LISTING: &str = "   Compiling alpha v0.1.0 (/w/crates/alpha)
     Running tests/alpha.rs (/target/debug/build/alpha/1a/out/alpha-1a)
tables::test_table: test
test_one: test

2 tests, 0 benchmarks
";

/// A minimal repository: two Python modules, one Rust crate with one test file and a data file.
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path();
    put(
        root,
        "port/python-test-ids.txt",
        "test_alpha.AlphaTest.test_one\n\
         test_alpha.AlphaTest.test_table\n\
         test_alpha.AlphaTest.test_two\n\
         test_beta.BetaTest.test_three\n",
    );
    put(
        root,
        "port/python-test-rows.tsv",
        "test_alpha.AlphaTest.test_table\t3\n",
    );
    put(
        root,
        "port/regex-inventory.tsv",
        "# file\twhere\tconstructs\tpattern\n\
         scripts/a.py\t<module>:_X\tlookahead\t(?=x)\n",
    );
    put(
        root,
        "port/ledger/test_alpha.tsv",
        "# python_id\tstatus\trust_test\tnote\n\
         test_alpha.AlphaTest.test_one\tported\tcrates/alpha/tests/alpha.rs::test_one\n\
         test_alpha.AlphaTest.test_table\tported\tcrates/alpha/tests/alpha.rs::tables::test_table\n\
         test_alpha.AlphaTest.test_two\tmerged:test_alpha.AlphaTest.test_one\n",
    );
    put(
        root,
        "port/ledger/test_beta.tsv",
        "test_beta.BetaTest.test_three\tdropped:superseded:F-3\t\treplaced by shims.rs\n",
    );
    put(
        root,
        "crates/alpha/Cargo.toml",
        "[package]\nname = \"alpha\"\nversion = \"0.1.0\"\n\n[dependencies]\nname = \"not-this\"\n",
    );
    put(
        root,
        "crates/alpha/tests/alpha.rs",
        "#[test]\nfn test_one() {}\n\nmod tables {\n    #[test]\n    fn test_table() {}\n}\n",
    );
    put(
        root,
        "crates/alpha/tests/data/test_alpha.jsonl",
        "{\"py\": \"test_alpha.AlphaTest.test_table\", \"row\": 0}\n\
         {\"py\": \"test_alpha.AlphaTest.test_table\", \"row\": 1}\n\
         {\"py\": \"test_alpha.AlphaTest.test_table\", \"row\": 2}\n",
    );
    put(root, LISTING, DEFAULT_LISTING);
    dir
}

fn put(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, text).expect("write");
}

fn replace(root: &Path, rel: &str, from: &str, to: &str) {
    let path = root.join(rel);
    let text = fs::read_to_string(&path).expect("read");
    assert!(text.contains(from), "{rel} has no {from:?}");
    fs::write(path, text.replace(from, to)).expect("write");
}

fn append(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    let mut old = fs::read_to_string(&path).expect("read");
    old.push_str(text);
    fs::write(path, old).expect("write");
}

fn args(root: &Path, extra: &[&str]) -> Vec<OsString> {
    let mut out: Vec<OsString> = vec!["--root".into(), root.as_os_str().to_owned()];
    out.extend(extra.iter().map(OsString::from));
    out
}

/// A fresh extraction equal to the committed files, with the fixture listing as both listings.
fn fresh_copy(root: &Path) -> tempfile::TempDir {
    let fresh = tempfile::tempdir().expect("temp dir");
    for (from, to) in [
        (ledger::IDS_FILE, ledger::FRESH_IDS),
        (ledger::ROWS_FILE, ledger::FRESH_ROWS),
        (ledger::INVENTORY_FILE, ledger::FRESH_INVENTORY),
        (LISTING, ledger::FRESH_LIST_PLAIN),
        (LISTING, ledger::FRESH_LIST_ALL),
    ] {
        fs::copy(root.join(from), fresh.path().join(to)).expect("copy");
    }
    fresh
}

/// `(exit code, stdout, stderr)` of `ledger check --root <root> <extra>`. Without `--fresh` or
/// `--no-fresh` in `extra`, a fresh copy of the committed files and the listing is passed.
fn check(root: &Path, extra: &[&str]) -> (i32, String, String) {
    let given = extra.iter().any(|a| *a == "--fresh" || *a == "--no-fresh");
    let fresh = (!given).then(|| fresh_copy(root));
    let fresh_arg = fresh
        .as_ref()
        .map(|f| f.path().to_str().expect("utf-8").to_string());
    let mut all: Vec<&str> = extra.to_vec();
    if let Some(f) = &fresh_arg {
        all.extend(["--fresh", f.as_str()]);
    }
    let parsed = CheckArgs::parse(&args(root, &all)).expect("arguments");
    let out = ledger::check(&parsed);
    (
        out.code,
        String::from_utf8(out.stdout).expect("utf-8"),
        String::from_utf8(out.stderr).expect("utf-8"),
    )
}

fn assert_passes(root: &Path, extra: &[&str]) -> String {
    let (code, stdout, stderr) = check(root, extra);
    assert_eq!(code, 0, "{stderr}");
    stdout
}

fn assert_fails(root: &Path, extra: &[&str], needle: &str) {
    let (code, _, stderr) = check(root, extra);
    assert_eq!(
        code, 1,
        "expected a failure naming {needle:?}; stderr:\n{stderr}"
    );
    assert!(stderr.contains(needle), "no {needle:?} in:\n{stderr}");
}

const NOT_LISTED: &str = "is not in the `cargo test -- --list` output";

#[test]
fn a_complete_ledger_passes() {
    let dir = repo();
    let stdout = assert_passes(dir.path(), &[]);
    assert!(
        stdout.contains("ledger: ok (4 ids in 2 modules)"),
        "{stdout}"
    );
    // Statically too.
    assert_passes(dir.path(), &["--no-fresh"]);
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
    put(
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
fn a_test_inside_a_fn_body_or_macro_rules_is_not_a_target() {
    for wrapper in [
        ("fn outer() {\n", "}\n"),
        ("macro_rules! m {\n    () => {\n", "    };\n}\n"),
    ] {
        let dir = repo();
        replace(
            dir.path(),
            "crates/alpha/tests/alpha.rs",
            "#[test]\nfn test_one() {}\n",
            &format!("{}#[test]\nfn test_one() {{}}\n{}", wrapper.0, wrapper.1),
        );
        assert_fails(dir.path(), &[], "test_one is missing (no #[test] fn there)");
    }
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
    assert_fails(dir.path(), &[], "behind a cfg other than test or feature");
}

#[test]
fn a_target_switched_off_by_a_cfg_fails() {
    for cfg in [
        "#[cfg(any())]",
        "#[cfg(not(test))]",
        "#[cfg_attr(target_os = \"linux\", cfg(any()))]",
        "#[cfg_attr(test, cfg(any()))]",
    ] {
        let dir = repo();
        replace(
            dir.path(),
            "crates/alpha/tests/alpha.rs",
            "#[test]\nfn test_one",
            &format!("{cfg}\n#[test]\nfn test_one"),
        );
        // cargo would not list it either.
        replace(dir.path(), LISTING, "test_one: test\n", "");
        assert_fails(dir.path(), &[], "behind a cfg other than test or feature");
    }
}

#[test]
fn a_file_level_cfg_switches_off_every_target() {
    let dir = repo();
    let path = dir.path().join("crates/alpha/tests/alpha.rs");
    let text = fs::read_to_string(&path).expect("read");
    fs::write(&path, format!("#![cfg(any())]\n{text}")).expect("write");
    put(dir.path(), LISTING, "");
    assert_fails(dir.path(), &[], "behind a cfg other than test or feature");
}

#[test]
fn a_target_behind_an_undefined_feature_fails_through_the_listing() {
    let dir = repo();
    replace(
        dir.path(),
        "crates/alpha/tests/alpha.rs",
        "#[test]\nfn test_one",
        "#[cfg(feature = \"never-defined\")]\n#[test]\nfn test_one",
    );
    replace(dir.path(), LISTING, "test_one: test\n", "");
    assert_fails(dir.path(), &[], NOT_LISTED);
    // The static layer alone cannot see it; the listing is the binding check.
    assert_passes(dir.path(), &["--no-fresh"]);
}

#[test]
fn a_target_cargo_does_not_list_fails() {
    let dir = repo();
    replace(dir.path(), LISTING, "test_one: test\n", "");
    assert_fails(dir.path(), &[], NOT_LISTED);
    // A listing of another package's same-named file does not count.
    replace(dir.path(), LISTING, "build/alpha/", "build/beta/");
    assert_fails(dir.path(), &[], NOT_LISTED);
}

#[test]
fn a_missing_listing_fails() {
    let dir = repo();
    let fresh = fresh_copy(dir.path());
    fs::remove_file(fresh.path().join(ledger::FRESH_LIST_ALL)).expect("rm");
    let f = fresh.path().to_str().expect("utf-8");
    assert_fails(dir.path(), &["--fresh", f], "rust-tests-all.txt");
}

#[test]
fn a_target_in_a_file_no_target_compiles_fails() {
    // tests/<dir>/x.rs without tests/<dir>/main.rs; src/x.rs without `mod x;`; examples/.
    for (file, content) in [
        ("tests/orphan/x.rs", "#[test]\nfn test_one() {}\n"),
        ("src/orphan.rs", "#[test]\nfn test_one() {}\n"),
        ("examples/demo.rs", "#[test]\nfn test_one() {}\n"),
    ] {
        let dir = repo();
        put(dir.path(), "crates/alpha/src/lib.rs", "pub mod other;\n");
        put(dir.path(), &format!("crates/alpha/{file}"), content);
        replace(
            dir.path(),
            "port/ledger/test_alpha.tsv",
            "crates/alpha/tests/alpha.rs::test_one\n",
            &format!("crates/alpha/{file}::test_one\n"),
        );
        let (code, _, stderr) = check(dir.path(), &[]);
        assert_eq!(code, 1, "{file}: {stderr}");
        assert!(
            stderr.contains("is not compiled into a test target")
                || stderr.contains("is not in a test target"),
            "{file}: {stderr}"
        );
    }
}

#[test]
fn a_target_in_a_module_file_is_followed_from_its_root() {
    let dir = repo();
    put(
        dir.path(),
        "crates/alpha/src/lib.rs",
        "mod inner;\n#[cfg(test)]\nmod more { mod deep; }\n",
    );
    put(
        dir.path(),
        "crates/alpha/src/inner.rs",
        "#[cfg(test)]\nmod tests {\n    #[test]\n    fn test_one() {}\n}\n",
    );
    replace(
        dir.path(),
        "port/ledger/test_alpha.tsv",
        "crates/alpha/tests/alpha.rs::test_one\n",
        "crates/alpha/src/inner.rs::tests::test_one\n",
    );
    put(
        dir.path(),
        LISTING,
        &format!(
            "{DEFAULT_LISTING}     Running unittests src/lib.rs (/target/debug/build/alpha/2b/out/alpha-2b)\n\
             inner::tests::test_one: test\n"
        ),
    );
    assert_passes(dir.path(), &[]);
    // A platform cfg on the `mod` declaration hides the file on one OS.
    put(
        dir.path(),
        "crates/alpha/src/lib.rs",
        "#[cfg(target_os = \"macos\")]\nmod inner;\n",
    );
    assert_fails(dir.path(), &[], "`mod inner` is behind a cfg");
    // So does a missing declaration.
    put(dir.path(), "crates/alpha/src/lib.rs", "mod other;\n");
    assert_fails(dir.path(), &[], "no `mod inner;` in src/lib.rs reaches it");
}

#[test]
fn one_rust_test_cannot_port_two_python_tests() {
    let dir = repo();
    put(
        dir.path(),
        "port/python-test-ids.txt",
        "test_alpha.AlphaTest.test_one\n\
         test_alpha.AlphaTest.test_table\n\
         test_alpha.AlphaTest.test_two\n\
         test_beta.BetaTest.test_one\n\
         test_beta.BetaTest.test_three\n",
    );
    append(
        dir.path(),
        "port/ledger/test_beta.tsv",
        "test_beta.BetaTest.test_one\tported\tcrates/alpha/tests/alpha.rs::test_one\n",
    );
    assert_fails(
        dir.path(),
        &[],
        "rust_test crates/alpha/tests/alpha.rs::test_one is already the port of test_alpha.AlphaTest.test_one",
    );
    replace(
        dir.path(),
        "port/ledger/test_beta.tsv",
        "test_beta.BetaTest.test_one\tported\tcrates/alpha/tests/alpha.rs::test_one",
        "test_beta.BetaTest.test_one\tmerged:test_alpha.AlphaTest.test_one",
    );
    assert_passes(dir.path(), &[]);
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
    assert_fails(dir.path(), &[], "is not a path under crates/<crate>/");
}

#[test]
fn a_lower_row_count_fails_unless_the_rows_are_computed_with_a_reason() {
    let dir = repo();
    put(
        dir.path(),
        "crates/alpha/tests/data/test_alpha.jsonl",
        "{\"py\": \"test_alpha.AlphaTest.test_table\", \"row\": 0}\n",
    );
    assert_fails(dir.path(), &[], "1 distinct Rust rows tagged \"py\"");
    replace(
        dir.path(),
        "port/ledger/test_alpha.tsv",
        "tables::test_table\n",
        "tables::test_table\trows:computed\n",
    );
    assert_fails(dir.path(), &[], "`rows:computed` needs a reason");
    replace(
        dir.path(),
        "port/ledger/test_alpha.tsv",
        "rows:computed\n",
        "rows:computed iterates the same files\n",
    );
    let stdout = assert_passes(dir.path(), &["--report"]);
    assert!(
        stdout.contains("row counts waived (rows:computed): 1"),
        "{stdout}"
    );
    assert!(
        stdout.contains("test_alpha.AlphaTest.test_table\t3 rows\trows:computed iterates"),
        "{stdout}"
    );
}

#[test]
fn repeated_row_lines_do_not_count_as_rows() {
    let dir = repo();
    put(
        dir.path(),
        "crates/alpha/tests/data/test_alpha.jsonl",
        &"{\"py\": \"test_alpha.AlphaTest.test_table\", \"row\": 0}\n".repeat(3),
    );
    assert_fails(dir.path(), &[], "1 distinct Rust rows");
}

#[test]
fn rows_in_another_crate_do_not_count() {
    let dir = repo();
    let rows = fs::read_to_string(dir.path().join("crates/alpha/tests/data/test_alpha.jsonl"))
        .expect("read");
    fs::remove_file(dir.path().join("crates/alpha/tests/data/test_alpha.jsonl")).expect("rm");
    put(dir.path(), "crates/beta/tests/data/test_alpha.jsonl", &rows);
    assert_fails(
        dir.path(),
        &[],
        "0 distinct Rust rows tagged \"py\" in crates/alpha/tests/data/test_alpha.jsonl",
    );
}

#[test]
fn a_merged_test_with_rows_needs_its_rows_too() {
    let dir = repo();
    put(
        dir.path(),
        "port/python-test-rows.tsv",
        "test_alpha.AlphaTest.test_table\t3\ntest_alpha.AlphaTest.test_two\t2\n",
    );
    assert_fails(dir.path(), &[], "test_two: 0 distinct Rust rows");
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
    assert_passes(dir.path(), &["--files", "tests/test_alpha.py"]);
    // … nor the CI report while nobody has started that module.
    let stdout = assert_passes(dir.path(), &["--report"]);
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
fn a_deferred_row_needs_an_explicit_task_and_blocks_only_that_task() {
    let dir = repo();
    replace(
        dir.path(),
        "port/ledger/test_alpha.tsv",
        "test_alpha.AlphaTest.test_two\tmerged:test_alpha.AlphaTest.test_one",
        "test_alpha.AlphaTest.test_two\ttodo:MS2",
    );
    // The plan's bare DoD command fails: deferring must be checked as a named task.
    assert_fails(
        dir.path(),
        &["--files", "tests/test_alpha.py"],
        "deferred to MS2; a --files check of a module with deferred rows needs --task",
    );
    assert_passes(
        dir.path(),
        &["--files", "tests/test_alpha.py", "--task", "MS1"],
    );
    assert_fails(
        dir.path(),
        &["--files", "tests/test_alpha.py", "--task", "MS2"],
        "still todo (deferred to MS2)",
    );
    assert_fails(dir.path(), &[], "test_two: still todo (deferred to MS2)");
    // The CI report passes, and lists the deferred row.
    let stdout = assert_passes(dir.path(), &["--report"]);
    assert!(
        stdout.contains("rows deferred to a task (todo:<TASK>): 1"),
        "{stdout}"
    );
    assert!(
        stdout.contains("test_alpha.AlphaTest.test_two\ttodo:MS2"),
        "{stdout}"
    );
}

#[test]
fn a_deferred_row_must_name_a_task_of_the_plan() {
    let dir = repo();
    replace(
        dir.path(),
        "port/ledger/test_alpha.tsv",
        "test_alpha.AlphaTest.test_two\tmerged:test_alpha.AlphaTest.test_one",
        "test_alpha.AlphaTest.test_two\ttodo:ZZ9",
    );
    assert_fails(dir.path(), &["--report"], "needs a task of plan §4");
}

#[test]
fn the_task_list_matches_the_plan() {
    let plan = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/superpowers/plans/2026-11-20-rust-port.md");
    let text = fs::read_to_string(plan).expect("read the plan");
    let tasks_section = text
        .split("\n## 4. Tasks")
        .nth(1)
        .expect("plan §4")
        .split("\n## 5.")
        .next()
        .expect("§4 body");
    let mut headings: Vec<&str> = tasks_section
        .lines()
        .filter_map(|l| l.strip_prefix("### "))
        .filter_map(|l| l.split(':').next())
        .collect();
    headings.push("F4a"); // F4's heading names it: "F4a = ledger, ids and extraction".
    headings.sort_unstable();
    let mut ours = ledger::TASKS.to_vec();
    ours.sort_unstable();
    assert_eq!(ours, headings);
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
    put(
        dir.path(),
        "crates/difftest/src/surfaces/guard_check.rs",
        "pub const NAME: &str = \"guard.check\";\n",
    );
    assert_passes(dir.path(), &[]);
}

#[test]
fn stale_and_misplaced_rows_fail() {
    let dir = repo();
    append(
        dir.path(),
        "port/ledger/test_beta.tsv",
        "test_beta.BetaTest.test_gone\ttodo\ntest_alpha.AlphaTest.test_one\ttodo\n",
    );
    assert_fails(
        dir.path(),
        &[],
        "test_gone is not in port/python-test-ids.txt (a stale row)",
    );
    assert_fails(dir.path(), &[], "belongs in port/ledger/test_alpha.tsv");
    put(dir.path(), "port/ledger/test_gamma.tsv", "");
    assert_fails(
        dir.path(),
        &[],
        "test_gamma.tsv: no ids of module test_gamma",
    );
}

#[test]
fn malformed_rows_fail() {
    let dir = repo();
    append(
        dir.path(),
        "port/ledger/test_beta.tsv",
        "test_beta.BetaTest.test_three\tdone\n",
    );
    assert_fails(dir.path(), &[], "unknown status \"done\"");
}

#[test]
fn a_fresh_extraction_equal_to_the_committed_files_passes() {
    let dir = repo();
    let (code, _, stderr) = check(dir.path(), &[]);
    assert_eq!(code, 0, "{stderr}");
    assert!(!stderr.contains("--no-fresh"), "{stderr}");
}

#[test]
fn a_python_test_missing_from_the_id_list_fails() {
    let dir = repo();
    let fresh = fresh_copy(dir.path());
    append(
        fresh.path(),
        ledger::FRESH_IDS,
        "test_beta.BetaTest.test_zeta\n",
    );
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
    append(
        fresh.path(),
        ledger::FRESH_INVENTORY,
        "scripts/b.py\t<module>:_Y\tneg-lookbehind\t(?<!y)z\n",
    );
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
    put(
        fresh.path(),
        ledger::FRESH_ROWS,
        "test_alpha.AlphaTest.test_table\t4\n",
    );
    let f = fresh.path().to_str().expect("utf-8");
    assert_fails(dir.path(), &["--fresh", f], "row count not committed");
}

#[test]
fn ids_write_commits_the_fresh_files_and_syncs_every_ledger() {
    let dir = repo();
    let fresh = fresh_copy(dir.path());
    put(
        fresh.path(),
        ledger::FRESH_IDS,
        "test_alpha.AlphaTest.test_one\n\
         test_alpha.AlphaTest.test_table\n\
         test_alpha.AlphaTest.test_two\n\
         test_beta.BetaTest.test_three\n\
         test_delta.DeltaTest.test_four\n",
    );
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
    assert_eq!(
        dispatch(&["ledger", "check", "--no-fresh", "--files"]).code,
        2
    );
    assert_eq!(
        dispatch(&["ledger", "check", "--no-fresh", "--task", "ZZ9"]).code,
        2
    );
    assert_eq!(dispatch(&["ledger", "ids", "--write"]).code, 2);
    // Neither --fresh nor --no-fresh, or both: refused, so a bare check never passes quietly.
    let bare = dispatch(&["ledger", "check", "--files", "tests/test_x.py"]);
    assert_eq!(bare.code, 2);
    assert!(String::from_utf8_lossy(&bare.stderr).contains("needs --fresh DIR"));
    assert_eq!(
        dispatch(&["ledger", "check", "--fresh", "x", "--no-fresh"]).code,
        2
    );
}

#[test]
fn the_committed_ledger_is_structurally_sound() {
    // The repository's own ledger: every id has a row, and nothing but `todo` is pending.
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let stdout = assert_passes(&root, &["--report", "--no-fresh"]);
    assert!(stdout.contains("total"), "{stdout}");
}
