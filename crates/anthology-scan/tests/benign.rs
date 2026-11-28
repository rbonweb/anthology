//! The benign samples (plan Task 11): small real files from public projects that look like an attack
//! to a careless scanner. Each sample folder under `tests/benign` is scanned as if it were a repo.
//! The scan must give no HIGH and no CRITICAL: a finding at that level here is a false alarm, and the
//! fix belongs in the rule, never in this test. MEDIUM and lower findings are printed (run with
//! `--nocapture`) and the ones that matter are pinned below, so a rule that stops seeing its surface
//! fails too.
//!
//! `tests/benign/SOURCES.md` records where each file came from.

use std::fs;
use std::path::{Path, PathBuf};

use anthology_scan::report;
use anthology_scan::tree::{DiskTree, WalkOpts};
use anthology_scan::{Finding, Scan, Severity, scan};

fn benign_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/benign")
}

/// Every sample folder, as named in `SOURCES.md`.
const SAMPLES: &[&str] = &[
    "claude-prettier-hook",
    "esbuild",
    "flask",
    "gradle-wrapper",
    "husky",
    "mcp-pinned",
    "minified-bundle",
    "ripgrep",
    "sharp",
    "unicode-text",
    "vscode-devcontainer",
    "vscode-tasks",
];

fn scan_sample(name: &str) -> Scan {
    let root = benign_root().join(name);
    let tree = DiskTree::open(&root, &WalkOpts::default(), None)
        .unwrap_or_else(|e| panic!("cannot open {}: {e}", root.display()));
    let found = scan(&tree);
    assert!(found.errors.is_empty(), "{name}: {:?}", found.errors);
    assert!(!found.partial, "{name}: the scan was partial");
    assert!(found.files > 0, "{name}: the scan saw no file");
    found
}

fn line(f: &Finding) -> String {
    format!(
        "{} {} {}:{} count={} seen={:?}",
        f.severity.name(),
        f.rule,
        f.path,
        f.line,
        f.count,
        f.seen.chars().take(100).collect::<String>()
    )
}

/// Prints every finding of MEDIUM or lower, and fails on any HIGH or CRITICAL.
fn assert_no_false_alarm(name: &str, s: &Scan) {
    for f in &s.findings {
        println!("[{name}] {}", line(f));
    }
    let alarms: Vec<String> = s
        .findings
        .iter()
        .filter(|f| f.severity >= Severity::High)
        .map(line)
        .collect();
    assert!(
        alarms.is_empty(),
        "{name}: a false alarm (fix the rule, not the test):\n{}",
        alarms.join("\n")
    );
    assert_eq!(report::exit_code(s), 0, "{name}: the exit code");
}

/// The sample scans clean and holds these findings, each at exactly this severity.
fn check(name: &str, pinned: &[(&str, Severity)]) {
    let s = scan_sample(name);
    assert_no_false_alarm(name, &s);
    for (rule, severity) in pinned {
        let hit = s
            .findings
            .iter()
            .find(|f| f.rule == *rule)
            .unwrap_or_else(|| {
                let all: Vec<String> = s.findings.iter().map(line).collect();
                panic!("{name}: no {rule}; findings:\n{}", all.join("\n"))
            });
        assert_eq!(hit.severity, *severity, "{name}: {rule}: {}", line(hit));
    }
}

#[test]
fn vscode_task_that_runs_a_watch_on_folder_open() {
    check(
        "vscode-tasks",
        &[("autorun.vscode-folder-open", Severity::Medium)],
    );
}

#[test]
fn devcontainer_with_features_and_a_container_side_command() {
    check("vscode-devcontainer", &[]);
}

#[test]
fn husky_prepare_script_and_hook() {
    check("husky", &[("autorun.npm-lifecycle", Severity::Info)]);
}

#[test]
fn esbuild_postinstall_runs_its_own_install_js() {
    check("esbuild", &[("autorun.npm-lifecycle", Severity::Medium)]);
}

#[test]
fn sharp_install_chain_of_node_scripts_and_prebuild_install() {
    check("sharp", &[("autorun.npm-lifecycle", Severity::Medium)]);
}

#[test]
fn prettier_post_tool_hook_in_claude_settings() {
    check(
        "claude-prettier-hook",
        &[("agent.claude-hook", Severity::Medium)],
    );
}

#[test]
fn mcp_servers_pinned_to_a_version() {
    check("mcp-pinned", &[("agent.mcp-stdio", Severity::Low)]);
}

#[test]
fn ripgrep_build_rs_that_reads_a_git_hash() {
    check("ripgrep", &[]);
}

#[test]
fn flask_pyproject_with_a_build_backend() {
    check("flask", &[]);
}

#[test]
fn gradle_wrapper_properties_on_the_official_host() {
    check("gradle-wrapper", &[]);
}

#[test]
fn minified_bundle_is_not_a_payload() {
    let s = scan_sample("minified-bundle");
    assert_no_false_alarm("minified-bundle", &s);
}

#[test]
fn emoji_flags_and_right_to_left_text_hide_nothing() {
    let s = scan_sample("unicode-text");
    assert_no_false_alarm("unicode-text", &s);
    let hidden: Vec<&Finding> = s
        .findings
        .iter()
        .filter(|f| f.rule.starts_with("hidden."))
        .collect();
    assert!(
        hidden.is_empty(),
        "hidden-text false hit: {:?}",
        hidden.iter().map(|f| line(f)).collect::<Vec<_>>()
    );
}

#[test]
fn every_sample_folder_is_listed_and_has_a_source() {
    let sources = fs::read_to_string(benign_root().join("SOURCES.md")).expect("SOURCES.md");
    let mut folders: Vec<String> = fs::read_dir(benign_root())
        .expect("the benign folder")
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != "LICENSES")
        .collect();
    folders.sort();
    assert_eq!(
        folders, SAMPLES,
        "tests/benign holds a folder SAMPLES lacks"
    );
    for name in SAMPLES {
        assert!(
            sources.contains(&format!("`{name}/")),
            "SOURCES.md has no row for {name}"
        );
    }
    assert!(sources.contains("license"), "SOURCES.md names the licenses");
}
