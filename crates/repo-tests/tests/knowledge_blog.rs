//! The claude-blog knowledge folders: seven sub-skills plus the `blog` support folder (shared
//! references and templates, no `SKILL.md`). Pins the sources, that no file sends an agent to an
//! upstream script or paid route, that no relative link dangles, and the discourse and score-gate
//! text `blog-writer` relies on.

use std::path::Path;

use anthology_pycompat::json::{self, Value};
use regex::Regex;
use repo_tests::{links, repo};

const BLOG: [&str; 7] = [
    "blog",
    "blog-discourse",
    "blog-brief",
    "blog-outline",
    "blog-write",
    "blog-factcheck",
    "blog-cluster",
];

const REPO: &str = "https://github.com/AgriciDaniel/claude-blog";
const REF: &str = "v2.3.2";
const COMMIT: &str = "4f9c2cb8fd17f594e868d6c2ce0e8fb2c971f987";

fn folder(name: &str) -> String {
    format!("{}/{name}", repo::KNOWLEDGE)
}

/// Every file below `dir`, as a repo-relative `/`-separated path, sorted.
fn files_below(dir: &str) -> Vec<String> {
    fn walk(root: &Path, rel: &str, out: &mut Vec<String>) {
        let mut names: Vec<String> = std::fs::read_dir(root.join(rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}"))
            .map(|entry| entry.expect("a directory entry").file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .collect();
        names.sort();
        for name in names {
            let child = format!("{rel}/{name}");
            if root.join(&child).is_dir() {
                walk(root, &child, out);
            } else {
                out.push(child);
            }
        }
    }
    let mut out = Vec::new();
    walk(&repo::root(), dir, &mut out);
    out
}

/// The text of every file in the seven folders, with its path.
fn blog_texts() -> Vec<(String, String)> {
    BLOG.iter()
        .flat_map(|name| files_below(&folder(name)))
        .map(|rel| {
            let bytes = std::fs::read(repo::path(&rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
            (rel, String::from_utf8_lossy(&bytes).into_owned())
        })
        .collect()
}

fn source(name: &str) -> Value {
    let data = json::loads(&repo::read("vendor/sources.json")).expect("sources.json is JSON");
    data.as_object()
        .and_then(|o| o.get("sources"))
        .and_then(Value::as_array)
        .expect("sources.json has a sources list")
        .iter()
        .find(|e| {
            e.as_object()
                .and_then(|o| o.get("name"))
                .and_then(Value::as_str)
                == Some(name)
        })
        .unwrap_or_else(|| panic!("no source {name}"))
        .clone()
}

fn field<'a>(entry: &'a Value, key: &str) -> &'a Value {
    entry
        .as_object()
        .and_then(|o| o.get(key))
        .unwrap_or_else(|| panic!("missing key {key}"))
}

fn text(entry: &Value, key: &str) -> String {
    field(entry, key)
        .as_str()
        .unwrap_or_else(|| panic!("{key} is not a string"))
        .to_string()
}

#[test]
fn the_blog_sources_are_pinned() {
    let drop_blog = r#"{"name":"drop-paths","paths":["SKILL.md","references/blog-delivery-contract.md","references/orchestration-details.md"]}"#;
    let drop_discourse = r#"{"name":"drop-paths","paths":["scripts"]}"#;
    for name in BLOG {
        let entry = source(name);
        assert_eq!(text(&entry, "repo"), REPO, "{name}");
        assert_eq!(text(&entry, "path"), format!("skills/{name}"), "{name}");
        assert_eq!(text(&entry, "ref"), REF, "{name}");
        assert_eq!(text(&entry, "commit"), COMMIT, "{name}");
        assert_eq!(text(&entry, "license"), "MIT", "{name}");
        assert_eq!(text(&entry, "license_file"), "LICENSE", "{name}");
        assert_eq!(text(&entry, "target"), folder(name), "{name}");
        let patch = format!(r#"{{"patch":"vendor/patches/{name}.patch"}}"#);
        let expected = match name {
            "blog" => format!("[{drop_blog},{patch}]"),
            "blog-discourse" => format!("[{drop_discourse},{patch}]"),
            _ => format!("[{patch}]"),
        };
        let expected = json::loads(&expected).expect("expected transform is JSON");
        assert_eq!(field(&entry, "transform"), &expected, "{name}");
    }
}

#[test]
fn the_support_folder_has_references_and_templates_but_no_skill() {
    let blog = folder("blog");
    assert!(!repo::path(&format!("{blog}/SKILL.md")).exists());
    assert!(repo::path(&format!("{blog}/LICENSE")).is_file());
    assert!(repo::path(&format!("{blog}/references/quality-scoring.md")).is_file());
    assert!(repo::path(&format!("{blog}/templates/how-to-guide.md")).is_file());
    for left_out in [
        "references/blog-delivery-contract.md",
        "references/orchestration-details.md",
    ] {
        assert!(
            !repo::path(&format!("{blog}/{left_out}")).exists(),
            "{left_out}"
        );
    }
}

#[test]
fn no_blog_file_runs_the_upstream_pipeline() {
    const BANNED: [&str; 21] = [
        "python3",
        "blog_script_dir",
        "claude_blog_scripts_dir",
        "blog_preflight",
        "generate_hero",
        "blog_render",
        "load_untrusted_root",
        "discourse_research",
        "analyze_blog",
        "quality_gate.py",
        "cognitive_load.py",
        "patchright",
        "playwright",
        "puppeteer",
        "nanobanana",
        "--allow-paid",
        "dataforseo",
        "notebooklm",
        "nonce",
        "dev-browser",
        "browseros",
    ];
    let api_key = Regex::new(r"\$\{?[A-Z0-9_]*API_KEY").unwrap();
    let mut found = Vec::new();
    for (rel, body) in blog_texts() {
        // LICENSE and NOTICE are upstream attribution text, shipped unchanged (`vendor check`
        // enforces it). The NOTICE names the scripts its methods were adapted from; it never
        // tells an agent to run them.
        if rel.ends_with("/LICENSE") || rel.ends_with("/NOTICE") {
            continue;
        }
        let lower = body.to_lowercase();
        for word in BANNED {
            if lower.contains(word) {
                found.push(format!("{rel}: {word}"));
            }
        }
        if api_key.is_match(&body) {
            found.push(format!("{rel}: an API key variable"));
        }
    }
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn every_relative_link_resolves() {
    let dangling: Vec<String> = BLOG
        .iter()
        .flat_map(|name| links::dangling_links(&folder(name)))
        .collect();
    assert!(dangling.is_empty(), "{dangling:#?}");
}

#[test]
fn discourse_uses_the_public_endpoints_quoted() {
    let dir = folder("blog-discourse");
    let skill = repo::read(&format!("{dir}/SKILL.md"));
    for url in [
        "https://hn.algolia.com/api/v1/search?tags=story&query=<q>&numericFilters=created_at_i><epoch>",
        "https://hn.algolia.com/api/v1/search_by_date?tags=story&query=<q>&numericFilters=created_at_i><epoch>",
        "https://www.reddit.com/r/<sub>/top.json?t=month&limit=25",
        "https://www.reddit.com/search.json?q=<q>&sort=top&t=month&limit=25",
    ] {
        assert!(
            skill.contains(&format!("`{url}`")),
            "not in backticks: {url}"
        );
    }
    assert!(
        skill.contains("`curl -s -A 'anthology-blog-writer' '<url>'`"),
        "the quoted curl fallback is missing"
    );
    assert!(!skill.contains("$("), "a subshell in the discourse skill");
    assert!(
        !repo::path(&format!("{dir}/scripts")).exists(),
        "scripts/ is shipped"
    );
}

#[test]
fn blog_write_ends_in_the_score_gate() {
    let skill = repo::read(&format!("{}/SKILL.md", folder("blog-write")));
    assert!(skill.contains("### Phase 6.5: Score gate"));
    assert!(skill.contains("90 or more"));
    assert!(!skill.contains("Delivery Contract Enforcement"));
}
