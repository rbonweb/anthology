//! Port of `tests/test_docs.py`: the public documents and what the repo may contain.
//!
//! "Tracked files" is `git ls-files` when git can list them, else the whole tree is walked (the
//! Python tests did the same for the Docker images that have no git).

use std::path::Path;

use regex::{Regex, bytes};
use repo_tests::repo;

/// Directories the walk fallback never enters (not repo content). It covers every directory
/// pattern in .gitignore (`.claude/worktrees`, `.vendor-cache`, `__pycache__`, `evals/results`):
/// git never lists them, so the walk must not either. `target` is the cargo build directory
/// (`/target` in .gitignore); the Python list had no such directory.
const PRUNED: [&str; 9] = [
    ".git",
    ".claude/worktrees",
    ".superpowers",
    ".vendor-cache",
    "__pycache__",
    "node_modules",
    "evals/results",
    "port/replay",
    "target",
];

/// The home-path check skips the atl tree (a fork of rtk) and the generated vendored trees.
const HOME_PATH_SKIPS: [&str; 4] = [
    "atl/",
    "plugins/anthology-superpowers/",
    "plugins/anthology/knowledge/",
    "vendor/snapshots/",
];
/// Fixture placeholders, not real people.
const PLACEHOLDER_NAMES: [&str; 6] = ["someone", "tester", "user", "username", "example", "runner"];

/// The owner's name, built by concatenation so this file never contains the home paths it looks for.
const OWNER: &str = concat!("rbon", "web");

const PUBLIC_DOCS: [&str; 4] = [
    "README.md",
    "SECURITY.md",
    "CONTRIBUTING.md",
    "DECISIONS.md",
];

fn markdown_link() -> Regex {
    Regex::new(r#"\[[^\]]*\]\(([^)\s]+)(?:\s+"[^"]*")?\)"#).unwrap()
}

fn heading_re() -> Regex {
    Regex::new(r"(?m)^#{1,6}\s+(.*)$").unwrap()
}

fn code_span_path() -> Regex {
    Regex::new(r"`((?:scripts|\.github/workflows)/[A-Za-z0-9._-]+)`").unwrap()
}

/// Relative POSIX paths of the repo's files.
fn tracked_files() -> Vec<String> {
    match repo::tracked_files() {
        Some(names) => {
            let mut files: Vec<String> = names
                .into_iter()
                .filter(|n| repo::path(n).is_file())
                .collect();
            files.sort();
            files
        }
        None => walk_files(&repo::root()),
    }
}

/// The no-git listing: every file under `root` outside the PRUNED directories.
fn walk_files(root: &Path) -> Vec<String> {
    fn visit(root: &Path, rel_dir: &str, found: &mut Vec<String>) {
        let dir = if rel_dir.is_empty() {
            root.to_path_buf()
        } else {
            root.join(rel_dir)
        };
        let entries = std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
        for entry in entries {
            let entry = entry.unwrap();
            let name = entry.file_name().to_string_lossy().into_owned();
            let rel = if rel_dir.is_empty() {
                name.clone()
            } else {
                format!("{rel_dir}/{name}")
            };
            // os.walk lists a symlink to a directory with the directories and never enters it.
            if entry.path().is_dir() {
                let pruned = PRUNED.contains(&name.as_str()) || PRUNED.contains(&rel.as_str());
                if !pruned && !entry.file_type().unwrap().is_symlink() {
                    visit(root, &rel, found);
                }
                continue;
            }
            // a linked worktree has a `.git` file, not a directory
            if rel == ".git" {
                continue;
            }
            found.push(rel);
        }
    }
    let mut found = Vec::new();
    visit(root, "", &mut found);
    found.sort();
    found
}

fn read_bytes(rel: &str) -> Vec<u8> {
    let p = repo::path(rel);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// A link target without its #fragment and without one leading `./`.
fn clean(target: &str) -> String {
    let target = target.split('#').next().unwrap_or("");
    target.strip_prefix("./").unwrap_or(target).to_string()
}

fn headings(text: &str) -> Vec<String> {
    heading_re()
        .captures_iter(text)
        .map(|c| c[1].trim().to_string())
        .collect()
}

/// The anchor GitHub gives a heading: lower case, punctuation dropped, spaces to hyphens.
fn github_anchor(heading: &str) -> String {
    let lower = heading.trim().to_lowercase();
    let text = Regex::new(r"[`*_]").unwrap().replace_all(&lower, "");
    let text = Regex::new(r"[^\w\- ]").unwrap().replace_all(&text, "");
    text.replace(' ', "-")
}

fn basename(rel: &str) -> &str {
    rel.rsplit('/').next().unwrap_or(rel)
}

mod license_test {
    use super::*;

    #[test]
    fn test_license_is_apache_2() {
        let text = repo::read("LICENSE");
        assert!(text.trim_start().starts_with("Apache License"));
        assert!(text.contains("Version 2.0, January 2004"));
    }

    #[test]
    fn test_license_is_unmodified() {
        // The appendix keeps its template line: nobody's name is filled in.
        assert!(repo::read("LICENSE").contains("Copyright [yyyy] [name of copyright owner]"));
    }
}

mod repo_content_test {
    use super::*;

    #[test]
    fn test_file_listing_is_not_empty() {
        assert!(tracked_files().iter().any(|f| f == "LICENSE"));
    }

    #[test]
    fn test_walk_skips_ignored_eval_results() {
        // The Docker images have no git, so the walk runs there; it must not scan the
        // git-ignored evals/results folders, which hold local paths.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("evals/results/run1")).unwrap();
        std::fs::write(root.join("evals/results/run1/out.txt"), "x").unwrap();
        std::fs::create_dir_all(root.join("evals/case")).unwrap();
        std::fs::write(root.join("evals/case/case.yaml"), "x").unwrap();
        assert_eq!(walk_files(root), ["evals/case/case.yaml"]);
    }

    /// One row per directory pattern of .gitignore (a line ending in `/`).
    #[test]
    fn test_pruned_covers_gitignored_directories() {
        let mut failures = Vec::new();
        for line in repo::read(".gitignore").lines() {
            let line = line.trim();
            if line.ends_with('/') && !line.starts_with(['#', '!']) {
                let dir = line.trim_matches('/');
                if !PRUNED.contains(&dir) {
                    failures.push(format!("{line}: {dir:?} is not in PRUNED"));
                }
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }

    #[test]
    fn test_no_real_home_paths() {
        let home_path = bytes::Regex::new(r"/(?:Users|home)/([A-Za-z0-9._-]+)/").unwrap();
        let mut hits = Vec::new();
        for rel in tracked_files() {
            if HOME_PATH_SKIPS.iter().any(|s| rel.starts_with(s)) {
                continue;
            }
            let data = read_bytes(&rel);
            for m in home_path.captures_iter(&data) {
                let name = String::from_utf8_lossy(&m[1]).to_string();
                if !PLACEHOLDER_NAMES.contains(&name.to_lowercase().as_str()) {
                    hits.push(format!("{rel}: {}", String::from_utf8_lossy(&m[0])));
                }
            }
        }
        assert!(
            hits.is_empty(),
            "real home path in a public file: {hits:#?}"
        );
    }

    #[test]
    fn test_no_owner_home_path_anywhere() {
        let needles: Vec<Vec<u8>> = ["/Users/", "/home/"]
            .iter()
            .map(|p| format!("{p}{OWNER}").to_lowercase().into_bytes())
            .collect();
        let mut hits = Vec::new();
        for rel in tracked_files() {
            let data = read_bytes(&rel).to_ascii_lowercase();
            if needles
                .iter()
                .any(|n| data.windows(n.len()).any(|w| w == n.as_slice()))
            {
                hits.push(rel);
            }
        }
        assert!(
            hits.is_empty(),
            "the owner's home path is in a public file: {hits:#?}"
        );
    }

    #[test]
    fn test_no_env_files() {
        let bad: Vec<String> = tracked_files()
            .into_iter()
            .filter(|rel| basename(rel).starts_with(".env"))
            .collect();
        assert!(bad.is_empty(), "{bad:?}");
    }

    #[test]
    fn test_no_codeowners_file() {
        // The README recommends CODEOWNERS for users' own memory folders; this repo has none.
        let bad: Vec<String> = tracked_files()
            .into_iter()
            .filter(|rel| basename(rel) == "CODEOWNERS")
            .collect();
        assert!(bad.is_empty(), "{bad:?}");
    }
}

mod security_doc_test {
    use super::*;

    fn text() -> String {
        repo::read("SECURITY.md")
    }

    #[test]
    fn test_names_the_rtk_allow() {
        assert!(text().contains("Bash(rtk *)"));
    }

    #[test]
    fn test_has_memory_trust_model_heading() {
        let re = Regex::new(r"(?i)memory trust model").unwrap();
        assert!(
            headings(&text()).iter().any(|h| re.is_match(h)),
            "SECURITY.md needs a heading about the memory trust model"
        );
    }

    #[test]
    fn test_reporting_route_has_no_email() {
        let text = text();
        assert!(
            !Regex::new(r"[\w.+-]+@[\w-]+\.[\w.-]+")
                .unwrap()
                .is_match(&text)
        );
        assert!(
            text.to_lowercase()
                .contains("private vulnerability reporting")
        );
    }
}

mod contributing_doc_test {
    use super::*;

    fn targets() -> Vec<String> {
        markdown_link()
            .captures_iter(&repo::read("CONTRIBUTING.md"))
            .map(|c| c[1].to_string())
            .collect()
    }

    /// One row per tool. omitted: `scripts/vendor.py` (a Python artifact; F-12/F-17 rewrite the
    /// link) -> CO2 replacement: cutover_docs.rs asserts CONTRIBUTING.md links `scripts/dev.sh`.
    #[test]
    fn test_links_the_maintainer_tools() {
        let cleaned: Vec<String> = targets().iter().map(|t| clean(t)).collect();
        let mut failures = Vec::new();
        for want in ["scripts/sync-rtk.sh", "scripts/release.sh"] {
            if !cleaned.iter().any(|c| c == want) {
                failures.push(format!("{want} is not linked from CONTRIBUTING.md"));
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }

    #[test]
    fn test_links_the_evals() {
        assert!(
            targets()
                .iter()
                .map(|t| clean(t))
                .any(|p| p == "evals" || p.starts_with("evals/")),
            "CONTRIBUTING.md must link evals/"
        );
    }
}

mod readme_test {
    use super::*;

    /// One row per install command.
    #[test]
    fn test_install_commands() {
        let text = repo::read("README.md");
        let mut failures = Vec::new();
        for cmd in [
            "/plugin marketplace add rbonweb/anthology",
            "/plugin install anthology@anthology",
            "/anthology:setup",
        ] {
            if !text.contains(cmd) {
                failures.push(format!("README.md lacks {cmd}"));
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }

    #[test]
    fn test_states_the_limits() {
        let text = repo::read("README.md");
        assert!(
            text.to_lowercase()
                .contains("repo memory is visible to everyone who can see the repo")
        );
        assert!(text.contains("Bash(rtk *)"));
    }

    /// One row per public document.
    #[test]
    fn test_no_credit_sections() {
        let credit = Regex::new(r"(?i)credit|acknowledg|thanks").unwrap();
        let mut failures = Vec::new();
        for name in PUBLIC_DOCS {
            for h in headings(&repo::read(name)) {
                if credit.is_match(&h) {
                    failures.push(format!("{name}: credit heading {h:?}"));
                }
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }

    #[test]
    fn test_codeowners_example_is_a_placeholder() {
        assert!(repo::read("README.md").contains("@your-org/reviewers"));
    }
}

mod link_test {
    use super::*;

    #[test]
    fn test_relative_links_resolve() {
        let scheme = Regex::new(r"^[a-z][a-z0-9+.-]*:").unwrap(); // https:, mailto:
        let mut problems = Vec::new();
        for doc in PUBLIC_DOCS {
            let text = repo::read(doc);
            for m in markdown_link().captures_iter(&text) {
                let target = &m[1];
                if scheme.is_match(target) {
                    continue;
                }
                let (path, fragment) = match target.split_once('#') {
                    Some((p, f)) => (p, f),
                    None => (target, ""),
                };
                let rel = if path.is_empty() {
                    doc.to_string()
                } else {
                    clean(path)
                };
                let dest = repo::root().join(&rel);
                if !dest.exists() {
                    problems.push(format!("{doc}: {target} does not exist"));
                } else if !fragment.is_empty()
                    && dest.is_file()
                    && dest.extension().is_some_and(|e| e == "md")
                {
                    let body = std::fs::read_to_string(&dest).unwrap();
                    let anchors: Vec<String> =
                        headings(&body).iter().map(|h| github_anchor(h)).collect();
                    if !anchors.iter().any(|a| a == fragment) {
                        problems.push(format!("{doc}: {target} has no such heading"));
                    }
                }
            }
        }
        assert!(problems.is_empty(), "{problems:#?}");
    }

    #[test]
    fn test_named_scripts_and_workflows_exist() {
        let mut problems = Vec::new();
        // DECISIONS names paths in upstream trees
        for doc in ["README.md", "SECURITY.md", "CONTRIBUTING.md"] {
            for m in code_span_path().captures_iter(&repo::read(doc)) {
                let rel = &m[1];
                if !repo::path(rel).is_file() {
                    problems.push(format!("{doc}: `{rel}` does not exist"));
                }
            }
        }
        assert!(problems.is_empty(), "{problems:#?}");
    }
}

mod decisions_doc_test {
    use super::*;

    /// One row per required field.
    #[test]
    fn test_entries_are_adr_style() {
        let text = repo::read("DECISIONS.md");
        let adrs: Vec<String> = Regex::new(r"(?m)^## ADR-(\d{3}): .+$")
            .unwrap()
            .captures_iter(&text)
            .map(|c| c[1].to_string())
            .collect();
        assert!(adrs.len() >= 6, "{} ADRs", adrs.len());
        let mut sorted = adrs.clone();
        sorted.sort();
        assert_eq!(adrs, sorted);
        let mut failures = Vec::new();
        for field in [
            "**Date:**",
            "**Task:**",
            "**Status:** Accepted",
            "**Context**",
            "**Decision**",
            "**Consequences**",
        ] {
            let n = text.matches(field).count();
            if n != adrs.len() {
                failures.push(format!("{field}: {n} times for {} ADRs", adrs.len()));
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }
}
