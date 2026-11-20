//! `anthology-dev ledger check` fails on every design §13.3 case (a missing row, a missing or
//! `#[ignore]`d Rust target, a lower row count, a `todo` row), on a Rust target cargo does not
//! compile or run, on a stale id list and on a regex pattern missing from the inventory;
//! `ledger ids --write` syncs the per-module files.

use std::ffi::OsString;
use std::fs;
use std::path::Path;

use anthology_dev::ledger::{self, CheckArgs, IdsArgs};

/// The fixture's `cargo test -- --list` output (both listings), kept at this path in the fixture,
/// and its `-- --list --ignored` output (both ignored listings).
const LISTING: &str = "target/listing.txt";
const IGNORED: &str = "target/ignored.txt";
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
        "test_memory_sync.AlphaTest.test_one\n\
         test_memory_sync.AlphaTest.test_table\n\
         test_memory_sync.AlphaTest.test_two\n\
         test_rtk_lock.BetaTest.test_three\n",
    );
    put(
        root,
        "port/python-test-rows.tsv",
        "test_memory_sync.AlphaTest.test_table\t3\n",
    );
    put(
        root,
        "port/regex-inventory.tsv",
        "# file\twhere\tconstructs\tpattern\n\
         scripts/a.py\t<module>:_X\tlookahead\t(?=x)\n",
    );
    put(
        root,
        "port/ledger/test_memory_sync.tsv",
        "# python_id\tstatus\trust_test\tnote\n\
         test_memory_sync.AlphaTest.test_one\tported\tcrates/alpha/tests/alpha.rs::test_one\n\
         test_memory_sync.AlphaTest.test_table\tported\tcrates/alpha/tests/alpha.rs::tables::test_table\n\
         test_memory_sync.AlphaTest.test_two\tmerged:test_memory_sync.AlphaTest.test_one\t\tcovered by test_one's table\n",
    );
    put(
        root,
        "port/ledger/test_rtk_lock.tsv",
        "test_rtk_lock.BetaTest.test_three\tdropped:superseded:F-3\t\treplaced by shims.rs\n",
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
        "crates/alpha/tests/data/test_memory_sync.jsonl",
        "{\"py\": \"test_memory_sync.AlphaTest.test_table\", \"row\": 0}\n\
         {\"py\": \"test_memory_sync.AlphaTest.test_table\", \"row\": 1}\n\
         {\"py\": \"test_memory_sync.AlphaTest.test_table\", \"row\": 2}\n",
    );
    put(root, LISTING, DEFAULT_LISTING);
    put(
        root,
        IGNORED,
        "     Running tests/alpha.rs (/target/debug/build/alpha/1a/out/alpha-1a)\n\n\
         0 tests, 0 benchmarks\n",
    );
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
        (IGNORED, ledger::FRESH_LIST_IGNORED_PLAIN),
        (IGNORED, ledger::FRESH_LIST_IGNORED_ALL),
    ] {
        fs::copy(root.join(from), fresh.path().join(to)).expect("copy");
    }
    fresh
}

/// `(exit code, stdout, stderr)` of `ledger check --root <root> <extra>`. Without `--fresh` or
/// `--no-fresh` in `extra`, a fresh copy of the committed files and the listing is passed.
fn check(root: &Path, extra: &[&str]) -> (i32, String, String) {
    let given = extra
        .iter()
        .any(|a| ["--fresh", "--no-fresh", "--listings"].contains(a));
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
        "port/ledger/test_memory_sync.tsv",
        "test_memory_sync.AlphaTest.test_two\tmerged:test_memory_sync.AlphaTest.test_one\t\tcovered by test_one's table\n",
        "",
    );
    assert_fails(
        dir.path(),
        &[],
        "test_memory_sync.AlphaTest.test_two: missing row",
    );
}

#[test]
fn a_missing_ledger_file_fails() {
    let dir = repo();
    fs::remove_file(dir.path().join("port/ledger/test_rtk_lock.tsv")).expect("rm");
    assert_fails(dir.path(), &[], "port/ledger/test_rtk_lock.tsv: missing");
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
fn a_target_cargo_lists_as_ignored_fails() {
    // An ignore the source does not show (a macro, an attribute the scanner cannot read).
    let dir = repo();
    put(
        dir.path(),
        IGNORED,
        "     Running tests/alpha.rs (/target/debug/build/alpha/1a/out/alpha-1a)\n\
         test_one: test\n\n1 test, 0 benchmarks\n",
    );
    assert_fails(
        dir.path(),
        &[],
        "Rust test crates/alpha/tests/alpha.rs::test_one is ignored on linux: `cargo test -- --list --ignored` lists it",
    );
    let fresh = fresh_copy(dir.path());
    fs::remove_file(fresh.path().join(ledger::FRESH_LIST_IGNORED_PLAIN)).expect("rm");
    let f = fresh.path().to_str().expect("utf-8");
    assert_fails(dir.path(), &["--fresh", f], "rust-tests-ignored-plain.txt");
}

#[test]
fn a_target_inside_an_untrusted_macro_fails() {
    // `ig!` could add #[ignore] or a platform cfg in its expansion.
    let dir = repo();
    replace(
        dir.path(),
        "crates/alpha/tests/alpha.rs",
        "#[test]\nfn test_one() {}\n",
        "macro_rules! ig { ($i:item) => { #[cfg(target_os = \"linux\")] $i }; }\n\
         ig! { #[test] fn test_one() {} }\n",
    );
    assert_fails(
        dir.path(),
        &[],
        "is inside `ig!`, whose expansion the ledger cannot check",
    );
    // proptest! is trusted.
    let dir = repo();
    replace(
        dir.path(),
        "crates/alpha/tests/alpha.rs",
        "#[test]\nfn test_one() {}\n",
        "proptest::proptest! {\n    #[test]\n    fn test_one(x in 0..3u8) { let _ = x; }\n}\n",
    );
    assert_passes(dir.path(), &[]);
}

#[test]
fn a_module_moved_by_a_cfg_attr_path_fails() {
    let dir = repo();
    put(
        dir.path(),
        "crates/alpha/tests/a/main.rs",
        "#[cfg_attr(all(), path = \"real.rs\")]\nmod m;\n",
    );
    put(
        dir.path(),
        "crates/alpha/tests/a/m.rs",
        "#[test]\nfn test_one() {}\n",
    );
    replace(
        dir.path(),
        "port/ledger/test_memory_sync.tsv",
        "crates/alpha/tests/alpha.rs::test_one\n",
        "crates/alpha/tests/a/m.rs::test_one\n",
    );
    put(
        dir.path(),
        LISTING,
        &format!(
            "{DEFAULT_LISTING}     Running tests/a/main.rs (/target/debug/build/alpha/3c/out/a-3c)\n\
             m::test_one: test\n"
        ),
    );
    assert_fails(
        dir.path(),
        &[],
        "uses #[path], which the ledger does not follow",
    );
}

/// A listing-only directory (one platform's four listing files) from the fixture listings.
fn listings_dir(root: &Path) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp dir");
    for (from, to) in [
        (LISTING, ledger::FRESH_LIST_PLAIN),
        (LISTING, ledger::FRESH_LIST_ALL),
        (IGNORED, ledger::FRESH_LIST_IGNORED_PLAIN),
        (IGNORED, ledger::FRESH_LIST_IGNORED_ALL),
    ] {
        fs::copy(root.join(from), dir.path().join(to)).expect("copy");
    }
    dir
}

#[test]
fn a_platform_cfg_on_a_trusted_macro_invocation_fails() {
    let dir = repo();
    replace(
        dir.path(),
        "crates/alpha/tests/alpha.rs",
        "#[test]\nfn test_one() {}\n",
        "#[cfg(target_os = \"linux\")]\nproptest::proptest! {\n    #[test]\n    \
         fn test_one(x in 0..3u8) { let _ = x; }\n}\n",
    );
    assert_fails(dir.path(), &[], "behind a cfg other than test or feature");
}

#[test]
fn an_untrusted_macro_around_a_trusted_one_fails() {
    // `wrap!` is not defined in the crate (an imported macro), so only the rule that every
    // enclosing invocation must be trusted refuses it.
    let dir = repo();
    replace(
        dir.path(),
        "crates/alpha/tests/alpha.rs",
        "#[test]\nfn test_one() {}\n",
        "wrap! {\n    proptest::proptest! {\n        #[test]\n        \
         fn test_one(x in 0..3u8) { let _ = x; }\n    }\n}\n",
    );
    assert_fails(
        dir.path(),
        &[],
        "is inside `wrap!`, whose expansion the ledger cannot check",
    );
}

#[test]
fn a_local_macro_named_like_a_trusted_one_is_not_trusted() {
    let dir = repo();
    replace(
        dir.path(),
        "crates/alpha/tests/alpha.rs",
        "#[test]\nfn test_one() {}\n",
        "proptest! {\n    #[test]\n    fn test_one(x in 0..3u8) { let _ = x; }\n}\n",
    );
    // Defined in another file of the same crate.
    put(
        dir.path(),
        "crates/alpha/src/lib.rs",
        "#[macro_export]\nmacro_rules! proptest {\n    ($i:item) => { #[cfg(target_os = \"linux\")] $i };\n}\n",
    );
    assert_fails(
        dir.path(),
        &[],
        "but crates/alpha/src/lib.rs defines its own `macro_rules! proptest`",
    );
}

#[test]
fn the_listing_only_mode_checks_one_platform() {
    let dir = repo();
    let listings = listings_dir(dir.path());
    let l = listings.path().to_str().expect("utf-8");
    let stdout = assert_passes(dir.path(), &["--listings", l, "--platform", "macos"]);
    assert!(stdout.contains("ledger: ok"), "{stdout}");
    // Not compiled on macOS.
    replace(dir.path(), LISTING, "test_one: test\n", "");
    let listings = listings_dir(dir.path());
    let l = listings.path().to_str().expect("utf-8");
    assert_fails(
        dir.path(),
        &["--listings", l, "--platform", "macos"],
        "is not in the `cargo test -- --list` output on macos",
    );
    // Ignored on macOS.
    let dir = repo();
    put(
        dir.path(),
        IGNORED,
        "     Running tests/alpha.rs (/target/debug/build/alpha/1a/out/alpha-1a)\n\
         test_one: test\n",
    );
    let listings = listings_dir(dir.path());
    let l = listings.path().to_str().expect("utf-8");
    assert_fails(
        dir.path(),
        &["--listings", l, "--platform", "macos"],
        "is ignored on macos",
    );
}

/// The fixture's Python source for `test_memory_sync`, with `test_one` decorated by `decorator`
/// and `body` as its body (the ledger never reads it any more: only the exception table counts).
fn python_source(root: &Path, decorator: &str, body: &str) {
    put(
        root,
        "tests/test_memory_sync.py",
        &format!(
            "import sys\nimport unittest\nfrom unittest import mock\n\n\n\
             class AlphaTest(unittest.TestCase):\n    {decorator}\n    def test_one(self):\n        \
             {body}\n\n    def test_two(self):\n        pass\n"
        ),
    );
}

/// A Linux-only Rust port of `test_one` whose row says `platform:linux`.
fn linux_only_port(root: &Path) {
    replace(
        root,
        "crates/alpha/tests/alpha.rs",
        "#[test]\nfn test_one",
        "#[cfg(target_os = \"linux\")]\n#[test]\nfn test_one",
    );
    replace(
        root,
        "port/ledger/test_memory_sync.tsv",
        "crates/alpha/tests/alpha.rs::test_one\n",
        "crates/alpha/tests/alpha.rs::test_one\tplatform:linux (the Python test is Linux-only)\n",
    );
}

const EXCEPTION_NEEDED: &str = "`platform:linux` needs the pair (test_memory_sync.AlphaTest.test_one, linux) in port/ledger/platform-exceptions.tsv";

#[test]
fn a_platform_note_needs_a_listed_exception_whatever_the_python_says() {
    // The re-review's cases: none of them makes the note acceptable, because no exception row
    // lists the pair (the Python source is not read at all).
    for (decorator, body) in [
        ("# no decorator", "pass"),
        (
            "@unittest.skipUnless(sys.platform.startswith(\"linux\"), \"Linux only\")",
            "pass",
        ),
        (
            "@unittest.skipUnless(LOCK.exists(), \"no lock\")  # not sys.platform",
            "pass",
        ),
        ("@unittest.skipIf(False, \"sys.platform\")", "pass"),
        (
            "@unittest.skipUnless(sys.platform == \"darwin\", \"macOS only\")",
            "pass",
        ),
        (
            "@unittest.skipIf(sys.platform == \"win32\", \"not on Windows\")",
            "pass",
        ),
        (
            "# body",
            "self.skipTest(\"no lock\")\n        _ = sys.platform",
        ),
        ("@mock.patch(\"sys.platform\", \"linux\")", "pass"),
    ] {
        let dir = repo();
        linux_only_port(dir.path());
        python_source(dir.path(), decorator, body);
        assert_fails(dir.path(), &[], EXCEPTION_NEEDED);
    }
}

#[test]
fn a_listed_exception_allows_one_platform_and_an_unlisted_pair_fails() {
    let dir = repo();
    linux_only_port(dir.path());
    // test_two cannot stay merged into a one-platform test (see below): drop it here.
    replace(
        dir.path(),
        "port/ledger/test_memory_sync.tsv",
        "test_two\tmerged:test_memory_sync.AlphaTest.test_one\t\tcovered by test_one's table\n",
        "test_two\tdropped:py-compat\t\tfixture: not under test here\n",
    );
    // The pair for the other platform does not count.
    put(
        dir.path(),
        "port/ledger/platform-exceptions.tsv",
        "# python_id\tplatform\treason\n\
         test_memory_sync.AlphaTest.test_one\tmacos\tthe Python test is macOS-only\n",
    );
    assert_fails(dir.path(), &[], EXCEPTION_NEEDED);
    put(
        dir.path(),
        "port/ledger/platform-exceptions.tsv",
        "# python_id\tplatform\treason\n\
         test_memory_sync.AlphaTest.test_one\tlinux\tthe Python test is Linux-only\n",
    );
    assert_passes(dir.path(), &[]);
    // Linux checks it; macOS does not need it.
    replace(dir.path(), LISTING, "test_one: test\n", "");
    let listings = listings_dir(dir.path());
    let l = listings.path().to_str().expect("utf-8");
    assert_passes(dir.path(), &["--listings", l, "--platform", "macos"]);
    assert_fails(
        dir.path(),
        &[],
        "is not in the `cargo test -- --list` output on linux",
    );
    // A typo is not a platform.
    replace(
        dir.path(),
        "port/ledger/test_memory_sync.tsv",
        "platform:linux",
        "platform:win",
    );
    assert_fails(
        dir.path(),
        &[],
        "`platform:win` is not platform:linux or platform:macos",
    );
}

#[test]
fn the_exception_table_is_validated() {
    for (row, needle) in [
        (
            "test_memory_sync.AlphaTest.test_gone\tlinux\tx",
            "is not in port/python-test-ids.txt",
        ),
        (
            "test_memory_sync.AlphaTest.test_one\twindows\tx",
            "windows is not linux or macos",
        ),
        (
            "test_memory_sync.AlphaTest.test_one\tlinux\t ",
            "an exception needs a reason",
        ),
        (
            "test_memory_sync.AlphaTest.test_one\tlinux",
            "not `python_id<TAB>platform<TAB>reason`",
        ),
    ] {
        let dir = repo();
        put(
            dir.path(),
            "port/ledger/platform-exceptions.tsv",
            &format!("# python_id\tplatform\treason\n{row}\n"),
        );
        assert_fails(dir.path(), &[], needle);
    }
    // It is not a module ledger.
    let dir = repo();
    put(
        dir.path(),
        "port/ledger/platform-exceptions.tsv",
        "# python_id\tplatform\treason\n",
    );
    assert_passes(dir.path(), &[]);
}

#[test]
fn a_merged_row_cannot_be_platform_limited() {
    // It cannot carry the note …
    let dir = repo();
    replace(
        dir.path(),
        "port/ledger/test_memory_sync.tsv",
        "covered by test_one's table",
        "covered by test_one's table platform:linux",
    );
    assert_fails(
        dir.path(),
        &[],
        "a merged row cannot carry a `platform:` note",
    );
    // … nor ride on a target's (pn_merged), even a listed exception.
    let dir = repo();
    linux_only_port(dir.path());
    put(
        dir.path(),
        "port/ledger/platform-exceptions.tsv",
        "test_memory_sync.AlphaTest.test_one\tlinux\tthe Python test is Linux-only\n",
    );
    assert_fails(
        dir.path(),
        &[],
        "merged into test_memory_sync.AlphaTest.test_one, whose Rust test runs on one platform only",
    );
}

#[test]
fn the_committed_exception_table_is_empty() {
    let table = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../port/ledger/platform-exceptions.tsv"),
    )
    .expect("read the table");
    assert!(table.contains("CONTROLLER-OWNED"), "{table}");
    assert!(
        table
            .lines()
            .all(|l| l.starts_with('#') || l.trim().is_empty()),
        "{table}"
    );
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
            "port/ledger/test_memory_sync.tsv",
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
        "port/ledger/test_memory_sync.tsv",
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
        "test_memory_sync.AlphaTest.test_one\n\
         test_memory_sync.AlphaTest.test_table\n\
         test_memory_sync.AlphaTest.test_two\n\
         test_memory_sync.OtherTest.test_one\n\
         test_rtk_lock.BetaTest.test_three\n",
    );
    append(
        dir.path(),
        "port/ledger/test_memory_sync.tsv",
        "test_memory_sync.OtherTest.test_one\tported\tcrates/alpha/tests/alpha.rs::test_one\n",
    );
    assert_fails(
        dir.path(),
        &[],
        "rust_test crates/alpha/tests/alpha.rs::test_one is already the port of test_memory_sync.AlphaTest.test_one",
    );
    replace(
        dir.path(),
        "port/ledger/test_memory_sync.tsv",
        "test_memory_sync.OtherTest.test_one\tported\tcrates/alpha/tests/alpha.rs::test_one",
        "test_memory_sync.OtherTest.test_one\tmerged:test_memory_sync.AlphaTest.test_one\t\t\
         the same Rust test, inherited",
    );
    assert_passes(dir.path(), &[]);
}

#[test]
fn a_rust_test_must_keep_the_python_method_name() {
    let dir = repo();
    replace(
        dir.path(),
        "port/ledger/test_memory_sync.tsv",
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
        "port/ledger/test_memory_sync.tsv",
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
        "crates/alpha/tests/data/test_memory_sync.jsonl",
        "{\"py\": \"test_memory_sync.AlphaTest.test_table\", \"row\": 0}\n",
    );
    assert_fails(dir.path(), &[], "1 distinct Rust rows tagged \"py\"");
    replace(
        dir.path(),
        "port/ledger/test_memory_sync.tsv",
        "tables::test_table\n",
        "tables::test_table\trows:computed\n",
    );
    assert_fails(dir.path(), &[], "`rows:computed` needs a reason");
    replace(
        dir.path(),
        "port/ledger/test_memory_sync.tsv",
        "rows:computed\n",
        "rows:computed iterates the same files\n",
    );
    let stdout = assert_passes(dir.path(), &["--report"]);
    assert!(
        stdout.contains("row counts waived (rows:computed): 1"),
        "{stdout}"
    );
    assert!(
        stdout.contains("test_memory_sync.AlphaTest.test_table\t3 rows\trows:computed iterates"),
        "{stdout}"
    );
}

#[test]
fn repeated_row_lines_do_not_count_as_rows() {
    let dir = repo();
    put(
        dir.path(),
        "crates/alpha/tests/data/test_memory_sync.jsonl",
        &"{\"py\": \"test_memory_sync.AlphaTest.test_table\", \"row\": 0}\n".repeat(3),
    );
    assert_fails(dir.path(), &[], "1 distinct Rust rows");
}

#[test]
fn rows_in_another_crate_do_not_count() {
    let dir = repo();
    let rows = fs::read_to_string(
        dir.path()
            .join("crates/alpha/tests/data/test_memory_sync.jsonl"),
    )
    .expect("read");
    fs::remove_file(
        dir.path()
            .join("crates/alpha/tests/data/test_memory_sync.jsonl"),
    )
    .expect("rm");
    put(
        dir.path(),
        "crates/beta/tests/data/test_memory_sync.jsonl",
        &rows,
    );
    assert_fails(
        dir.path(),
        &[],
        "0 distinct Rust rows tagged \"py\" in crates/alpha/tests/data/test_memory_sync.jsonl",
    );
}

#[test]
fn a_merged_test_with_rows_needs_its_rows_too() {
    let dir = repo();
    put(
        dir.path(),
        "port/python-test-rows.tsv",
        "test_memory_sync.AlphaTest.test_table\t3\ntest_memory_sync.AlphaTest.test_two\t2\n",
    );
    assert_fails(dir.path(), &[], "test_two: 0 distinct Rust rows");
}

#[test]
fn a_todo_row_fails() {
    let dir = repo();
    replace(
        dir.path(),
        "port/ledger/test_rtk_lock.tsv",
        "dropped:superseded:F-3\t\treplaced by shims.rs",
        "todo",
    );
    assert_fails(dir.path(), &[], "test_three: still todo");
    assert_fails(
        dir.path(),
        &["--files", "tests/test_rtk_lock.py"],
        "still todo",
    );
    // Another lane's file does not block a per-lane check …
    assert_passes(dir.path(), &["--files", "tests/test_memory_sync.py"]);
    // … nor the CI report while nobody has started that module.
    let stdout = assert_passes(dir.path(), &["--report"]);
    assert!(stdout.contains("test_rtk_lock"), "{stdout}");
}

#[test]
fn a_todo_row_in_a_started_module_fails_the_report() {
    let dir = repo();
    replace(
        dir.path(),
        "port/ledger/test_memory_sync.tsv",
        "test_memory_sync.AlphaTest.test_two\tmerged:test_memory_sync.AlphaTest.test_one",
        "test_memory_sync.AlphaTest.test_two\ttodo",
    );
    assert_fails(dir.path(), &["--report"], "test_two: still todo");
}

#[test]
fn a_deferred_row_needs_an_explicit_task_and_blocks_only_that_task() {
    let dir = repo();
    replace(
        dir.path(),
        "port/ledger/test_memory_sync.tsv",
        "test_memory_sync.AlphaTest.test_two\tmerged:test_memory_sync.AlphaTest.test_one",
        "test_memory_sync.AlphaTest.test_two\ttodo:MS2",
    );
    // The plan's bare DoD command fails: deferring must be checked as a named task.
    assert_fails(
        dir.path(),
        &["--files", "tests/test_memory_sync.py"],
        "deferred to MS2; a --files check of a module with deferred rows needs --task",
    );
    assert_passes(
        dir.path(),
        &["--files", "tests/test_memory_sync.py", "--task", "MS1"],
    );
    assert_fails(
        dir.path(),
        &["--files", "tests/test_memory_sync.py", "--task", "MS2"],
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
        stdout.contains("test_memory_sync.AlphaTest.test_two\ttodo:MS2"),
        "{stdout}"
    );
}

#[test]
fn a_deferred_row_must_name_an_owner_of_its_module() {
    let dir = repo();
    // MT1 is a task of the plan, but test_memory_sync belongs to MS1 and MS2 (plan §5).
    replace(
        dir.path(),
        "port/ledger/test_memory_sync.tsv",
        "test_memory_sync.AlphaTest.test_two\tmerged:test_memory_sync.AlphaTest.test_one",
        "test_memory_sync.AlphaTest.test_two\ttodo:MT1",
    );
    assert_fails(
        dir.path(),
        &["--report"],
        "deferred to MT1, which does not own test_memory_sync (plan §5 owners: MS1, MS2)",
    );
    // And a lane cannot check a module it does not own.
    assert_fails(
        dir.path(),
        &["--files", "tests/test_memory_sync.py", "--task", "MT1"],
        "--task MT1 does not own test_memory_sync",
    );
}

#[test]
fn the_module_owners_match_the_plan() {
    let plan = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/superpowers/plans/2026-11-20-rust-port.md");
    let text = fs::read_to_string(plan).expect("read the plan");
    let table = text
        .split("\n## 5. Ledger accounting summary")
        .nth(1)
        .expect("plan §5")
        .split("\n## 6.")
        .next()
        .expect("§5 body");
    let mut from_plan: Vec<(String, Vec<String>)> = Vec::new();
    for line in table.lines().filter(|l| l.starts_with("| test_")) {
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        let owners: Vec<String> = cells[3]
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|w| ledger::TASKS.contains(w))
            .map(str::to_string)
            .collect();
        for module in cells[1].split(',').map(str::trim) {
            from_plan.push((module.to_string(), owners.clone()));
        }
    }
    from_plan.sort();
    let mut ours: Vec<(String, Vec<String>)> = ledger::MODULE_OWNERS
        .iter()
        .map(|(m, o)| (m.to_string(), o.iter().map(|t| t.to_string()).collect()))
        .collect();
    ours.sort();
    assert_eq!(ours, from_plan);
    // Every module of the committed id list has an owner.
    let ids = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../port/python-test-ids.txt"),
    )
    .expect("read the id list");
    for id in ids.lines() {
        let module = id.split('.').next().expect("module");
        assert!(
            !ledger::owners_of(module).is_empty(),
            "{module} has no owner"
        );
    }
}

#[test]
fn a_deferred_row_must_name_a_task_of_the_plan() {
    let dir = repo();
    replace(
        dir.path(),
        "port/ledger/test_memory_sync.tsv",
        "test_memory_sync.AlphaTest.test_two\tmerged:test_memory_sync.AlphaTest.test_one",
        "test_memory_sync.AlphaTest.test_two\ttodo:ZZ9",
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
fn merged_must_point_at_a_ported_row_of_the_same_module_and_say_how() {
    let dir = repo();
    replace(
        dir.path(),
        "port/ledger/test_memory_sync.tsv",
        "test_memory_sync.AlphaTest.test_one\tported\tcrates/alpha/tests/alpha.rs::test_one",
        "test_memory_sync.AlphaTest.test_one\tdropped:py-compat\t\tno Rust equivalent",
    );
    assert_fails(dir.path(), &[], "which is not ported");

    let dir = repo();
    replace(
        dir.path(),
        "port/ledger/test_memory_sync.tsv",
        "merged:test_memory_sync.AlphaTest.test_one",
        "merged:test_rtk_lock.BetaTest.test_three",
    );
    assert_fails(dir.path(), &[], "which is in another module");

    let dir = repo();
    replace(
        dir.path(),
        "port/ledger/test_memory_sync.tsv",
        "covered by test_one's table",
        "",
    );
    assert_fails(dir.path(), &[], "a merged row needs a note");

    // The report lists every merged row with its note.
    let dir = repo();
    let stdout = assert_passes(dir.path(), &["--report"]);
    assert!(
        stdout.contains("rows merged into another test (merged:): 1"),
        "{stdout}"
    );
    assert!(
        stdout.contains("test_two\tmerged:test_memory_sync.AlphaTest.test_one\tcovered by"),
        "{stdout}"
    );
}

#[test]
fn a_dropped_row_needs_a_note() {
    let dir = repo();
    replace(
        dir.path(),
        "port/ledger/test_rtk_lock.tsv",
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
        "port/ledger/test_rtk_lock.tsv",
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
        "port/ledger/test_rtk_lock.tsv",
        "test_rtk_lock.BetaTest.test_gone\ttodo\ntest_memory_sync.AlphaTest.test_one\ttodo\n",
    );
    assert_fails(
        dir.path(),
        &[],
        "test_gone is not in port/python-test-ids.txt (a stale row)",
    );
    assert_fails(
        dir.path(),
        &[],
        "belongs in port/ledger/test_memory_sync.tsv",
    );
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
        "port/ledger/test_rtk_lock.tsv",
        "test_rtk_lock.BetaTest.test_three\tdone\n",
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
        "test_rtk_lock.BetaTest.test_zeta\n",
    );
    let f = fresh.path().to_str().expect("utf-8");
    assert_fails(
        dir.path(),
        &["--fresh", f],
        "Python test not in the id list: test_rtk_lock.BetaTest.test_zeta",
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
        "test_memory_sync.AlphaTest.test_table\t4\n",
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
        "test_memory_sync.AlphaTest.test_one\n\
         test_memory_sync.AlphaTest.test_table\n\
         test_memory_sync.AlphaTest.test_two\n\
         test_rtk_lock.BetaTest.test_three\n\
         test_zz_new.DeltaTest.test_four\n",
    );
    let f = fresh.path().to_str().expect("utf-8");
    let a = IdsArgs::parse(&args(dir.path(), &["--fresh", f])).expect("arguments");
    assert_eq!(ledger::ids(&a).code, 1, "a stale list is reported");
    let a = IdsArgs::parse(&args(dir.path(), &["--fresh", f, "--write"])).expect("arguments");
    let out = ledger::ids(&a);
    assert_eq!(out.code, 0, "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(
        fs::read_to_string(dir.path().join("port/ledger/test_zz_new.tsv")).expect("read"),
        "# python_id\tstatus\trust_test\tnote\ntest_zz_new.DeltaTest.test_four\ttodo\n"
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
    assert_eq!(
        dispatch(&["ledger", "check", "--listings", "x"]).code,
        2,
        "--listings needs --platform"
    );
    assert_eq!(
        dispatch(&[
            "ledger",
            "check",
            "--listings",
            "x",
            "--platform",
            "windows"
        ])
        .code,
        2
    );
    assert_eq!(
        dispatch(&["ledger", "check", "--no-fresh", "--platform", "macos"]).code,
        2
    );
    assert_eq!(
        dispatch(&["ledger", "check", "--fresh", "x", "--listings", "y"]).code,
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
