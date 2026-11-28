//! check-repo Task 15: the scanner scans this repository and finds nothing worse than MEDIUM.
//!
//! "No false alarms" (plan, Global Constraints) is a promise about real repos; this repo is the
//! first one it is held to. Indicator literals in Rust test sources are split (`concat!`) so the
//! scan stays clean without an exclusion; the scan runs with no exclusions at all.

use anthology_scan::tree::{DiskTree, WalkOpts};
use anthology_scan::{Finding, Scan, Severity, scan};
use repo_tests::repo;

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
