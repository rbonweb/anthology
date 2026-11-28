//! check-repo Task 15: the scanner scans this repository and finds nothing worse than MEDIUM.
//!
//! "No false alarms" (plan, Global Constraints) is a promise about real repos; this repo is the
//! first one it is held to. Indicator literals in Rust test sources are split (`concat!`) so the
//! scan stays clean without an exclusion; the scan runs with no exclusions at all.
//!
//! The same file pins the rest of Task 15: the network corpus (`ci/check-repo-corpus.tsv`, its job
//! script and workflow) and the fuzz target.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Output};

use anthology_scan::tree::{DiskTree, WalkOpts};
use anthology_scan::{Finding, Scan, Severity, scan};
use repo_tests::repo;
use repo_tests::yaml;

fn scan_repo() -> Scan {
    let tree = DiskTree::open(&repo::root(), &WalkOpts::default(), None).expect("open the repo");
    scan(&tree)
}

fn block(f: &Finding) -> String {
    format!(
        "{} {} {}:{}\n  why: {}\n  seen: {}",
        f.severity.name(),
        f.rule,
        f.path,
        f.line,
        f.why,
        f.seen.chars().take(160).collect::<String>().escape_debug()
    )
}

#[test]
fn this_repo_has_no_high_or_critical_finding() {
    let s = scan_repo();
    let bad: Vec<String> = s
        .findings
        .iter()
        .filter(|f| f.severity >= Severity::High)
        .map(block)
        .collect();
    assert!(
        bad.is_empty(),
        "{} HIGH/CRITICAL finding(s) in this repo:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

#[test]
fn the_self_scan_covers_the_whole_tree() {
    let s = scan_repo();
    assert!(!s.partial, "partial scan: {:?} {:?}", s.errors, s.skipped);
    assert!(s.files > 500, "only {} files scanned", s.files);
    assert!(
        s.errors.is_empty(),
        "scan errors on this repo: {:?}",
        s.errors
    );
}

/// The lower levels are printed so a reviewer sees them in the test log (`--nocapture`); they do
/// not fail the test.
#[test]
fn medium_findings_are_listed_for_review() {
    let s = scan_repo();
    for f in s.findings.iter().filter(|f| f.severity == Severity::Medium) {
        println!("{}", block(f));
    }
}

// ---- the network corpus ---------------------------------------------------------------------

const CORPUS: &str = "ci/check-repo-corpus.tsv";
const CORPUS_JOB: &str = "ci/jobs/check-repo-corpus.sh";
const CORPUS_WORKFLOW: &str = ".github/workflows/check-repo-corpus.yml";

/// The (url, rev) rows of a corpus file: blank lines and `#` lines are skipped.
fn rows(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| {
            let (url, rev) = l
                .split_once('\t')
                .unwrap_or_else(|| panic!("no tab: {l:?}"));
            (url.to_string(), rev.to_string())
        })
        .collect()
}

#[test]
fn the_corpus_pins_the_twelve_repositories_to_full_commits() {
    let rows = rows(&repo::read(CORPUS));
    let mut repos: Vec<&str> = rows
        .iter()
        .map(|(url, _)| {
            url.strip_prefix("https://github.com/")
                .and_then(|r| r.strip_suffix(".git"))
                .unwrap_or_else(|| panic!("not a GitHub https clone URL: {url}"))
        })
        .collect();
    repos.sort_unstable();
    assert_eq!(
        repos,
        [
            "BurntSushi/ripgrep",
            "anthropics/claude-code",
            "devcontainers/cli",
            "evanw/esbuild",
            "facebook/react",
            "lovell/sharp",
            "microsoft/vscode",
            "modelcontextprotocol/servers",
            "pallets/flask",
            "psf/requests",
            "spring-projects/spring-petclinic",
            "typicode/husky",
        ]
    );
    for (url, rev) in &rows {
        assert!(
            rev.len() == 40 && rev.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
            "{url}: {rev:?} is not a full lowercase commit SHA"
        );
    }
}

#[test]
fn the_corpus_workflow_is_pinned_read_only_and_runs_the_job() {
    let wf = yaml::load_file(CORPUS_WORKFLOW);
    // Read-only token, set at the top so no job can widen it by leaving it out.
    assert_eq!(wf["permissions"]["contents"].as_str(), Some("read"));
    assert_eq!(wf["permissions"].as_hash().map(|h| h.len()), Some(1));
    let on = &wf["on"];
    // No pull_request_target and no push: a fork never reaches a token, and the weekly run covers
    // upstream moving.
    assert!(!yaml::has(on, "pull_request_target"));
    assert!(!yaml::has(on, "push"));
    assert_eq!(
        yaml::strings(&on["pull_request"]["paths"]),
        [
            "crates/anthology-scan/**",
            "ci/check-repo-corpus.tsv",
            "ci/jobs/check-repo-corpus.sh",
        ]
    );
    assert!(yaml::has(on, "workflow_dispatch"));
    assert!(
        on["schedule"][0]["cron"]
            .as_str()
            .is_some_and(|c| c.split(' ').count() == 5)
    );
    let mut runs = String::new();
    let mut actions = 0;
    for (name, job) in wf["jobs"].as_hash().expect("jobs") {
        for step in yaml::items(&job["steps"]) {
            if let Some(uses) = step["uses"].as_str() {
                actions += 1;
                let (_, sha) = uses.split_once('@').expect("uses has a ref");
                assert!(
                    sha.len() == 40 && sha.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
                    "{name:?}: {uses} is not pinned by SHA"
                );
            }
            runs.push_str(step["run"].as_str().unwrap_or(""));
        }
    }
    assert!(actions >= 1, "the job checks the repo out");
    assert!(runs.contains(CORPUS_JOB), "{runs}");
    let text = repo::read(CORPUS_WORKFLOW);
    for line in text.lines().filter(|l| l.contains("uses: ")) {
        assert!(line.contains(" # v"), "{line}");
    }
    assert!(text.contains("persist-credentials: false"));
}

/// The body of a script that stands in for `anthology`: it logs its arguments, and answers by the
/// last one (the URL).
const FAKE_BIN: &str = r#"
printf '%s\n' "$*" >> "$LOG"
for url; do :; done
case $url in
    *slow*) cat "$DIR/slow.json" ;;
    *edge*) cat "$DIR/edge.json" ;;
    *quiet-high*) cat "$DIR/high.json" ;;
    *high*) cat "$DIR/high.json"; exit 1 ;;
    *broken*) echo "anthology check-repo: the fetch failed" >&2; exit 3 ;;
    *) cat "$DIR/ok.json" ;;
esac
"#;

fn report(high: u32, scan_ms: u64) -> String {
    let finding = if high > 0 {
        r#",{"id":"F2","severity":"HIGH","rule":"payload.decode-exec","path":"src/a.js","line":7}"#
    } else {
        ""
    };
    format!(
        r#"{{"version":1,"counts":{{"CRITICAL":0,"HIGH":{high},"MEDIUM":1,"LOW":0,"INFO":2}},"exit":{},"files":10,"scan_ms":{scan_ms},"findings":[{{"id":"F1","severity":"MEDIUM","rule":"hidden.zero-width","path":"a/b.md","line":3}}{finding}]}}"#,
        i32::from(high > 0)
    )
}

struct Corpus {
    dir: tempfile::TempDir,
}

impl Corpus {
    fn new(tsv: &str) -> Corpus {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        fs::write(p.join("ok.json"), report(0, 120)).unwrap();
        fs::write(p.join("slow.json"), report(0, 5001)).unwrap();
        fs::write(p.join("edge.json"), report(0, 5000)).unwrap();
        fs::write(p.join("high.json"), report(1, 90)).unwrap();
        fs::write(p.join("corpus.tsv"), tsv).unwrap();
        let bin = p.join("anthology");
        fs::write(&bin, format!("#!/bin/sh\n{FAKE_BIN}")).unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        Corpus { dir }
    }

    fn run(&self) -> Output {
        let p = self.dir.path();
        Command::new("sh")
            .arg(repo::path(CORPUS_JOB))
            .arg("--run")
            .arg(p.join("anthology"))
            .arg(p.join("corpus.tsv"))
            .env("DIR", p)
            .env("LOG", p.join("argv.log"))
            .output()
            .expect("sh ran")
    }

    fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.dir.path().join("argv.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

const REV_A: &str = "0123456789abcdef0123456789abcdef01234567";
const REV_B: &str = "fedcba9876543210fedcba9876543210fedcba98";

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn the_corpus_job_scans_each_row_without_tools_at_its_pinned_rev() {
    let c = Corpus::new(&format!(
        "# a comment\n\nhttps://example.invalid/a.git\t{REV_A}\nhttps://example.invalid/b.git\t{REV_B}\n"
    ));
    let out = c.run();
    assert!(out.status.success(), "{}", text(&out));
    let calls = c.calls();
    assert_eq!(calls.len(), 2, "{calls:?}");
    for (call, (url, rev)) in calls.iter().zip([
        ("https://example.invalid/a.git", REV_A),
        ("https://example.invalid/b.git", REV_B),
    ]) {
        let words: Vec<&str> = call.split(' ').collect();
        assert_eq!(
            &words[..6],
            ["check-repo", "--json", "--no-tools", "--rev", rev, "--into"],
            "{call}"
        );
        assert_eq!(*words.last().unwrap(), url, "{call}");
        assert_eq!(words.len(), 8, "{call}");
    }
    // Every row gets its own quarantine folder, outside the job's checkout.
    let into: Vec<&str> = calls.iter().map(|c| c.split(' ').nth(6).unwrap()).collect();
    assert_ne!(into[0], into[1]);
    assert!(
        into.iter()
            .all(|p| !p.starts_with(repo::root().to_str().unwrap())),
        "{into:?}"
    );
}

#[test]
fn the_corpus_job_fails_on_a_nonzero_exit() {
    let c = Corpus::new(&format!("https://example.invalid/broken.git\t{REV_A}\n"));
    let out = c.run();
    assert!(!out.status.success());
    let all = text(&out);
    assert!(all.contains("exit 3"), "{all}");
    assert!(all.contains("the fetch failed"), "{all}");
}

#[test]
fn the_corpus_job_fails_on_a_high_finding_and_names_it() {
    // Exit 1 (what the binary gives for a HIGH) and, separately, a HIGH count with exit 0.
    for name in ["high", "quiet-high"] {
        let c = Corpus::new(&format!("https://example.invalid/{name}.git\t{REV_A}\n"));
        let out = c.run();
        assert!(!out.status.success(), "{name}");
        let all = text(&out);
        assert!(all.contains("HIGH F2 payload.decode-exec"), "{name}: {all}");
        assert!(all.contains("src/a.js"), "{name}: {all}");
    }
}

#[test]
fn the_corpus_job_allows_5000_ms_and_fails_5001() {
    let slow = Corpus::new(&format!("https://example.invalid/slow.git\t{REV_A}\n")).run();
    assert!(!slow.status.success());
    assert!(text(&slow).contains("5001"), "{}", text(&slow));
    let edge = Corpus::new(&format!("https://example.invalid/edge.git\t{REV_A}\n")).run();
    assert!(edge.status.success(), "{}", text(&edge));
}

#[test]
fn the_corpus_job_scans_every_row_before_it_fails() {
    let c = Corpus::new(&format!(
        "https://example.invalid/broken.git\t{REV_A}\nhttps://example.invalid/ok.git\t{REV_B}\n"
    ));
    assert!(!c.run().status.success());
    assert_eq!(c.calls().len(), 2);
}

#[test]
fn the_corpus_job_refuses_an_empty_or_malformed_corpus() {
    for tsv in [
        "# only a comment\n",
        "https://example.invalid/a.git\n",
        "https://example.invalid/a.git\tmain\n",
        "https://example.invalid/a.git\t0123456789abcdef0123456789abcdef01234567\textra\n",
        "-ohttps://example.invalid/a.git\t0123456789abcdef0123456789abcdef01234567\n",
    ] {
        let c = Corpus::new(tsv);
        let out = c.run();
        assert!(!out.status.success(), "{tsv:?}: {}", text(&out));
        assert!(c.calls().is_empty(), "{tsv:?} ran the binary");
    }
}

#[test]
fn the_corpus_job_builds_the_release_binary_through_the_test_runner_and_never_runs_repo_code() {
    let script = repo::read(CORPUS_JOB);
    let code: String = script
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(code.contains("scripts/rust-test.sh"));
    assert!(code.contains("cargo build --release --locked -p anthology-cli"));
    for flag in ["--json", "--no-tools", "--rev", "--into"] {
        assert!(code.contains(flag), "{flag}");
    }
    // The scanned repos are data: nothing here installs, builds or runs them.
    for word in [
        "npm ",
        "npx ",
        "make ",
        "pip ",
        "cargo run",
        "bash -c",
        "eval ",
    ] {
        assert!(!code.contains(word), "{word:?} in {CORPUS_JOB}");
    }
}

#[test]
fn the_corpus_rows_in_the_scope_rules_run_the_repository_checks() {
    let rules = repo::read("ci/scope-rules.tsv");
    for path in [CORPUS_JOB, CORPUS, CORPUS_WORKFLOW] {
        let row = rules
            .lines()
            .filter(|l| !l.starts_with('#'))
            .find(|l| l.split('\t').next() == Some(path))
            .unwrap_or_else(|| panic!("no scope row for {path}"));
        assert!(row.ends_with("\tpkg:repo-tests"), "{row}");
    }
}

// ---- the fuzz target ------------------------------------------------------------------------

#[test]
fn the_check_repo_fuzz_target_is_registered_and_locked() {
    let manifest = repo::read("fuzz/Cargo.toml");
    assert!(manifest.contains("anthology-scan = { path = \"../crates/anthology-scan\" }"));
    assert!(manifest.contains("name = \"check_repo\"\npath = \"fuzz_targets/check_repo.rs\""));
    let target = repo::read("fuzz/fuzz_targets/check_repo.rs");
    for call in ["jsonc::parse", "yara::compile", "scan("] {
        assert!(target.contains(call), "{call}");
    }
    // `ci/jobs/fuzz.sh` runs `cargo metadata --locked`: the lock names the crate.
    let lock = repo::read("fuzz/Cargo.lock");
    assert!(lock.contains("name = \"anthology-scan\""));
}
