//! Port of `tests/test_licenses.py`: every installed plugin directory carries its license text.

use anthology_pycompat::json::{self, Map, Value};
use regex::Regex;
use repo_tests::repo;

const KNOWLEDGE: &str = repo::KNOWLEDGE;
const LICENSE_NAMES: [&str; 3] = ["LICENSE", "LICENSE.txt", "LICENSE.md"];

/// One `vendor/sources.json` entry (the dict `vendor.py` loads).
struct Source(Map);

impl Source {
    fn get(&self, key: &str) -> Option<&Value> {
        self.0.get(key)
    }

    /// The string under `key`; `None` for a missing key, null, or a non-string.
    fn str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Value::as_str)
    }

    fn name(&self) -> &str {
        self.str("name").expect("a source has a name")
    }

    fn target(&self) -> &str {
        self.str("target").expect("a source has a target")
    }

    /// `e["license_file"] and posixpath.basename(e["license_file"]) or "LICENSE"`.
    fn license_name(&self) -> String {
        match self.str("license_file") {
            Some(f) if !f.is_empty() => match f.rsplit('/').next() {
                Some(base) if !base.is_empty() => base.to_string(),
                _ => "LICENSE".to_string(),
            },
            _ => "LICENSE".to_string(),
        }
    }

    /// `vendor.source_slug`: `vendor/licenses/<slug>.txt`; owner-repo for git sources, else the
    /// entry name.
    fn slug(&self) -> String {
        let slug = match self.str("repo").filter(|r| !r.is_empty()) {
            Some(repo) => {
                let mut parts: Vec<&str> = repo
                    .trim_end_matches('/')
                    .split('/')
                    .filter(|p| !p.is_empty())
                    .collect();
                if let Some(last) = parts.last_mut() {
                    *last = last.strip_suffix(".git").unwrap_or(last);
                }
                parts[parts.len().saturating_sub(2)..].join("-")
            }
            None => self.name().to_string(),
        };
        Regex::new(r"[^A-Za-z0-9._-]")
            .unwrap()
            .replace_all(&slug, "-")
            .to_lowercase()
    }
}

/// `vendor.vendored(vendor.load_sources(ROOT))` restricted to the knowledge targets: the entries
/// vendor.py builds (subtree-managed ones, like atl, are skipped) that land in the knowledge folder.
fn knowledge_sources() -> Vec<Source> {
    let data = json::loads(&repo::read("vendor/sources.json")).expect("sources.json is JSON");
    let sources = data
        .as_object()
        .and_then(|o| o.get("sources"))
        .and_then(Value::as_array)
        .expect("sources.json has a sources list");
    sources
        .iter()
        .map(|v| Source(v.as_object().expect("a source is an object").clone()))
        .filter(|e| e.str("managed_by") != Some("subtree"))
        .filter(|e| e.target().starts_with("plugins/anthology/knowledge/"))
        .collect()
}

fn source_named(name: &str) -> Source {
    knowledge_sources()
        .into_iter()
        .find(|e| e.name() == name)
        .unwrap_or_else(|| panic!("no source {name}"))
}

fn is_file(rel: &str) -> bool {
    repo::path(rel).is_file()
}

fn read_bytes(rel: &str) -> Vec<u8> {
    let p = repo::path(rel);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn dirs_in(rel: &str) -> Vec<String> {
    repo::list_dir(rel)
        .into_iter()
        .filter(|n| repo::path(&format!("{rel}/{n}")).is_dir())
        .collect()
}

mod knowledge_license_test {
    use super::*;

    #[test]
    fn test_every_knowledge_folder_has_a_license() {
        let folders = dirs_in(KNOWLEDGE);
        assert_eq!(folders.len(), 57);
        for folder in folders {
            assert!(
                LICENSE_NAMES
                    .iter()
                    .any(|n| is_file(&format!("{KNOWLEDGE}/{folder}/{n}"))),
                "{folder} ships no LICENSE, LICENSE.txt or LICENSE.md"
            );
        }
    }

    #[test]
    fn test_every_knowledge_source_ships_the_license_it_names() {
        let sources = knowledge_sources();
        assert_eq!(sources.len(), 56);
        for e in sources {
            let name = e.license_name();
            let shipped = format!("{}/{name}", e.target());
            assert!(is_file(&shipped), "{}: {name} is not shipped", e.name());
            assert!(
                !read_bytes(&shipped).trim_ascii().is_empty(),
                "{}: {name} is empty",
                e.name()
            );
        }
    }

    #[test]
    fn test_licenses_from_vendor_licenses_match_the_fixed_text() {
        let mut checked = 0;
        for e in knowledge_sources() {
            // a URL source has no fixed text under vendor/licenses
            if e.str("url").is_some_and(|u| !u.is_empty()) {
                continue;
            }
            let expected = read_bytes(&format!("vendor/licenses/{}.txt", e.slug()));
            let shipped = read_bytes(&format!("{}/{}", e.target(), e.license_name()));
            assert_eq!(shipped, expected, "{}", e.name());
            checked += 1;
        }
        assert_eq!(checked, 55);
    }

    #[test]
    fn test_the_fixed_text_ships_as_license_for_web_design_guidelines() {
        let e = source_named("web-design-guidelines");
        assert!(matches!(e.get("license_file"), Some(Value::Null)));
        assert_eq!(
            read_bytes(&format!("{}/LICENSE", e.target())),
            read_bytes("vendor/licenses/vercel-labs-agent-skills.txt")
        );
    }

    #[test]
    fn test_the_fixed_text_ships_as_license_for_karpathy_guidelines() {
        let e = source_named("karpathy-guidelines");
        assert!(matches!(e.get("license_file"), Some(Value::Null)));
        let text = repo::read(&format!("{}/LICENSE", e.target()));
        assert!(text.contains("MIT License"));
        assert!(text.contains("forrestchang"));
        assert!(
            text.lines()
                .next()
                .unwrap_or("")
                .contains("multica-ai/andrej-karpathy-skills")
        );
    }

    #[test]
    fn test_caveman_ships_its_notice_and_the_mit_text() {
        let caveman = format!("{KNOWLEDGE}/caveman");
        assert!(!repo::read(&format!("{caveman}/NOTICE")).trim().is_empty());
        assert!(repo::read(&format!("{caveman}/LICENSE-MIT")).contains("MIT License"));
        let entry = source_named("caveman");
        let extra: Vec<&str> = entry
            .get("license_extra")
            .and_then(Value::as_array)
            .expect("license_extra is a list")
            .iter()
            .map(|v| v.as_str().expect("a string"))
            .collect();
        assert_eq!(extra, ["NOTICE", "LICENSE-MIT"]);
    }
}

mod plugin_root_license_test {
    use super::*;

    #[test]
    fn test_every_plugin_root_has_a_license() {
        let plugins = dirs_in("plugins");
        assert_eq!(
            plugins,
            ["anthology", "anthology-guardrails", "anthology-superpowers"]
        );
        for plugin in plugins {
            assert!(
                LICENSE_NAMES
                    .iter()
                    .any(|n| is_file(&format!("plugins/{plugin}/{n}"))),
                "{plugin} ships no LICENSE"
            );
        }
    }

    #[test]
    fn test_first_party_plugins_carry_the_repo_license() {
        let text = read_bytes("LICENSE");
        assert!(text.windows(14).any(|w| w == b"Apache License"));
        for name in ["anthology", "anthology-guardrails"] {
            assert_eq!(
                read_bytes(&format!("plugins/{name}/LICENSE")),
                text,
                "{name}"
            );
        }
    }
}
