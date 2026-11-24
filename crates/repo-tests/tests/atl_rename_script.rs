//! `scripts/atl-rename.sh`: the rename rule, run over a scratch tree. The old name is built
//! from the three constants below, so no fixture line has to carry it.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use repo_tests::repo;
use tempfile::TempDir;

const SCRIPT: &str = "scripts/atl-rename.sh";
const LOW: &str = "rtk"; // legacy
const UP: &str = "RTK"; // legacy
const TITLE: &str = "Rtk"; // legacy
const OLD_TITLE: &str = "Rust Token Killer"; // legacy

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn read(root: &Path, rel: &str) -> String {
    fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new("bash")
        .arg(repo::path(SCRIPT))
        .arg("--root")
        .arg(root)
        .args(args)
        .output()
        .expect("bash ran")
}

fn scratch() -> TempDir {
    tempfile::tempdir().unwrap()
}

#[test]
fn is_executable_with_strict_mode() {
    use std::os::unix::fs::PermissionsExt;
    let text = repo::read(SCRIPT);
    assert!(text.starts_with("#!"));
    assert!(text.contains("\nset -eu\n"));
    let mode = fs::metadata(repo::path(SCRIPT))
        .unwrap()
        .permissions()
        .mode();
    assert!(mode & 0o111 != 0, "{SCRIPT} is not executable");
}

#[test]
fn renames_every_spelling_and_keeps_the_case() {
    let dir = scratch();
    write(
        dir.path(),
        "src/main.rs",
        &format!(
            "{UP}_HOME {LOW} {TITLE}::new() Real{TITLE} {LOW}_text {LOW}-text {LOW}Foo ALLOW_{UP}\n\
             {LOW}s {LOW}x {LOW}probe {TITLE}Wiring {OLD_TITLE}\n"
        ),
    );
    let out = run(dir.path(), &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        read(dir.path(), "src/main.rs"),
        "ATL_HOME atl Atl::new() RealAtl atl_text atl-text atlFoo ALLOW_ATL\n\
         atls atlx atlprobe AtlWiring Anthology Token Layer\n"
    );
}

#[test]
fn renames_after_an_escaped_letter() {
    let dir = scratch();
    write(
        dir.path(),
        "esc.rs",
        &format!("\"a\\n{LOW} b\\t{LOW} c\\r{UP}\"\n"),
    );
    run(dir.path(), &[]);
    assert_eq!(read(dir.path(), "esc.rs"), "\"a\\natl b\\tatl c\\rATL\"\n");
}

#[test]
fn leaves_the_name_inside_a_longer_word_alone() {
    let dir = scratch();
    let line = format!("my{LOW} x{UP} 9{LOW} Real{UP}\n");
    write(dir.path(), "a.txt", &line);
    let out = run(dir.path(), &["--check"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(read(dir.path(), "a.txt"), line);
}

#[test]
fn keeps_the_upstream_names_and_the_published_tags() {
    let dir = scratch();
    let keep = format!(
        "github.com/{LOW}-ai/{LOW} {LOW}-ai.app rbonweb/{LOW} {UP} AI Labs {UP} Pro \
         {LOW}--v1.2.1-anthology.4 fork of {LOW} test_{LOW}_x\n"
    );
    write(
        dir.path(),
        "docs.md",
        &format!("{keep}use {LOW} now {UP} AI Labs\n"),
    );
    run(dir.path(), &[]);
    assert_eq!(
        read(dir.path(), "docs.md"),
        format!("{keep}use atl now {UP} AI Labs\n"),
        "the kept names stay and the rest of the line, and the next lines, still change"
    );
}

#[test]
fn renames_test_function_names_only_inside_the_binary_crate() {
    let dir = scratch();
    write(
        dir.path(),
        "atl/src/a.rs",
        &format!("fn test_{LOW}_init() {{}}\n"),
    );
    write(
        dir.path(),
        "crates/x/a.rs",
        &format!("fn test_{LOW}_init() {{}}\n"),
    );
    run(dir.path(), &[]);
    assert_eq!(read(dir.path(), "atl/src/a.rs"), "fn test_atl_init() {}\n");
    assert_eq!(
        read(dir.path(), "crates/x/a.rs"),
        format!("fn test_{LOW}_init() {{}}\n")
    );
}

#[test]
fn keeps_only_the_whole_product_name() {
    let dir = scratch();
    write(
        dir.path(),
        "p.md",
        &format!("{UP} Proxy and {UP} Pro, {UP} Pro-ish\n"),
    );
    run(dir.path(), &[]);
    assert_eq!(
        read(dir.path(), "p.md"),
        format!("ATL Proxy and {UP} Pro, {UP} Pro-ish\n")
    );
}

#[test]
fn renames_a_tag_that_was_not_published() {
    let dir = scratch();
    write(
        dir.path(),
        "t.txt",
        &format!("{LOW}--v1.2.1-anthology.6 {LOW}--v1.2.1-anthology.5\n"),
    );
    run(dir.path(), &[]);
    assert_eq!(
        read(dir.path(), "t.txt"),
        format!("atl--v1.2.1-anthology.6 {LOW}--v1.2.1-anthology.5\n")
    );
}

#[test]
fn leaves_a_line_with_a_legacy_comment_alone() {
    let dir = scratch();
    let text = format!(
        "\"{LOW}\" | \"atl\" // legacy\nx = \"{LOW}\" # legacy: old name\n<!-- legacy --> {LOW}\n\
         //! legacy: {LOW}\n/// legacy: {LOW}\nnext {LOW}\n"
    );
    write(dir.path(), "g.rs", &text);
    run(dir.path(), &[]);
    assert_eq!(
        read(dir.path(), "g.rs"),
        format!(
            "\"{LOW}\" | \"atl\" // legacy\nx = \"{LOW}\" # legacy: old name\n<!-- legacy --> {LOW}\n\
             //! legacy: {LOW}\n/// legacy: {LOW}\nnext atl\n"
        )
    );
}

#[test]
fn renames_a_line_that_only_mentions_the_word_legacy() {
    let dir = scratch();
    write(
        dir.path(),
        "g.md",
        &format!("the legacy shell hook runs {LOW} read\n"),
    );
    run(dir.path(), &[]);
    assert_eq!(
        read(dir.path(), "g.md"),
        "the legacy shell hook runs atl read\n"
    );
}

#[test]
fn skips_history_licenses_and_test_data_but_moves_the_folder() {
    let dir = scratch();
    let text = format!("{LOW} {UP} AI Labs {LOW}\n");
    for rel in [
        &format!("{LOW}/LICENSE"),
        &format!("{LOW}/CHANGELOG.md"),
        "CHANGELOG.md",
        "DECISIONS.md",
        "docs/history/PLAN.md",
        "port/notes.md",
        "docs/superpowers/plans/2026-11-20-rust-port.md",
        "crates/anthology-guard/tests/data/rules.jsonl",
    ] {
        write(dir.path(), rel, &text);
    }
    let out = run(dir.path(), &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    for rel in [
        "atl/LICENSE",
        "atl/CHANGELOG.md",
        "CHANGELOG.md",
        "DECISIONS.md",
        "docs/history/PLAN.md",
        "port/notes.md",
        "docs/superpowers/plans/2026-11-20-rust-port.md",
        "crates/anthology-guard/tests/data/rules.jsonl",
    ] {
        assert_eq!(read(dir.path(), rel), text, "{rel}");
    }
    assert!(!dir.path().join(LOW).exists(), "the old folder is gone");
}

#[test]
fn renames_file_and_folder_names() {
    let dir = scratch();
    write(dir.path(), &format!("{LOW}/.{LOW}/filters.toml"), "x\n");
    write(dir.path(), &format!("{LOW}/tests/{LOW}_layout.rs"), "y\n");
    write(
        dir.path(),
        &format!("{LOW}/hooks/claude/{LOW}-rewrite.sh"),
        "z\n",
    );
    run(dir.path(), &[]);
    assert_eq!(read(dir.path(), "atl/.atl/filters.toml"), "x\n");
    assert_eq!(read(dir.path(), "atl/tests/atl_layout.rs"), "y\n");
    assert_eq!(read(dir.path(), "atl/hooks/claude/atl-rewrite.sh"), "z\n");
    assert!(!dir.path().join(LOW).exists(), "the old folder is gone");
}

#[test]
fn leaves_binary_files_alone() {
    let dir = scratch();
    let bytes: Vec<u8> = [LOW.as_bytes(), &[0, 1, 2], LOW.as_bytes()].concat();
    fs::write(dir.path().join("blob.bin"), &bytes).unwrap();
    run(dir.path(), &[]);
    assert_eq!(fs::read(dir.path().join("blob.bin")).unwrap(), bytes);
}

#[test]
fn only_touches_the_paths_it_is_given() {
    let dir = scratch();
    write(dir.path(), "a/one.txt", &format!("{LOW}\n"));
    write(dir.path(), "b/two.txt", &format!("{LOW}\n"));
    run(dir.path(), &["a"]);
    assert_eq!(read(dir.path(), "a/one.txt"), "atl\n");
    assert_eq!(read(dir.path(), "b/two.txt"), format!("{LOW}\n"));
}

#[test]
fn check_changes_nothing_and_a_second_run_finds_nothing() {
    let dir = scratch();
    write(dir.path(), &format!("{LOW}/a.rs"), &format!("{LOW}\n"));
    let check = run(dir.path(), &["--check"]);
    assert_eq!(check.status.code(), Some(1));
    assert_eq!(read(dir.path(), &format!("{LOW}/a.rs")), format!("{LOW}\n"));
    assert!(run(dir.path(), &[]).status.success());
    assert_eq!(read(dir.path(), "atl/a.rs"), "atl\n");
    let again = run(dir.path(), &["--check"]);
    assert_eq!(
        again.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&again.stdout)
    );
}

#[test]
fn rejects_an_unknown_option() {
    let dir = scratch();
    let out = run(dir.path(), &["--nope"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("usage: scripts/atl-rename.sh"));
}
