//! CO2: CI and the maintainer tools once the Python is gone (plan CO2; design §14.2; GC-15
//! F-16, F-17). The replacements the ledger names for the Python-pinning parts of test_ci,
//! test_evals_layout, test_repo_layout, test_rtk_layout and test_rtk_claude_only.
//!
//! Scripts CO2 moves out of `port/staging/` are read through `anthology_testkit::staged`; a test
//! that only holds once the Python is deleted skips while `port/staging/` exists.

use std::collections::BTreeSet;

use repo_tests::repo;

/// Whether CO2 has moved `port/staging/` into place (the switch of `anthology_testkit::staged`).
fn cut_over() -> bool {
    !repo::path("port/staging").is_dir()
}

fn staged(rel: &str) -> String {
    let p = anthology_testkit::staged(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// The lines of a shell script that are not comments.
fn code_lines(text: &str) -> Vec<&str> {
    text.lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect()
}

fn runs_no_python(rel: &str, text: &str) {
    for line in code_lines(text) {
        assert!(!line.to_lowercase().contains("python"), "{rel}: {line}");
    }
}

/// test_evals_layout `test_moved_cases_are_smokes_not_cases`: the Rust smoke runner (not
/// smoke.py) registers the two smokes that moved out of the eval cases.
#[test]
fn the_smoke_runner_registers_rtk_rewrite_and_agent_memory_path() {
    let names = anthology_dev::smoke::names();
    for want in ["rtk-rewrite", "agent-memory-path"] {
        assert!(names.contains(&want), "{want} in {names:?}");
    }
}

/// test_rtk_layout `test_records_the_merged_upstream_pin`: sync-rtk.sh records the new pin with
/// `scripts/dev.sh vendor set-rtk-upstream` and embeds no python3.
#[test]
fn sync_rtk_records_the_pin_through_dev_sh() {
    let rel = "scripts/sync-rtk.sh";
    let text = staged(rel);
    assert!(
        text.contains("scripts/dev.sh vendor set-rtk-upstream --ref \"$tag\" --commit"),
        "{rel}"
    );
    runs_no_python(rel, &text);
}

/// test_rtk_claude_only `test_trim_entry_is_executable_strict_and_runs_the_python_trim`: the trim
/// runs as `scripts/dev.sh rtk-trim`; after the cutover `scripts/rtk_trim.py` is gone.
#[test]
fn rtk_trim_runs_the_trim_through_dev_sh() {
    let rel = "scripts/rtk-trim.sh";
    let text = staged(rel);
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(anthology_testkit::staged(rel))
        .expect("stat")
        .permissions()
        .mode();
    assert!(mode & 0o111 != 0, "{rel} is executable");
    assert!(code_lines(&text).contains(&"set -eu"), "{rel}: set -eu");
    assert!(
        text.contains("\"$root/scripts/dev.sh\" rtk-trim --root rtk"),
        "{rel}"
    );
    runs_no_python(rel, &text);
    if cut_over() {
        assert!(!repo::path("scripts/rtk_trim.py").exists());
    }
}

/// test_repo_layout `test_dockerfiles_pinned_by_digest`, the file set: after the cutover
/// `ci/images` holds exactly these Dockerfiles (F-16 removes the Python images).
#[test]
fn after_the_cutover_ci_images_holds_no_python_image() {
    if !cut_over() {
        eprintln!("skipped: before the cutover (port/staging/ exists)");
        return;
    }
    let found: BTreeSet<String> = repo::list_dir("ci/images").into_iter().collect();
    let want: BTreeSet<String> = [
        "alpine",
        "debian-slim",
        "gitleaks",
        "rust",
        "shellcheck",
        "ubuntu",
    ]
    .iter()
    .map(|n| format!("{n}.Dockerfile"))
    .collect();
    assert_eq!(found, want);
}

/// No CI job script runs Python after the cutover (design §14.2); the shims and tools CO2 moves
/// into place already run none.
#[test]
fn no_ci_job_script_runs_python() {
    let mut scripts: Vec<String> = ["ci/jobs/cli-syntax.sh", "ci/jobs/evals.sh"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    if cut_over() {
        scripts.extend(
            repo::list_dir("ci/jobs")
                .into_iter()
                .filter(|n| n.ends_with(".sh"))
                .map(|n| format!("ci/jobs/{n}")),
        );
    }
    for rel in scripts {
        runs_no_python(&rel, &staged(&rel));
    }
}
