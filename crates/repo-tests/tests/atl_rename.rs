//! The ATL rename's final check (ADR-033): the old name `rtk` is gone from the repository.
//!
//! Over `git ls-files` (the text files, and every `.gz` file decompressed, which is how the
//! difftest goldens are stored), every case-insensitive `rtk` must be under an allowed path or on
//! an allowed line. The allowed paths are history and upstream records (the upstream license and
//! changelog, the past changelog, ADRs and plan, the port records, the dated 2026-11-20 designs)
//! and the shell and guard test data and goldens, which keep rtk rows on purpose (the guard still
//! recognises the old name). The allowed lines are the upstream names (`rtk-ai/rtk`,
//! `rtk-ai.app`, `rbonweb/rtk`, `RTK AI Labs`, `RTK Pro`), the five release tags of the old
//! name, "fork of rtk", the Python test ids `test_rtk_*`, and any line that says `legacy`: every
//! `rtk` that stays in code carries the word on the same line.
//!
//! This file names the old name itself, so it is an allowed path.
//!
//! It passes only once every part of the rename has landed (the binary and CI, the shell and
//! guard, install, hook and doctor, settings, setup and docs), so it is ignored until the release
//! integration, which removes the `#[ignore]`.

use std::io::Read;

use flate2::read::GzDecoder;
use regex::Regex;
use repo_tests::repo;

/// Paths (a folder prefix ending in `/`, or a whole file) whose content is history or upstream's.
const ALLOWED_PATHS: [&str; 9] = [
    "atl/LICENSE",
    "atl/CHANGELOG.md",
    "CHANGELOG.md",
    "DECISIONS.md",
    "PLAN.md",
    "port/",
    "crates/anthology-guard/tests/data/",
    "crates/anthology-guard/tests/golden/",
    "crates/anthology-shell/tests/golden/",
];

/// This file, which has to name the old name to look for it.
const THIS_FILE: &str = "crates/repo-tests/tests/atl_rename.rs";

fn allowed_path(path: &str) -> bool {
    if path == THIS_FILE {
        return true;
    }
    // The dated designs and plans: docs/superpowers/<kind>/2026-11-20-*.
    if let Some(rest) = path.strip_prefix("docs/superpowers/")
        && rest
            .split('/')
            .next_back()
            .is_some_and(|name| name.starts_with("2026-11-20-"))
    {
        return true;
    }
    ALLOWED_PATHS.iter().any(|a| {
        if a.ends_with('/') {
            path.starts_with(a)
        } else {
            path == *a
        }
    })
}

fn old_name() -> Regex {
    Regex::new(r"(?i)rtk").unwrap()
}

fn allowed_line() -> Regex {
    Regex::new(
        r"(?i)rtk-ai/rtk|rtk-ai\.app|rbonweb/rtk|RTK AI Labs|RTK Pro|rtk--v1\.2\.1-anthology\.[1-5]\b|fork of rtk|test_rtk_|legacy",
    )
    .unwrap()
}

/// The text of a tracked file: `.gz` decompressed; `None` for a file that is not UTF-8 text.
fn text_of(path: &str) -> Option<String> {
    let mut bytes = std::fs::read(repo::path(path)).ok()?;
    if path.ends_with(".gz") {
        let mut plain = Vec::new();
        GzDecoder::new(bytes.as_slice())
            .read_to_end(&mut plain)
            .ok()?;
        bytes = plain;
    }
    if bytes.contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

#[test]
fn the_rules_say_what_they_allow() {
    let (old, line) = (old_name(), allowed_line());
    for ok in [
        "see https://github.com/rtk-ai/rtk for upstream",
        "Copyright 2026 RTK AI Labs",
        "the tag rtk--v1.2.1-anthology.3 stays",
        "ATL is a fork of rtk by the upstream authors",
        "def test_rtk_install(self):",
        "const LEGACY_ALLOW_RTK: &str = \"Bash(rtk *)\";",
        "a legacy rtk line",
    ] {
        assert!(old.is_match(ok) && line.is_match(ok), "{ok}");
    }
    for bad in [
        "run rtk gain",
        "Bash(rtk *)",
        "the tag rtk--v1.2.1-anthology.6",
        "~/.claude/anthology/rtk/",
        "RTK_DB_PATH",
    ] {
        assert!(old.is_match(bad) && !line.is_match(bad), "{bad}");
    }
    for path in [
        "atl/LICENSE",
        "atl/CHANGELOG.md",
        "CHANGELOG.md",
        "DECISIONS.md",
        "PLAN.md",
        "port/difftest/divergences/guard.check.toml",
        "docs/superpowers/plans/2026-11-20-rust-port.md",
        "docs/superpowers/specs/2026-11-20-rust-port-design.md",
        "crates/anthology-guard/tests/data/test_guard_rules.jsonl",
        "crates/anthology-guard/tests/golden/guard.check.jsonl.gz",
        "crates/anthology-shell/tests/golden/shell.analyze.jsonl.gz",
        THIS_FILE,
    ] {
        assert!(allowed_path(path), "{path}");
    }
    for path in [
        "README.md",
        "SECURITY.md",
        "atl/README.md",
        "docs/superpowers/plans/2026-11-22-something.md",
        "crates/anthology-settings/tests/golden/settings.jsonl.gz",
        "crates/anthology-runtime/tests/golden/setup.jsonl.gz",
        "plugins/anthology/skills/setup/SKILL.md",
    ] {
        assert!(!allowed_path(path), "{path}");
    }
}

#[test]
#[ignore = "passes only once the rest of the rename (T1, T2, T3) has landed; the release integration removes this"]
fn no_rtk_outside_the_allowed_paths_and_lines() {
    let files = repo::tracked_files().expect("git lists the tracked files here");
    let (old, line) = (old_name(), allowed_line());
    let mut found: Vec<String> = Vec::new();
    for path in files {
        if allowed_path(&path) {
            continue;
        }
        if old.is_match(&path) {
            found.push(format!("{path}: the path itself"));
        }
        let Some(text) = text_of(&path) else { continue };
        for (n, l) in text.lines().enumerate() {
            if old.is_match(l) && !line.is_match(l) {
                let shown: String = l.chars().take(160).collect();
                found.push(format!("{path}:{}: {shown}", n + 1));
            }
        }
    }
    assert!(
        found.is_empty(),
        "{} place(s) still name the old tool; first 40:\n{}",
        found.len(),
        found
            .iter()
            .take(40)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
