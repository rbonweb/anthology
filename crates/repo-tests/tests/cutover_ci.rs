//! CO2: CI and the maintainer tools once the Python is gone (plan CO2; design §14.2; GC-15
//! F-16, F-17). The replacements the ledger names for the Python-pinning parts of test_ci,
//! test_evals_layout, test_repo_layout, test_rtk_layout and test_rtk_claude_only.
//!
//! Scripts CO2 moves out of `port/staging/` are read through `anthology_testkit::staged`; a test
//! that only holds once the Python is deleted skips while `port/staging/` exists.

use std::collections::BTreeSet;

use repo_tests::repo;
use repo_tests::yaml::{self, Yaml};

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
    // legacy: port/ledger names this test
    let names = anthology_dev::smoke::names();
    for want in ["atl-rewrite", "agent-memory-path"] {
        assert!(names.contains(&want), "{want} in {names:?}");
    }
}

/// test_rtk_layout `test_records_the_merged_upstream_pin`: sync-atl.sh records the new pin with
/// `scripts/dev.sh vendor set-atl-upstream` and embeds no python3.
#[test]
fn sync_rtk_records_the_pin_through_dev_sh() {
    // legacy: port/ledger names this test
    let rel = "scripts/sync-atl.sh";
    let text = staged(rel);
    assert!(
        text.contains("scripts/dev.sh vendor set-atl-upstream --ref \"$tag\" --commit"),
        "{rel}"
    );
    runs_no_python(rel, &text);
}

/// test_rtk_claude_only `test_trim_entry_is_executable_strict_and_runs_the_python_trim`: the trim
/// runs as `scripts/dev.sh atl-trim`; after the cutover `scripts/atl_trim.py` is gone.
#[test]
fn rtk_trim_runs_the_trim_through_dev_sh() {
    // legacy: port/ledger names this test
    let rel = "scripts/atl-trim.sh";
    let text = staged(rel);
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(anthology_testkit::staged(rel))
        .expect("stat")
        .permissions()
        .mode();
    assert!(mode & 0o111 != 0, "{rel} is executable");
    assert!(code_lines(&text).contains(&"set -eu"), "{rel}: set -eu");
    assert!(
        text.contains("\"$root/scripts/dev.sh\" atl-trim --root atl"),
        "{rel}"
    );
    runs_no_python(rel, &text);
    if cut_over() {
        assert!(!repo::path("scripts/atl_trim.py").exists());
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

// ---- The workflows after the switch (design §14.2) ------------------------------------------

fn workflow(name: &str) -> Yaml {
    yaml::load_file(&format!(".github/workflows/{name}"))
}

/// The `run` text of every step of `job`, joined.
fn run_text(job: &Yaml) -> String {
    yaml::items(&job["steps"])
        .iter()
        .map(|s| s["run"].as_str().unwrap_or("").trim_end())
        .filter(|r| !r.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn ci_job(name: &str) -> Yaml {
    let job = workflow("ci.yml")["jobs"][name].clone();
    assert!(!job.is_badvalue(), "ci.yml has no job {name}");
    job
}

/// test_ci `test_version_bump_job`, the step: `anthology-dev version-bump "$BASE" "$HEAD_SHA"`.
#[test]
fn the_version_bump_job_runs_anthology_dev() {
    let run = run_text(&ci_job("version-bump"));
    assert_eq!(run, r#"scripts/dev.sh version-bump "$BASE" "$HEAD_SHA""#);
}

/// test_ci `test_memory_diff_is_pull_request_only_and_passes_the_shas_by_env`, the step.
#[test]
fn the_memory_diff_job_runs_anthology_dev() {
    let run = run_text(&ci_job("memory-diff"));
    assert_eq!(
        run,
        r#"scripts/dev.sh memory-diff "$BASE" "$HEAD_SHA" >>"$GITHUB_STEP_SUMMARY""#
    );
}

/// test_ci `test_startup_budget_runs_both_budget_tests_in_the_pinned_image` (m-18): the
/// per-agent budget tool, the test_roster ports (`--test roster`, so the filter picks no timeout
/// pin of ci.rs) and the test_session_start `budget` ports, all in the pinned Rust image.
#[test]
fn the_startup_budget_job_runs_both_budgets_in_the_rust_image() {
    let run = run_text(&ci_job("startup-budget"));
    let lines: Vec<&str> = run.lines().collect();
    assert_eq!(
        lines,
        [
            "scripts/dev.sh startup-budget",
            "scripts/rust-test.sh run cargo test --locked -p repo-tests --test roster startup_budget",
            "scripts/rust-test.sh run cargo test --locked -p anthology-runtime budget",
        ]
    );
}

/// test_ci `test_cli_syntax_installs_the_pinned_cli_and_runs_the_smoke`: the job runs
/// ci/jobs/cli-syntax.sh, which runs `anthology-dev smoke cli-syntax` with the pinned CLI.
#[test]
fn the_cli_syntax_job_runs_the_rust_smoke() {
    let run = run_text(&ci_job("cli-syntax"));
    assert!(run.contains("npm ci --prefix ci/claude-code"), "{run}");
    assert!(run.lines().any(|l| l == "ci/jobs/cli-syntax.sh"), "{run}");
    let script = staged("ci/jobs/cli-syntax.sh");
    assert!(
        script.contains(
            r#"exec "$dev" smoke --claude ci/claude-code/node_modules/.bin/claude cli-syntax"#
        ),
        "ci/jobs/cli-syntax.sh"
    );
}

/// upstream-watch.yml runs the Rust watch with host cargo (dev.sh clears the environment the
/// watch needs: GITHUB_TOKEN, GITHUB_REPOSITORY, GITHUB_OUTPUT).
#[test]
fn the_upstream_watch_runs_anthology_dev() {
    let run = run_text(&workflow("upstream-watch.yml")["jobs"]["watch"]);
    assert!(
        run.contains("cargo run --locked --release -p anthology-dev -- upstream-watch"),
        "{run}"
    );
}

/// Design §14.2: no workflow uses Python, and the Python jobs are gone.
#[test]
fn no_workflow_uses_python() {
    for name in repo::list_dir(".github/workflows") {
        let rel = format!(".github/workflows/{name}");
        for line in repo::read(&rel).lines() {
            assert!(!line.to_lowercase().contains("python"), "{rel}: {line}");
        }
    }
    let jobs: BTreeSet<String> = yaml::keys(&workflow("ci.yml")["jobs"])
        .into_iter()
        .collect();
    for gone in [
        "test",
        "test-py39",
        "memory-git",
        "setup-git",
        "taste-bridge",
        "difftest-linux",
        "difftest-macos",
    ] {
        assert!(!jobs.contains(gone), "ci.yml still has {gone}");
    }
}

/// Every ci.yml job has a timeout (the Rust jobs build cold).
#[test]
fn every_ci_job_has_a_timeout() {
    let ci = workflow("ci.yml");
    for name in yaml::keys(&ci["jobs"]) {
        let minutes = ci["jobs"][name.as_str()]["timeout-minutes"].as_i64();
        assert!(minutes.is_some_and(|m| m > 0), "{name}: timeout-minutes");
    }
}

/// F-16: Dependabot has no `pip` ecosystem, and cargo updates target the default branch.
#[test]
fn dependabot_has_no_pip_and_no_port_branch() {
    let d = yaml::load_file(".github/dependabot.yml");
    for update in yaml::items(&d["updates"]) {
        let eco = update["package-ecosystem"].as_str().unwrap_or("");
        assert_ne!(eco, "pip");
        assert!(!yaml::has(update, "target-branch"), "{eco}: target-branch");
    }
}
