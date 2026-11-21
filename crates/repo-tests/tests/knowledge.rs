//! `tests/test_knowledge.py`: the knowledge folders under `plugins/anthology/knowledge/`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anthology_pycompat::json::{self, Value};
use regex::Regex;
use repo_tests::yaml::Yaml;
use repo_tests::{frontmatter, repo};
use sha2::{Digest, Sha256};

const FORBIDDEN_IN_STRIPPED: [&str; 7] = [
    "~~",
    "CONNECTORS.md",
    "uxkin.com/mcp",
    "find_ui_materials",
    "find_ui_references",
    "ui-design-mcp",
    "SUPERCHARGED",
];

/// Python's `\s` (Unicode str patterns) is `str.isspace`: White_Space plus U+001C..U+001F.
const S: &str = r"[\s\x1c-\x1f]";

/// `CONNECTED_LEAD_IN` of the Python test, in three pieces plus the code in [`connected_lead_in`]:
///
/// ```text
/// ^\s*(?:[-*+]\s+)?(?:\*\*)?If\s+(?!no\b).*\btools?\s+(?:is|are|weren't|were)\s+connected\b   (MULTILINE)
/// ```
///
/// `regex` has no look-ahead, so [`connected_lead_in`] finds every `If` the prefix can reach from a
/// line start, then tries each way the `\s+` after it can give back whitespace (Python backtracks
/// into it, so `If  no tool is connected` with two spaces matches) and checks `(?!no\b)` there.
struct LeadIn {
    prefix: Regex,
    space: Regex,
    no_word: Regex,
    rest: Regex,
}

static LEAD_IN: LazyLock<LeadIn> = LazyLock::new(|| LeadIn {
    prefix: Regex::new(&format!(r"\A{S}*(?:[-*+]{S}+)?(?:\*\*)?If")).unwrap(),
    space: Regex::new(&format!(r"\A{S}")).unwrap(),
    no_word: Regex::new(r"\Ano\b").unwrap(),
    rest: Regex::new(&format!(
        r"\A.*\btools?{S}+(?:is|are|weren't|were){S}+connected\b"
    ))
    .unwrap(),
});

/// `CONNECTED_LEAD_IN.search(text) is not None`: "If the UXKIN tools are connected ..." lead-ins;
/// "If no tool is connected" and "If tools are not connected" are fallbacks and stay.
fn connected_lead_in(text: &str) -> bool {
    let lead = &*LEAD_IN;
    let mut starts = vec![0];
    starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
    let mut tried = BTreeSet::new();
    for start in starts {
        let Some(m) = lead.prefix.find(&text[start..]) else {
            continue;
        };
        let after_if = start + m.end();
        if !tried.insert(after_if) {
            continue;
        }
        // `\s+` takes j >= 1 whitespace chars; the look-ahead and the rest must hold after them.
        let mut at = after_if;
        while let Some(ws) = lead.space.find(&text[at..]) {
            at += ws.end();
            let tail = &text[at..];
            if !lead.no_word.is_match(tail) && lead.rest.is_match(tail) {
                return true;
            }
        }
    }
    false
}

/// The chars of `bytes` that are valid UTF-8 (`read_text(errors="ignore")`).
fn decode_ignore(bytes: &[u8]) -> String {
    bytes.utf8_chunks().map(|c| c.valid()).collect()
}

fn read_json(rel: &str) -> Value {
    let text = repo::read(rel);
    json::loads(&text).unwrap_or_else(|e| panic!("{rel}: {}", e.py_str(&text)))
}

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

/// `source(name)`: the `vendor/sources.json` entry called `name`.
fn source(name: &str) -> Value {
    let sources = read_json("vendor/sources.json");
    let entries = get(&sources, "sources")
        .as_array()
        .expect("sources is a list");
    entries
        .iter()
        .find(|e| str_of(get(e, "name")) == name)
        .unwrap_or_else(|| panic!("StopIteration: no source {name}"))
        .clone()
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

/// Python truthiness of a YAML value (`assertTrue(str(meta.get("description", "")).strip())`).
fn description_text(meta: &Yaml) -> String {
    match &meta["description"] {
        Yaml::BadValue => String::new(),
        Yaml::String(s) => s.clone(),
        Yaml::Null => "None".to_string(),
        other => format!("{other:?}"),
    }
}

/// Every entry below `dir` (files, directories, symlinks), like `Path.rglob("*")`, which does not
/// descend into symlinked directories. Sorted.
fn rglob_entries(dir: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.map(|e| e.unwrap()) {
            let path = entry.path();
            let is_dir = entry.file_type().unwrap().is_dir();
            out.push(path.clone());
            if is_dir {
                walk(&path, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, &mut out);
    out.sort();
    out
}

/// `sorted(p for p in rglob("*") if p.is_file())`.
fn rglob_files(dir: &Path) -> Vec<PathBuf> {
    rglob_entries(dir)
        .into_iter()
        .filter(|p| p.is_file())
        .collect()
}

fn knowledge_dir() -> PathBuf {
    repo::path(repo::KNOWLEDGE)
}

/// `present()`: the knowledge folders on disk.
fn present() -> BTreeSet<String> {
    repo::list_dir(repo::KNOWLEDGE)
        .into_iter()
        .filter(|n| knowledge_dir().join(n).is_dir())
        .collect()
}

/// `skill_files(folder)`: the SKILL.md files of a folder; design-skills is a repo of skills.
fn skill_files(folder: &str) -> Vec<PathBuf> {
    if folder == "design-skills" {
        let skills = format!("{}/{folder}/skills", repo::KNOWLEDGE);
        let mut found: Vec<PathBuf> = repo::list_dir(&skills)
            .into_iter()
            .map(|n| repo::path(&format!("{skills}/{n}/SKILL.md")))
            .filter(|p| p.exists())
            .collect();
        found.sort();
        return found;
    }
    vec![repo::path(&format!(
        "{}/{folder}/SKILL.md",
        repo::KNOWLEDGE
    ))]
}

/// `files_of(folder)`: every file below a knowledge folder.
fn files_of(folder: &str) -> Vec<PathBuf> {
    rglob_files(&knowledge_dir().join(folder))
}

/// A path relative to the knowledge folder, `/`-separated.
fn rel_knowledge(path: &Path) -> String {
    path.strip_prefix(knowledge_dir())
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A repo-relative path as the argument of the `frontmatter`/`repo` helpers.
fn rel_repo(path: &Path) -> String {
    path.strip_prefix(repo::root())
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

mod knowledge_folders_test {
    use super::*;

    #[test]
    fn test_expected_set_is_the_22_plus_the_other_six() {
        assert_eq!(repo::stripped_knowledge().len(), 22);
        assert_eq!(repo::expected_knowledge().len(), 29);
    }

    #[test]
    fn test_no_unexpected_folders() {
        let expected = repo::expected_knowledge();
        let extra: Vec<String> = present().difference(&expected).cloned().collect();
        assert_eq!(extra, Vec::<String>::new());
    }

    #[test]
    fn test_every_landed_folder_is_there() {
        let mut landed = repo::expected_knowledge();
        for later in ["ui-taste", "caveman", "design-taste-frontend"] {
            landed.remove(later);
        }
        let missing: Vec<String> = landed.difference(&present()).cloned().collect();
        assert_eq!(missing, Vec::<String>::new());
    }

    #[test]
    fn test_ui_taste_lands_in_task_4() {
        assert!(present().contains("ui-taste"));
    }

    #[test]
    fn test_caveman_lands_in_task_4() {
        assert!(present().contains("caveman"));
    }

    #[test]
    fn test_design_taste_frontend_lands_in_task_5() {
        assert!(present().contains("design-taste-frontend"));
    }

    #[test]
    fn test_every_skill_has_its_folder_name_and_a_description() {
        let expected = repo::expected_knowledge();
        for folder in present().intersection(&expected) {
            let files = skill_files(folder);
            assert!(!files.is_empty(), "{folder}");
            for path in files {
                assert!(path.is_file(), "{}", path.display());
                let (meta, body) = frontmatter::load(&rel_repo(&path));
                let folder_name = path
                    .parent()
                    .unwrap()
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap();
                assert_eq!(
                    meta["name"].as_str(),
                    Some(folder_name),
                    "{}",
                    path.display()
                );
                assert!(
                    !py_strip(&description_text(&meta)).is_empty(),
                    "{}",
                    path.display()
                );
                assert!(!py_strip(&body).is_empty(), "{}", path.display());
            }
        }
    }

    #[test]
    fn test_the_22_have_no_connector_leftovers() {
        let mut stripped: Vec<String> = repo::stripped_knowledge()
            .into_iter()
            .map(|(name, _, _)| name)
            .collect();
        stripped.sort();
        let mut failures = Vec::new();
        for folder in &stripped {
            let root = knowledge_dir().join(folder);
            if !root.is_dir() {
                failures.push(format!("{folder}: not a folder"));
                continue;
            }
            let entries = rglob_entries(&root);
            if entries
                .iter()
                .any(|p| p.file_name().is_some_and(|n| n == "CONNECTORS.md"))
            {
                failures.push(format!("{folder}: has a CONNECTORS.md"));
            }
            for path in entries.iter().filter(|p| p.is_file()) {
                let text = decode_ignore(&std::fs::read(path).unwrap());
                let rel = rel_knowledge(path);
                for needle in FORBIDDEN_IN_STRIPPED {
                    if text.contains(needle) {
                        failures.push(format!("{rel}: {needle}"));
                    }
                }
                if connected_lead_in(&text) {
                    failures.push(format!("{rel}: connected lead-in"));
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn test_the_connected_lead_in_pattern_only_matches_the_connector_blocks() {
        for yes in [
            "If the UXKIN tools are connected:",
            "- If project management tool is connected, x",
            "**If tool is connected:**",
            "If the UXKIN tools weren't connected and",
        ] {
            assert!(connected_lead_in(yes), "{yes}");
        }
        for no in [
            "If no project management tool is connected:",
            "**If tools are not connected:**",
            "If these tools are not connected, work from what the user provides.",
        ] {
            assert!(!connected_lead_in(no), "{no}");
        }
    }

    #[test]
    fn test_code_review_checklist_never_points_at_the_builtin_command() {
        let text = repo::read(&format!(
            "{}/code-review-checklist/SKILL.md",
            repo::KNOWLEDGE
        ));
        // `/code-review(?!-checklist)`: each `/code-review` not followed by `-checklist`.
        let bare: Vec<&str> = text
            .match_indices("/code-review")
            .filter(|(i, m)| !text[i + m.len()..].starts_with("-checklist"))
            .map(|(_, m)| m)
            .collect();
        assert_eq!(bare, Vec::<&str>::new());
        assert!(text.contains("code-review-checklist"));
    }
}

mod ui_taste_test {
    use super::*;

    const PAID_MCP: [&str; 3] = ["find_ui_references", "find_ui_materials", "get_ui_flow"];

    static CHROME_LAUNCH: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)remote-debugging|user-data-dir|CHROME_PATH|Google Chrome|chrome\.exe|/usr/bin/(google-chrome|chromium)|playwright|puppeteer|child_process",
        )
        .unwrap()
    });

    #[test]
    fn test_the_paid_mcp_is_gone_from_every_file() {
        let files = files_of("ui-taste");
        assert!(!files.is_empty());
        let paid = Regex::new(r"(?i)uizze mcp|paid mcp").unwrap();
        for path in files {
            let text = std::fs::read_to_string(&path).unwrap();
            for needle in PAID_MCP {
                assert!(!text.contains(needle), "{}: {needle}", rel_knowledge(&path));
            }
            assert!(!paid.is_match(&text), "{}", rel_knowledge(&path));
        }
    }

    #[test]
    fn test_the_skill_and_the_policy_file_remain() {
        let (meta, _) = frontmatter::load(&format!("{}/ui-taste/SKILL.md", repo::KNOWLEDGE));
        assert_eq!(meta["name"].as_str(), Some("ui-taste"));
        let folder = knowledge_dir().join("ui-taste");
        assert!(
            folder
                .join("references/uizze-reference-policy.md")
                .is_file()
        );
        assert!(folder.join("reference/research.md").is_file());
    }

    #[test]
    fn test_the_shipped_folder_has_no_stale_checksums_file() {
        assert!(!knowledge_dir().join("ui-taste/CHECKSUMS.sha256").exists());
    }

    #[test]
    fn test_the_snapshot_matches_its_checksums_and_the_pin() {
        let snap = repo::path("vendor/snapshots/ui-taste");
        let sums = std::fs::read(snap.join("CHECKSUMS.sha256")).unwrap();
        assert_eq!(
            sha256_hex(&sums),
            str_of(get(&source("ui-taste"), "checksum"))
        );
        let listing = String::from_utf8(sums).expect("CHECKSUMS.sha256 is not UTF-8");
        let mut listed = std::collections::BTreeMap::new();
        for line in py_splitlines(&listing) {
            // `digest, _, rel = line.partition("  ")`
            let (digest, rel) = line.split_once("  ").unwrap_or((line, ""));
            listed.insert(rel.to_string(), digest.to_string());
        }
        assert!(!listed.is_empty());
        let on_disk: BTreeSet<String> = rglob_files(&snap)
            .iter()
            .map(|p| {
                p.strip_prefix(&snap)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        let mut want: BTreeSet<String> = listed.keys().cloned().collect();
        want.insert("CHECKSUMS.sha256".to_string());
        assert_eq!(on_disk, want);
        for (rel, digest) in &listed {
            let bytes = std::fs::read(snap.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
            assert_eq!(&sha256_hex(&bytes), digest, "{rel}");
        }
    }

    #[test]
    fn test_the_pin_is_the_apache_licensed_uizze_snapshot() {
        let e = source("ui-taste");
        assert_eq!(str_of(get(&e, "license")), "Apache-2.0");
        assert_eq!(str_of(get(&e, "license_file")), "LICENSE");
        assert_eq!(
            str_of(get(&e, "url")),
            "https://uizze.sh/.well-known/skills/ui-taste/"
        );
        assert_eq!(str_of(get(&e, "ref")), "uizze-design-v14");
        assert!(repo::path("vendor/licenses/ui-taste.txt").is_file());
    }

    #[test]
    fn test_the_evidence_script_launches_no_browser() {
        let text = repo::read(&format!(
            "{}/ui-taste/scripts/verify-visual-evidence.mjs",
            repo::KNOWLEDGE
        ));
        assert!(!CHROME_LAUNCH.is_match(&text));
    }

    #[test]
    fn test_the_chrome_pattern_catches_what_it_should() {
        for yes in [
            "--remote-debugging-port=0",
            "--user-data-dir=/tmp/x",
            "process.env.CHROME_PATH",
            "Google Chrome",
            "chrome.exe",
            "/usr/bin/chromium",
            "import playwright",
            "require('puppeteer')",
            "child_process",
        ] {
            assert!(CHROME_LAUNCH.is_match(yes), "{yes}");
        }
    }
}

mod caveman_test {
    use super::*;

    const DESCRIPTION: &str = "the anthology house style for every agent's chat and reports";
    const DROPPED: [&str; 6] = [
        "stop caveman",
        "normal mode",
        "/caveman",
        "ultracave",
        "megacave",
        "wenyan",
    ];

    #[test]
    fn test_only_the_skill_file_and_the_license_texts_are_shipped() {
        let folder = knowledge_dir().join("caveman");
        let shipped: Vec<String> = files_of("caveman")
            .iter()
            .map(|p| {
                p.strip_prefix(&folder)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(shipped, ["LICENSE", "LICENSE-MIT", "NOTICE", "SKILL.md"]);
    }

    #[test]
    fn test_the_modes_aliases_and_triggers_are_gone() {
        for path in files_of("caveman") {
            let text = std::fs::read_to_string(&path).unwrap().to_lowercase();
            for needle in DROPPED {
                assert!(
                    !text.contains(needle),
                    "{}: {needle}",
                    path.file_name().unwrap().to_string_lossy()
                );
            }
        }
    }

    #[test]
    fn test_the_kept_sections_remain() {
        let text = repo::read(&format!("{}/caveman/SKILL.md", repo::KNOWLEDGE));
        for heading in [
            "## Why",
            "## Rules",
            "## When to break the rules",
            "## Pre-send check",
        ] {
            let pattern = Regex::new(&format!(r"(?m)^{}{S}*$", regex::escape(heading))).unwrap();
            assert!(pattern.is_match(&text), "{heading}");
        }
        assert!(!text.contains("## Persistence"));
    }

    #[test]
    fn test_name_and_description() {
        let (meta, _) = frontmatter::load(&format!("{}/caveman/SKILL.md", repo::KNOWLEDGE));
        assert_eq!(meta["name"].as_str(), Some("caveman"));
        assert_eq!(meta["description"].as_str(), Some(DESCRIPTION));
    }

    #[test]
    fn test_the_pin_is_the_apache_licensed_upstream_commit() {
        let e = source("caveman");
        assert_eq!(
            (
                str_of(get(&e, "path")),
                str_of(get(&e, "ref")),
                str_of(get(&e, "license"))
            ),
            ("skills/caveman", "v4.1.0", "Apache-2.0")
        );
        assert_eq!(
            str_of(get(&e, "commit")),
            "1e91f1306d602d4ca23618701b1185575ae1869b"
        );
    }
}
