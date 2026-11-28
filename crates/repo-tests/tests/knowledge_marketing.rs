//! The marketing, landing-page, brand-review and UX-microcopy knowledge folders under
//! `plugins/anthology/knowledge/`: their pins, and the rules Anthology's patches enforce on them.

use std::path::Path;

use anthology_pycompat::json::{self, Value};
use regex::Regex;
use repo_tests::{frontmatter, links, repo};

/// The folders vendored from `coreyhaines31/marketingskills`, one per `skills/<name>`.
const MARKETINGSKILLS: [&str; 14] = [
    "product-marketing",
    "copywriting",
    "copy-editing",
    "emails",
    "social",
    "ad-creative",
    "launch",
    "public-relations",
    "cro",
    "ab-testing",
    "analytics",
    "programmatic-seo",
    "content-strategy",
    "ai-seo",
];

/// The three folders from other upstreams.
const OTHERS: [&str; 3] = ["brand-review", "landing-page-copywriter", "ux-microcopy"];

const MARKETINGSKILLS_REPO: &str = "https://github.com/coreyhaines31/marketingskills";
const MARKETINGSKILLS_COMMIT: &str = "2bf95edb4ebd37198ab7fd18b99ba95c2145ec64";

/// The sentence that replaces every tool-integration table in the marketing knowledge.
const ANTHOLOGY_TOOLS_LINE: &str = "In Anthology, no paid keys, vendor CLIs or marketing-tool MCP servers: work from exports, screenshots or numbers the user gives, and name a paid tool only as an option the user runs.";

/// Substrings no marketing knowledge file may hold (compared in lower case): each sends an agent to
/// something Anthology forbids or does not ship.
const FORBIDDEN: [&str; 13] = [
    "dev-browser",
    "playwright",
    "puppeteer",
    "brew install",
    "pip install",
    "npm install -g",
    "tools/integrations/",
    "tools/clis",
    "tools/registry.md",
    "tools/setup.md",
    "node tools/",
    "~~",
    "connectors.md",
];

fn all_folders() -> Vec<&'static str> {
    MARKETINGSKILLS
        .iter()
        .chain(OTHERS.iter())
        .copied()
        .collect()
}

fn folder(name: &str) -> String {
    format!("{}/{name}", repo::KNOWLEDGE)
}

/// The `vendor/sources.json` entry called `name`.
fn source(name: &str) -> Value {
    let text = repo::read("vendor/sources.json");
    let sources =
        json::loads(&text).unwrap_or_else(|e| panic!("sources.json: {}", e.py_str(&text)));
    sources
        .as_object()
        .and_then(|m| m.get("sources"))
        .and_then(Value::as_array)
        .expect("sources is a list")
        .iter()
        .find(|e| {
            e.as_object()
                .and_then(|m| m.get("name"))
                .and_then(Value::as_str)
                == Some(name)
        })
        .unwrap_or_else(|| panic!("no source {name}"))
        .clone()
}

fn field<'a>(entry: &'a Value, key: &str) -> &'a str {
    entry
        .as_object()
        .and_then(|m| m.get(key))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{key} is not a string in {entry:?}"))
}

/// The files below `rel` (repo-relative, `/`-separated), sorted.
fn files_below(rel: &str) -> Vec<String> {
    let mut out = Vec::new();
    collect(&repo::path(rel), rel, &mut out);
    out.sort();
    out
}

fn collect(dir: &Path, rel: &str, out: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let entry = entry.unwrap();
        let child = format!("{rel}/{}", entry.file_name().to_string_lossy());
        if entry.file_type().unwrap().is_dir() {
            collect(&entry.path(), &child, out);
        } else {
            out.push(child);
        }
    }
}

fn read_lossy(rel: &str) -> String {
    String::from_utf8_lossy(
        &std::fs::read(repo::path(rel)).unwrap_or_else(|e| panic!("{rel}: {e}")),
    )
    .into_owned()
}

#[test]
fn the_marketing_sources_are_pinned() {
    for name in MARKETINGSKILLS {
        let e = source(name);
        assert_eq!(field(&e, "repo"), MARKETINGSKILLS_REPO, "{name}");
        assert_eq!(field(&e, "path"), format!("skills/{name}"), "{name}");
        assert_eq!(field(&e, "ref"), "v2.14.29", "{name}");
        assert_eq!(field(&e, "commit"), MARKETINGSKILLS_COMMIT, "{name}");
        assert_eq!(field(&e, "license"), "MIT", "{name}");
        assert_eq!(field(&e, "target"), folder(name), "{name}");
    }
    let table = [
        (
            "brand-review",
            "https://github.com/anthropics/knowledge-work-plugins",
            "marketing/skills/brand-review",
            "main",
            "0e0fcacddcc892eb92a1b7873ff9d68159c47571",
            "Apache-2.0",
        ),
        (
            "landing-page-copywriter",
            "https://github.com/OneWave-AI/claude-skills",
            "landing-page-copywriter",
            "main",
            "b2267d9171af4f56848d82992381b20b89b44056",
            "MIT",
        ),
        (
            "ux-microcopy",
            "https://github.com/humbleteam/ux-writing",
            "",
            "main",
            "c6a63c59c846c6cadc3d8b1067b4bec531c8f2dc",
            "MIT",
        ),
    ];
    for (name, repo_url, path, git_ref, commit, license) in table {
        let e = source(name);
        assert_eq!(field(&e, "repo"), repo_url, "{name}");
        assert_eq!(field(&e, "path"), path, "{name}");
        assert_eq!(field(&e, "ref"), git_ref, "{name}");
        assert_eq!(field(&e, "commit"), commit, "{name}");
        assert_eq!(field(&e, "license"), license, "{name}");
        assert_eq!(field(&e, "target"), folder(name), "{name}");
    }
}

#[test]
fn no_folder_ships_upstream_evals() {
    for name in MARKETINGSKILLS {
        assert!(
            !repo::path(&format!("{}/evals", folder(name))).exists(),
            "{name} ships evals/"
        );
    }
    assert!(
        !repo::path(&format!(
            "{}/references/generative-tools.md",
            folder("ad-creative")
        ))
        .exists(),
        "ad-creative ships references/generative-tools.md"
    );
}

#[test]
fn no_file_breaks_the_anthology_rules() {
    let api_key = Regex::new(r"\$\{?[A-Z0-9_]*API_KEY").unwrap();
    let mut problems = Vec::new();
    for name in all_folders() {
        for file in files_below(&folder(name)) {
            let text = read_lossy(&file);
            for (index, line) in text.lines().enumerate() {
                let lower = line.to_lowercase();
                let at = format!("{file}:{}", index + 1);
                for word in FORBIDDEN {
                    if lower.contains(word) {
                        problems.push(format!("{at}: {word}"));
                    }
                }
                if lower.contains("npx ") && !lower.contains("docker run --rm") {
                    problems.push(format!("{at}: npx outside docker run --rm"));
                }
                if api_key.is_match(line) {
                    problems.push(format!("{at}: API key variable"));
                }
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn every_relative_link_resolves() {
    let mut dangling = Vec::new();
    for name in all_folders() {
        dangling.extend(links::dangling_links(&folder(name)));
    }
    assert!(dangling.is_empty(), "{}", dangling.join("\n"));
}

#[test]
fn every_folder_is_a_skill_named_after_itself() {
    for name in all_folders() {
        let skill = format!("{}/SKILL.md", folder(name));
        let (meta, _) = frontmatter::load(&skill);
        assert_eq!(meta["name"].as_str(), Some(name), "{skill}");
        assert!(
            repo::path(&format!("{}/LICENSE", folder(name))).is_file(),
            "{name} has no LICENSE"
        );
    }
}

#[test]
fn ux_microcopy_is_renamed_and_minimal() {
    let (meta, _) = frontmatter::load(&format!("{}/SKILL.md", folder("ux-microcopy")));
    assert_eq!(meta["name"].as_str(), Some("ux-microcopy"));
    assert_eq!(
        files_below(&folder("ux-microcopy")),
        [
            format!("{}/LICENSE", folder("ux-microcopy")),
            format!("{}/SKILL.md", folder("ux-microcopy")),
            format!("{}/references/banned-patterns.md", folder("ux-microcopy")),
        ]
    );
}

#[test]
fn the_tool_line_replaces_every_tool_table() {
    for name in ["emails", "analytics", "ad-creative"] {
        let skill = repo::read(&format!("{}/SKILL.md", folder(name)));
        assert!(
            skill.contains(ANTHOLOGY_TOOLS_LINE),
            "{name}/SKILL.md lacks the tool line"
        );
    }
}
