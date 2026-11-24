//! CO2: the docs once the Python is gone (plan CO2). The replacement the ledger names for
//! test_docs's `scripts/vendor.py` link, the PLAN "Runtime scripts" line, and the plan's
//! `python3?` mention allowlist: after the cutover no tracked file outside the areas below
//! mentions Python, except the single lines listed with their reason.

use std::path::Path;

use regex::Regex;
use repo_tests::repo;

/// Whether CO2 has moved `port/staging/` into place (the switch of `anthology_testkit::staged`).
fn cut_over() -> bool {
    !repo::path("port/staging").is_dir()
}

/// The markdown link targets of `text`.
fn link_targets(text: &str) -> Vec<String> {
    Regex::new(r"\]\(([^)\s]+)\)")
        .unwrap()
        .captures_iter(text)
        .map(|m| m[1].to_string())
        .collect()
}

/// test_docs `test_links_the_maintainer_tools`, the Rust tools: CONTRIBUTING links
/// `scripts/dev.sh`, which runs every maintainer tool (`anthology-dev`).
#[test]
fn contributing_links_the_dev_tool_runner() {
    let targets = link_targets(&repo::read("CONTRIBUTING.md"));
    assert!(
        targets.iter().any(|t| t == "scripts/dev.sh"),
        "CONTRIBUTING.md must link scripts/dev.sh"
    );
}

/// The PLAN "Runtime scripts" line names Rust, not Python.
#[test]
fn the_plan_runtime_line_is_rust() {
    let plan = repo::read("docs/history/PLAN.md");
    let line = plan
        .lines()
        .find(|l| l.starts_with("- **Runtime scripts:**"))
        .expect("docs/history/PLAN.md: the Runtime scripts line");
    assert!(line.contains("**Rust**"), "{line}");
    assert!(!line.to_lowercase().contains("python"), "{line}");
}

/// Areas where a Python mention is history or someone else's text: (path or folder prefix, why).
const ALLOWED_AREAS: [(&str, &str); 14] = [
    (
        ".claude/agent-memory/",
        "agent memory notes are history, not shipped docs",
    ),
    (
        ".claude/memory/",
        "repo memory notes are history, not shipped docs",
    ),
    (
        "crates/",
        "the port's sources cite the Python behaviour they reproduce",
    ),
    ("docs/superpowers/", "the port's plan and design"),
    ("port/", "the port's ledgers, divergences and reports"),
    ("atl/", "the fork of rtk, renamed"),
    ("vendor/", "upstream sources, patches and snapshots"),
    ("plugins/anthology-superpowers/", "vendored upstream"),
    ("plugins/anthology/knowledge/", "vendored upstream"),
    ("fuzz/corpus/", "fuzz inputs: shell commands users run"),
    ("DECISIONS.md", "the ADRs are history"),
    ("CHANGELOG.md", "history"),
    (
        "docs/history/PLAN.md",
        "the approved product plan; CO2 changes only its Runtime scripts line",
    ),
    (
        "evals/route-implementer/case.yaml",
        "the eval's task is a Python script the user asks for",
    ),
];

/// Single lines elsewhere that mention Python on purpose: (file, a substring of the line, why).
const ALLOWED_LINES: [(&str, &str, &str); 8] = [
    (
        "docs/guardrails.md",
        "`python3 -c`",
        "the user's interpreter code the guard cannot read",
    ),
    (
        "docs/atl.md",
        "`python -c`",
        "the user's code hidden from a parser",
    ),
    (
        "SECURITY.md",
        "`python3 -c`",
        "the user's interpreter code the guard cannot read",
    ),
    (
        "SECURITY.md",
        "`python -c`",
        "the user's programs atl cannot read inside",
    ),
    (
        "scripts/atl-trim.sh",
        "no Python",
        "says that no Python runs",
    ),
    (
        "scripts/atl-rename.sh",
        "Python test ids",
        "names the upstream test ids the rename keeps",
    ),
    (
        "scripts/run-evals.sh",
        "No Python runs.",
        "says that no Python runs",
    ),
    (
        "ci/jobs/ledger.sh",
        "CO2 deleted the Python",
        "why the ledger extracts nothing",
    ),
];

/// Every file of the checkout: `git ls-files`, else a walk that skips build and tool folders.
fn files() -> Vec<String> {
    if let Some(names) = repo::tracked_files() {
        return names
            .into_iter()
            .filter(|n| repo::path(n).is_file())
            .collect();
    }
    fn visit(root: &Path, rel: &str, out: &mut Vec<String>) {
        let dir = if rel.is_empty() {
            root.to_path_buf()
        } else {
            root.join(rel)
        };
        for e in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
            let e = e.expect("entry");
            let name = e.file_name().to_string_lossy().into_owned();
            let path = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            let ft = e.file_type().expect("file type");
            if ft.is_dir() {
                let skip = matches!(name.as_str(), "target" | "node_modules")
                    || (name.starts_with('.') && name != ".github");
                if !skip {
                    visit(root, &path, out);
                }
            } else if ft.is_file() {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    visit(&repo::root(), "", &mut out);
    out
}

/// The plan's acceptance "The `python3?` mention allowlist check passes": after the cutover,
/// every mention is in an allowed area or on an allowed line, and every allowed line still exists.
#[test]
fn after_the_cutover_python_is_mentioned_only_where_allowed() {
    if !cut_over() {
        eprintln!("skipped: before the cutover (port/staging/ exists)");
        return;
    }
    let word = Regex::new(r"(?i)\bpython3?\b").unwrap();
    let mut problems = Vec::new();
    let mut used = vec![false; ALLOWED_LINES.len()];
    for rel in files() {
        if ALLOWED_AREAS.iter().any(|(area, _)| rel.starts_with(area)) {
            continue;
        }
        let bytes = std::fs::read(repo::path(&rel)).expect("read");
        let text = String::from_utf8_lossy(&bytes);
        for (n, line) in text.lines().enumerate() {
            if !word.is_match(line) {
                continue;
            }
            match ALLOWED_LINES
                .iter()
                .position(|(f, part, _)| *f == rel && line.contains(part))
            {
                Some(i) => used[i] = true,
                None => problems.push(format!("{rel}:{}: {}", n + 1, line.trim())),
            }
        }
    }
    for (i, (file, part, _)) in ALLOWED_LINES.iter().enumerate() {
        if !used[i] {
            problems.push(format!(
                "{file}: no line with {part:?} is left; drop its entry"
            ));
        }
    }
    assert!(problems.is_empty(), "{problems:#?}");
    // `git ls-files '*.py'` lists only the vendored superpowers files.
    for rel in files() {
        if rel.ends_with(".py") {
            assert!(rel.starts_with("plugins/anthology-superpowers/"), "{rel}");
        }
    }
}
