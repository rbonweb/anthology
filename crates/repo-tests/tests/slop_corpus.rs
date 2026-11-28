//! The slop rules against the human docs: every rule must stay silent on the front documents and
//! every page under `docs/`. A finding here means a pattern is too broad; narrow the rule and
//! add the sentence as a negative test, never a waiver in the docs.

use std::path::Path;

use anthology_slop::{Kind, check_all};
use repo_tests::repo;

const FRONT: [&str; 5] = [
    "README.md",
    "CONTRIBUTING.md",
    "SECURITY.md",
    "DECISIONS.md",
    "CHANGELOG.md",
];

/// Every `*.md` under `dir` (repo-relative, `/`-separated), sorted.
fn markdown_below(dir: &str, out: &mut Vec<String>) {
    let mut entries: Vec<_> = std::fs::read_dir(repo::path(dir))
        .unwrap_or_else(|e| panic!("{dir}: {e}"))
        .map(|e| e.expect("dir entry"))
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = format!("{dir}/{name}");
        let kind = entry.file_type().expect("file type");
        if kind.is_dir() {
            markdown_below(&rel, out);
        } else if kind.is_file() && Path::new(&name).extension().is_some_and(|x| x == "md") {
            out.push(rel);
        }
    }
}

/// The corpus: the front documents, then every docs page.
fn corpus() -> Vec<String> {
    let mut files: Vec<String> = FRONT.iter().map(|f| f.to_string()).collect();
    markdown_below("docs", &mut files);
    files
}

#[test]
fn the_rules_flag_nothing_in_the_human_docs() {
    let mut found = Vec::new();
    for rel in corpus() {
        let text = repo::read(&rel);
        for f in check_all(Kind::Markdown, &text) {
            if f.waiver.is_none() {
                found.push(format!("{rel}:{}: {} \"{}\"", f.line, f.rule, f.excerpt));
            }
        }
    }
    assert!(found.is_empty(), "{}", found.join("\n"));
}

#[test]
fn the_corpus_holds_the_front_docs_and_the_pages() {
    let files = corpus();
    assert!(files.len() >= 12, "{files:?}");
    for want in ["README.md", "docs/agents.md"] {
        assert!(files.iter().any(|f| f == want), "{want} in {files:?}");
    }
}
