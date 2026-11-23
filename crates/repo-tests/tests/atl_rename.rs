//! The ATL rename's final check (ADR-035): the old name `rtk` is gone from the repository.
//!
//! Over `git ls-files` (the text files, and every `.gz` file decompressed, which is how the
//! difftest goldens are stored), every case-insensitive `rtk` must be under an allowed path or on
//! an allowed line. The allowed paths are history and upstream records (the upstream license and
//! changelog, the past changelog, ADRs and plan, the port records, the dated 2026-11-20 designs,
//! the dated agent and repo memory notes)
//! and the shell and guard test data and goldens, which keep rtk rows on purpose (the guard still
//! recognises the old name). The allowed lines are the upstream names (`rtk-ai/rtk`,
//! `rtk-ai.app`, `rbonweb/rtk`, `RTK AI Labs`, `RTK Pro`), the five release tags of the old
//! name, "fork of rtk", the Python test ids `test_rtk_*`, and any line that says `legacy`: every
//! `rtk` that stays in code carries the word on the same line. The few names the port records
//! pin, and lines that cannot carry the word, are listed one by one with the reason.
//!
//! This file names the old name itself, so it is an allowed path.
//!
//! It covers every part of the rename (the binary and CI, the shell and guard, install, hook and
//! doctor, settings, setup and docs).

use std::collections::HashMap;
use std::io::Read;

use flate2::read::GzDecoder;
use regex::Regex;
use repo_tests::repo;

/// Paths (a folder prefix ending in `/`, or a whole file) whose content is history or upstream's.
const ALLOWED_PATHS: [&str; 11] = [
    "atl/LICENSE",
    "atl/CHANGELOG.md",
    "CHANGELOG.md",
    "DECISIONS.md",
    "PLAN.md",
    "port/",
    // Dated agent notes are history, like the other dated records (ruling Q3).
    ".claude/agent-memory/",
    ".claude/memory/",
    "crates/anthology-guard/tests/data/",
    "crates/anthology-guard/tests/golden/",
    "crates/anthology-shell/tests/golden/",
];

/// This file, which has to name the old name to look for it.
const THIS_FILE: &str = "crates/repo-tests/tests/atl_rename.rs";

/// Files whose own name keeps the old name because the port records pin it (`port/ledger/*.tsv`
/// names the Rust test file, `port/difftest/divergences/dev.rtk-trim.toml` the surface), and
/// `port/` never changes. Only the path is excused: their lines are checked like any other.
const PINNED_PATHS: [&str; 5] = [
    "crates/anthology-dev/tests/rtk_trim.rs",
    "crates/anthology-dev/tests/golden/dev.rtk-trim.jsonl.gz",
    "crates/repo-tests/tests/rtk_claude_only.rs",
    "crates/repo-tests/tests/rtk_layout.rs",
    "crates/repo-tests/tests/rtk_release_workflow.rs",
];

/// Single lines that keep the old name and cannot say `legacy` on the same line: (file, a
/// substring of the line, why): golden lines, which are recorded output. Test names the port
/// ledger pins are taken out of each line by [`ledger_pins`] instead.
const PINNED_LINES: [(&str, &str, &str); 4] = [
    (
        "crates/anthology-dev/tests/golden/dev.rtk-trim.jsonl.gz",
        "\"surface\": \"dev.rtk-trim\"",
        "the header names the surface, which port/difftest/divergences pins",
    ),
    (
        "crates/anthology-runtime/tests/golden/hook.session-start.jsonl.gz",
        "rtkq",
        "random generated text that happens to spell the old name; the case is kept as a digest",
    ),
    (
        "crates/anthology-runtime/tests/golden/setup.jsonl.gz",
        "{\"id\": \"seed:127\", ",
        "a seed of a legacy install whose result is kept as a digest, so no line says legacy",
    ),
    (
        "crates/anthology-runtime/tests/golden/setup.jsonl.gz",
        "{\"id\": \"seed:133\", ",
        "a seed of a legacy install whose result is kept as a digest, so no line says legacy",
    ),
];

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

/// Port ledger pin: every module and fn name in the `rust_test` column of `port/ledger/*.tsv`
/// (`<file>::<module>::<fn>`) keeps its Python name, old tool name included, so the ledger can
/// tie it to the Python test. Per Rust file, the pinned names that hold the old name.
fn ledger_pins() -> HashMap<String, Vec<String>> {
    let old = old_name();
    let mut pins: HashMap<String, Vec<String>> = HashMap::new();
    let dir = repo::path("port/ledger");
    for entry in std::fs::read_dir(&dir).expect("port/ledger") {
        let path = entry.expect("an entry").path();
        if path.extension().is_none_or(|e| e != "tsv") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("a ledger file");
        for row in text.lines().filter(|l| !l.starts_with('#')) {
            let Some(rust_test) = row.split('\t').nth(2) else {
                continue;
            };
            let mut parts = rust_test.split("::");
            let Some(file) = parts.next() else { continue };
            for name in parts.filter(|n| old.is_match(n)) {
                let names = pins.entry(file.to_string()).or_default();
                if !names.iter().any(|n| n == name) {
                    names.push(name.to_string());
                }
            }
        }
    }
    // Longest first, so a name that contains a shorter pinned name is taken out whole.
    for names in pins.values_mut() {
        names.sort_by_key(|n| std::cmp::Reverse(n.len()));
    }
    pins
}

/// `line` with every whole-word occurrence of `names` taken out.
fn without_names(line: &str, names: &[String]) -> String {
    let mut out = line.to_string();
    for name in names {
        let word = Regex::new(&format!(r"\b{}\b", regex::escape(name))).unwrap();
        out = word.replace_all(&out, "").into_owned();
    }
    out
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
    // port ledger pin: a pinned test name is taken out whole, and nothing else is
    let names = vec!["test_rtk_x_y".to_string(), "rtk_x".to_string()];
    assert_eq!(
        without_names("    fn test_rtk_x_y() { rtk_x(); }", &names),
        "    fn () { (); }"
    );
    assert_eq!(without_names("run rtk_xy", &names), "run rtk_xy");
    let pins = ledger_pins();
    assert!(
        pins["crates/repo-tests/tests/rtk_layout.rs"]
            .iter()
            .any(|n| n == "rtk_source_entry_test"),
        "{pins:?}"
    );
    for path in [
        "atl/LICENSE",
        "atl/CHANGELOG.md",
        "CHANGELOG.md",
        "DECISIONS.md",
        "PLAN.md",
        "port/difftest/divergences/guard.check.toml",
        ".claude/agent-memory/anthology-implementer/project-x.md",
        ".claude/memory/reference-project-map.md",
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
fn no_rtk_outside_the_allowed_paths_and_lines() {
    let files = repo::tracked_files().expect("git lists the tracked files here");
    let (old, line) = (old_name(), allowed_line());
    let pins = ledger_pins();
    let mut found: Vec<String> = Vec::new();
    let mut used = [false; PINNED_LINES.len()];
    for path in files {
        if allowed_path(&path) {
            continue;
        }
        if old.is_match(&path) && !line.is_match(&path) && !PINNED_PATHS.contains(&path.as_str()) {
            found.push(format!("{path}: the path itself"));
        }
        let Some(text) = text_of(&path) else { continue };
        let names = pins.get(&path);
        for (n, l) in text.lines().enumerate() {
            let pinned = PINNED_LINES
                .iter()
                .position(|(file, part, _)| *file == path && l.contains(part));
            if let Some(i) = pinned {
                used[i] = true;
                continue;
            }
            // A test name the port ledger pins (port ledger pin) is not an old-name mention.
            let l = match names {
                Some(names) => without_names(l, names),
                None => l.to_string(),
            };
            let l = l.as_str();
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
    for (i, (file, part, _)) in PINNED_LINES.iter().enumerate() {
        assert!(
            used[i],
            "{file}: no line holds {part:?} any more; drop it from PINNED_LINES"
        );
    }
    for path in PINNED_PATHS {
        assert!(
            repo::path(path).is_file(),
            "{path} is gone; drop it from PINNED_PATHS"
        );
    }
}
