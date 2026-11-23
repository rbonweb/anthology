//! `tests/test_repo_layout.py`: the marketplace, plugin manifests, pinned images and vendored trees.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anthology_pycompat::json::{self, Value};
use regex::Regex;
use repo_tests::yaml::Yaml;
use repo_tests::{frontmatter, repo};

const PLUGINS: [&str; 3] = ["anthology", "anthology-guardrails", "anthology-superpowers"];

/// `SEMVER` of the Python test; its `$` also matches before one trailing newline, so `\n?\z`.
const SEMVER: &str = r"^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?\n?\z";

const DESIGN_SKILLS: [&str; 10] = [
    "accessibility-audit",
    "design-critique",
    "design-elevation",
    "design-ops",
    "design-systems",
    "interaction-design",
    "journey-mapping",
    "ux-research",
    "ux-strategy",
    "ux-writing",
];

fn read_json(rel: &str) -> Value {
    let text = repo::read(rel);
    json::loads(&text).unwrap_or_else(|e| panic!("{rel}: {}", e.py_str(&text)))
}

/// `value[key]` of a JSON object; panics like a Python `KeyError`.
fn get<'a>(value: &'a Value, key: &str) -> &'a Value {
    value
        .as_object()
        .and_then(|m| m.get(key))
        .unwrap_or_else(|| panic!("KeyError: {key:?} in {value:?}"))
}

fn str_of(value: &Value) -> &str {
    value
        .as_str()
        .unwrap_or_else(|| panic!("not a string: {value:?}"))
}

fn items(value: &Value) -> &[Value] {
    value
        .as_array()
        .unwrap_or_else(|| panic!("not an array: {value:?}"))
}

/// Python truthiness of a JSON value (`assertTrue(x)`).
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Int(i) => *i != json::Int::Small(0),
        Value::Float(f) => *f != 0.0,
        Value::Str(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(m) => !m.is_empty(),
    }
}

/// Python truthiness of a YAML value (`assertTrue(meta["description"])`); a missing key is falsy.
fn yaml_truthy(value: &Yaml) -> bool {
    match value {
        Yaml::Null | Yaml::BadValue => false,
        Yaml::Boolean(b) => *b,
        Yaml::Integer(i) => *i != 0,
        Yaml::Real(r) => r.parse::<f64>().is_ok_and(|f| f != 0.0),
        Yaml::String(s) => !s.is_empty(),
        Yaml::Array(a) => !a.is_empty(),
        Yaml::Hash(h) => !h.is_empty(),
        Yaml::Alias(_) => true,
    }
}

/// Python's `str.isspace` for one char: Unicode White_Space plus U+001C..U+001F.
fn is_py_space(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}

fn py_strip(s: &str) -> &str {
    s.trim_matches(is_py_space)
}

/// Python's `str.splitlines()`: all its line boundaries, `\r\n` as one, no trailing empty line.
fn py_splitlines(s: &str) -> Vec<&str> {
    let is_break = |c: char| {
        matches!(
            c,
            '\n' | '\r'
                | '\x0b'
                | '\x0c'
                | '\x1c'
                | '\x1d'
                | '\x1e'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        )
    };
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if !is_break(c) {
            continue;
        }
        lines.push(&s[start..i]);
        start = i + c.len_utf8();
        if c == '\r' && matches!(chars.peek(), Some((_, '\n'))) {
            chars.next();
            start += 1;
        }
    }
    if start < s.len() {
        lines.push(&s[start..]);
    }
    lines
}

/// `sorted(Path(root).rglob("*"))` files (`is_file`, symlinks to files included), as repo-relative
/// paths; `rglob` does not descend into symlinked directories.
fn rglob_files(rel: &str) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.map(|e| e.unwrap()) {
            let path = entry.path();
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                walk(&path, out);
            } else if kind.is_file() || (kind.is_symlink() && path.is_file()) {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(&repo::path(rel), &mut out);
    out.sort();
    out
}

/// `files_under`: the regular files below `rel`, symlinks excluded.
fn files_under(rel: &str) -> Vec<PathBuf> {
    rglob_files(rel)
        .into_iter()
        .filter(|p| !p.is_symlink())
        .collect()
}

/// `vendor.is_subtree`.
fn is_subtree(entry: &Value) -> bool {
    entry
        .as_object()
        .and_then(|m| m.get("managed_by"))
        .and_then(Value::as_str)
        == Some("subtree")
}

/// `vendor.source_slug`: `vendor/licenses/<slug>.txt`, owner-repo for git sources.
fn source_slug(entry: &Value) -> String {
    let repo_url = entry
        .as_object()
        .and_then(|m| m.get("repo"))
        .and_then(Value::as_str)
        .filter(|r| !r.is_empty());
    let slug = match repo_url {
        Some(url) => {
            let mut parts: Vec<&str> = url
                .trim_end_matches('/')
                .split('/')
                .filter(|p| !p.is_empty())
                .collect();
            if let Some(last) = parts.last_mut()
                && let Some(stripped) = last.strip_suffix(".git")
            {
                *last = stripped;
            }
            parts[parts.len().saturating_sub(2)..].join("-")
        }
        None => str_of(get(entry, "name")).to_string(),
    };
    slug.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

mod marketplace_test {
    use super::*;

    /// `setUp`: every test reads the marketplace first.
    fn set_up() -> Value {
        read_json(".claude-plugin/marketplace.json")
    }

    fn read_manifest(name: &str) -> Value {
        let manifest = read_json(&format!("plugins/{name}/.claude-plugin/plugin.json"));
        assert_eq!(str_of(get(&manifest, "name")), name);
        manifest
    }

    fn check_first_party(name: &str) -> Value {
        let manifest = read_manifest(name);
        let version = str_of(get(&manifest, "version"));
        assert!(Regex::new(SEMVER).unwrap().is_match(version), "{version}");
        assert_eq!(str_of(get(&manifest, "license")), "Apache-2.0");
        manifest
    }

    #[test]
    fn test_marketplace_name_and_plugins() {
        let marketplace = set_up();
        assert_eq!(str_of(get(&marketplace, "name")), "anthology");
        let plugins = items(get(&marketplace, "plugins"));
        let names: BTreeSet<&str> = plugins.iter().map(|p| str_of(get(p, "name"))).collect();
        assert_eq!(names, BTreeSet::from(PLUGINS));
        for p in plugins {
            let name = str_of(get(p, "name"));
            assert_eq!(str_of(get(p, "source")), format!("./plugins/{name}"));
        }
    }

    #[test]
    fn test_anthology_manifest() {
        set_up();
        let manifest = check_first_party("anthology");
        let deps: BTreeMap<&str, &Value> = items(get(&manifest, "dependencies"))
            .iter()
            .map(|d| (str_of(get(d, "name")), get(d, "version")))
            .collect();
        let names: BTreeSet<&str> = deps.keys().copied().collect();
        assert_eq!(
            names,
            BTreeSet::from(["anthology-guardrails", "anthology-superpowers"])
        );
        for version in deps.values() {
            assert!(truthy(version), "dependency needs a version range");
        }
        assert!(truthy(get(get(&manifest, "engines"), "claude-code")));
    }

    #[test]
    fn test_guardrails_manifest() {
        set_up();
        check_first_party("anthology-guardrails");
    }

    // anthology-superpowers is vendored: the upstream license stays MIT and it has its
    // own version, so it is not held to the first-party license check.
    #[test]
    fn test_superpowers_manifest() {
        set_up();
        let manifest = read_manifest("anthology-superpowers");
        assert_eq!(str_of(get(&manifest, "license")), "MIT");
        let version = str_of(get(&manifest, "version"));
        assert!(Regex::new(SEMVER).unwrap().is_match(version), "{version}");
        let sources = read_json("vendor/sources.json");
        let source = items(get(&sources, "sources"))
            .iter()
            .find(|s| str_of(get(s, "name")) == "superpowers")
            .expect("no superpowers source");
        assert_eq!(version, str_of(get(source, "plugin_version")));
        let anthology = read_json("plugins/anthology/.claude-plugin/plugin.json");
        let range = items(get(&anthology, "dependencies"))
            .iter()
            .find(|d| str_of(get(d, "name")) == "anthology-superpowers")
            .map(|d| str_of(get(d, "version")))
            .expect("KeyError: anthology-superpowers");
        // `major, minor, _ = version.split(".")` raises unless there are exactly three parts.
        let parts: Vec<&str> = version.split('.').collect();
        let [major, minor, _] = parts[..] else {
            panic!("ValueError: cannot unpack {version:?}");
        };
        assert!(
            range
                .trim_start_matches(['~', '^'])
                .starts_with(&format!("{major}.{minor}")),
            "the dependency range must resolve to the vendored version"
        );
    }
}

/// Each pinned toolchain image is a one-line Dockerfile, so Dependabot can bump it.
mod pinned_images_test {
    use super::*;

    // omitted: the exact file set including python.Dockerfile and python39.Dockerfile (F-16
    // removes them) -> CO2 replacement: cutover_ci.rs asserts ci/images holds exactly the seven
    // Dockerfiles rust, gitleaks, shellcheck, alpine, debian-slim, ubuntu and node. The set below
    // holds before and after the cutover; every Dockerfile, python ones included, is still
    // held to the one pinned FROM line.
    #[test]
    fn test_dockerfiles_pinned_by_digest() {
        let from = Regex::new(r"^FROM \S+:\S+@sha256:[0-9a-f]{64}$").unwrap();
        let names: Vec<String> = repo::list_dir("ci/images")
            .into_iter()
            .filter(|n| n.ends_with(".Dockerfile"))
            .collect();
        let without_python: BTreeSet<&str> = names
            .iter()
            .map(String::as_str)
            .filter(|n| !matches!(*n, "python.Dockerfile" | "python39.Dockerfile"))
            .collect();
        assert_eq!(
            without_python,
            BTreeSet::from([
                "rust.Dockerfile",
                "gitleaks.Dockerfile",
                "shellcheck.Dockerfile",
                "alpine.Dockerfile",
                "debian-slim.Dockerfile",
                "ubuntu.Dockerfile",
                "node.Dockerfile",
            ])
        );
        for name in &names {
            let text = repo::read(&format!("ci/images/{name}"));
            let lines: Vec<&str> = py_splitlines(&text)
                .into_iter()
                .filter(|l| !py_strip(l).is_empty())
                .collect();
            assert_eq!(lines.len(), 1, "{name}: expected a single FROM line");
            assert!(
                from.is_match(lines[0]),
                "{name}: FROM must be tag@sha256:digest"
            );
        }
    }
}

mod unchanged_copies_test {
    use super::*;

    #[test]
    fn test_copies_exist_with_their_skill_files() {
        for reference in ["frontend-design", "web-design-guidelines"] {
            let path = repo::knowledge_path(reference);
            assert!(repo::path(&path).is_file(), "{path}");
            let (meta, body) = frontmatter::load(&path);
            assert_eq!(meta["name"].as_str(), Some(reference));
            assert!(yaml_truthy(&meta["description"]));
            assert!(!py_strip(&body).is_empty());
        }
    }

    #[test]
    fn test_design_skills_resolve() {
        for name in DESIGN_SKILLS {
            let (meta, _) = frontmatter::load(&repo::knowledge_path(&format!("ds/{name}")));
            assert_eq!(meta["name"].as_str(), Some(name));
        }
    }

    #[test]
    fn test_every_source_has_a_target_and_a_license() {
        let data = read_json("vendor/sources.json");
        let sources = items(get(&data, "sources"));
        assert!(!sources.is_empty());
        for s in sources {
            let target = str_of(get(s, "target"));
            assert!(repo::path(target).is_dir(), "{target}");
            assert!(!files_under(target).is_empty(), "{target}");
        }
        // A subtree (atl) keeps its LICENSE in its own tree, not in vendor/licenses/.
        let slugs: BTreeSet<String> = sources
            .iter()
            .filter(|s| !is_subtree(s))
            .map(source_slug)
            .collect();
        let stems: BTreeSet<String> = repo::list_dir("vendor/licenses")
            .into_iter()
            .filter_map(|n| n.strip_suffix(".txt").map(str::to_string))
            .collect();
        assert_eq!(stems, slugs);
        let license = Regex::new(r"MIT License|Apache License").unwrap();
        for slug in &slugs {
            let text = repo::read(&format!("vendor/licenses/{slug}.txt"));
            assert!(!py_strip(&text).is_empty(), "{slug}");
            assert!(license.is_match(&text), "{slug}");
        }
    }

    #[test]
    fn test_vercel_license_is_the_fixed_mit_text() {
        let text = repo::read("vendor/licenses/vercel-labs-agent-skills.txt");
        let lines = py_splitlines(&text);
        assert!(lines[0].contains("README"));
        assert!(text.contains("Copyright (c) Vercel"));
        assert!(text.contains("MIT License"));
    }
}

mod anthology_superpowers_test {
    use super::*;

    #[test]
    fn test_manifest_name() {
        let manifest = read_json(&format!("{}/.claude-plugin/plugin.json", repo::SUPERPOWERS));
        assert_eq!(str_of(get(&manifest, "name")), "anthology-superpowers");
    }

    /// `UNPREFIXED_REF`, `(?<![\w./-])superpowers:(?=[a-z0-9-])` on bytes: a reference is
    /// `superpowers:<skill>` not preceded by another name character (bytes `\w` is ASCII).
    fn has_unprefixed_ref(data: &[u8]) -> bool {
        const NEEDLE: &[u8] = b"superpowers:";
        let name_byte = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'/' | b'-');
        data.windows(NEEDLE.len()).enumerate().any(|(i, w)| {
            w == NEEDLE
                && (i == 0 || !name_byte(data[i - 1]))
                && data
                    .get(i + NEEDLE.len())
                    .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
        })
    }

    #[test]
    fn test_no_unprefixed_skill_references() {
        let root = repo::path(repo::SUPERPOWERS);
        let offenders: Vec<String> = files_under(repo::SUPERPOWERS)
            .into_iter()
            .filter(|p| has_unprefixed_ref(&std::fs::read(p).unwrap()))
            .map(|p| p.strip_prefix(&root).unwrap().display().to_string())
            .collect();
        assert_eq!(offenders, Vec::<String>::new());
    }

    #[test]
    fn test_prefixed_references_exist() {
        let hook = repo::read(&format!("{}/hooks/session-start", repo::SUPERPOWERS));
        assert!(hook.contains("anthology-superpowers:using-superpowers"));
    }

    #[test]
    fn test_session_hook_is_executable_and_wired() {
        use std::os::unix::fs::PermissionsExt;
        let hook = repo::path(&format!("{}/hooks/session-start", repo::SUPERPOWERS));
        // `os.access(.., X_OK)`: any execute bit (checkouts set it for the owner and others).
        let mode = std::fs::metadata(&hook).unwrap().permissions().mode();
        assert!(mode & 0o111 != 0, "{} is not executable", hook.display());
        let hooks = read_json(&format!("{}/hooks/hooks.json", repo::SUPERPOWERS));
        assert!(
            get(&hooks, "hooks")
                .as_object()
                .is_some_and(|m| m.contains_key("SessionStart"))
        );
    }

    #[test]
    fn test_preloads_resolve() {
        let preloads: BTreeSet<&str> = repo::expected_roster()
            .into_iter()
            .flat_map(|a| a.preloads)
            .collect();
        for name in preloads {
            let (meta, _) = frontmatter::load(&repo::superpowers_skill_path(name));
            assert_eq!(meta["name"].as_str(), Some(name));
            assert!(yaml_truthy(&meta["description"]));
        }
    }
}
