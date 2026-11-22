//! `tests/test_taste_browseros.py`: design-taste-frontend on BrowserOS Neo (the committed tree and
//! the port map only: no network, no git, no browser).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anthology_pycompat::json::{self, Value};
use regex::Regex;
use regex::bytes::RegexBuilder;
use repo_tests::{frontmatter, repo};

const FOLDER: &str = "plugins/anthology/knowledge/design-taste-frontend";
const PORTMAP: &str = "vendor/patches/design-taste-frontend.portmap.json";
const BRIEF_REGEX: &str = concat!(
    r"remote-debugging|user-data-dir|CHROME_PATH|Google Chrome|chrome\.exe",
    r"|/usr/bin/(google-chrome|chromium)|playwright|puppeteer"
);
const ACTIONS: [&str; 3] = ["keep", "port", "drop"];

fn read_json(rel: &str) -> Value {
    let text = repo::read(rel);
    json::loads(&text).unwrap_or_else(|e| panic!("{rel}: {}", e.py_str(&text)))
}

fn portmap() -> Value {
    read_json(PORTMAP)
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

fn entries(value: &Value) -> Vec<(&str, &Value)> {
    value
        .as_object()
        .unwrap_or_else(|| panic!("not an object: {value:?}"))
        .iter()
        .collect()
}

fn folder_path(rel: &str) -> PathBuf {
    repo::path(&format!("{FOLDER}/{rel}"))
}

fn read_in_folder(rel: &str) -> String {
    repo::read(&format!("{FOLDER}/{rel}"))
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

/// `PurePath.suffix`: from the last dot, unless the dot is the first or last character.
fn py_suffix(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => &name[i..],
        _ => "",
    }
}

/// `shipped_files()`: `sorted(FOLDER.rglob("*"))` files; `rglob` does not follow symlinked
/// directories.
fn shipped_files() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().map(|e| e.unwrap()) {
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                walk(&path, out);
            } else if path.is_file() {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(&folder_path(""), &mut out);
    out.sort();
    out
}

fn rel_to_folder(path: &Path) -> String {
    path.strip_prefix(folder_path(""))
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

/// `re.compile(BRIEF_REGEX.encode(), re.IGNORECASE)`: a bytes pattern, so case folding is ASCII.
fn brief_pattern() -> regex::bytes::Regex {
    RegexBuilder::new(BRIEF_REGEX)
        .case_insensitive(true)
        .unicode(false)
        .build()
        .unwrap()
}

/// `vendor.portmap_scripts(tree, rule)`: the files of a tree the port map's `script_rule` counts as
/// scripts: a listed extension (any case), a `#!` first line, or a `.js` file that reads like Node
/// (not browser code). `os.walk` order does not matter: the result is a set.
fn portmap_scripts(tree: &Path, rule: &Value) -> BTreeSet<String> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().map(|e| e.unwrap()) {
            let path = entry.path();
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                if entry.file_name() != ".git" {
                    walk(&path, out);
                }
            } else if kind.is_symlink() && path.is_dir() {
                // os.walk lists a symlink to a directory with the directories and never enters it.
            } else {
                out.push(path);
            }
        }
    }
    let extensions: BTreeSet<String> = get(rule, "extensions")
        .as_array()
        .expect("extensions is a list")
        .iter()
        .map(|e| str_of(e).to_lowercase())
        .collect();
    let shebang = rule
        .as_object()
        .and_then(|m| m.get("shebang"))
        .is_some_and(|v| v.as_bool().unwrap_or(false));
    let node_js = Regex::new(str_of(get(rule, "js_node_regex"))).expect("js_node_regex");
    let mut files = Vec::new();
    walk(tree, &mut files);
    let mut found = BTreeSet::new();
    for path in files {
        let rel = path
            .strip_prefix(tree)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let name = path.file_name().unwrap().to_string_lossy();
        let ext = py_suffix(&name).to_lowercase();
        if extensions.contains(&ext) {
            found.insert(rel);
            continue;
        }
        // An unreadable file is skipped (`OSError`); only `.js` files are read whole.
        let Ok(raw) = std::fs::read(&path) else {
            continue;
        };
        let has_shebang = shebang && raw.starts_with(b"#!");
        let reads_like_node = ext == ".js" && node_js.is_match(&String::from_utf8_lossy(&raw));
        if has_shebang || reads_like_node {
            found.insert(rel);
        }
    }
    found
}

mod no_chrome_launch_test {
    use super::*;

    #[test]
    fn test_the_folder_is_there() {
        assert!(folder_path("SKILL.md").is_file());
    }

    #[test]
    fn test_the_port_map_carries_the_briefs_regex() {
        assert_eq!(str_of(get(&portmap(), "chrome_regex")), BRIEF_REGEX);
    }

    #[test]
    fn test_no_shipped_file_or_path_matches_the_chrome_pattern() {
        let pattern = brief_pattern();
        let files = shipped_files();
        assert!(files.len() > 1000, "{} shipped files", files.len());
        let mut failures = Vec::new();
        for path in &files {
            let rel = rel_to_folder(path);
            if pattern.is_match(rel.as_bytes()) {
                failures.push(format!("{rel}: the path matches"));
            }
            if pattern.is_match(&std::fs::read(path).unwrap()) {
                failures.push(format!("{rel}: the content matches"));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn test_the_pattern_catches_what_it_should() {
        let pattern = brief_pattern();
        for yes in [
            "--remote-debugging-port=0",
            "--USER-DATA-DIR=/tmp/x",
            "CHROME_PATH",
            "google chrome",
            "Chrome.exe",
            "/usr/bin/chromium",
            "Playwright",
            "require('puppeteer')",
        ] {
            assert!(pattern.is_match(yes.as_bytes()), "{yes}");
        }
        for no in [
            "HeadlessChrome/",
            "chrome://",
            "page.cdp('Page.captureScreenshot')",
        ] {
            assert!(!pattern.is_match(no.as_bytes()), "{no}");
        }
    }

    #[test]
    fn test_the_two_automation_brand_icons_are_gone() {
        for name in ["playwright", "puppeteer"] {
            assert!(!folder_path(&format!("assets/icons/demo/{name}.svg")).exists());
        }
    }
}

mod skill_file_test {
    use super::*;

    #[test]
    fn test_frontmatter_has_no_hooks() {
        let (meta, _) = frontmatter::load(&format!("{FOLDER}/SKILL.md"));
        assert_eq!(meta["name"].as_str(), Some("design-taste-frontend"));
        assert!(meta["hooks"].is_badvalue(), "frontmatter has a hooks key");
        let text = read_in_folder("SKILL.md");
        let head = text.split("\n---").next().unwrap();
        assert!(!Regex::new(r"(?m)^\s*hooks\s*:").unwrap().is_match(head));
        assert!(!head.contains("--hook"));
    }

    #[test]
    fn test_skill_mentions_browseros_neo() {
        let text = read_in_folder("SKILL.md");
        assert!(text.contains("BrowserOS Neo"));
        assert!(text.contains("mcp__browseros-neo__"));
    }

    #[test]
    fn test_nothing_tells_the_agent_to_run_the_removed_tools() {
        let stale = Regex::new(
            r"scripts/shoot\.mjs|shoot\.mjs --|gate\.mjs --hook|--profile\b|moments\.mjs shoot|backgrounds/check\.mjs",
        )
        .unwrap();
        let mut failures = Vec::new();
        for path in shipped_files() {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let small = std::fs::metadata(&path).unwrap().len() < 600_000;
            if matches!(py_suffix(&name), ".md" | ".mjs") && small {
                let text = String::from_utf8_lossy(&std::fs::read(&path).unwrap()).into_owned();
                if let Some(m) = stale.find(&text) {
                    failures.push(format!("{}: {:?}", rel_to_folder(&path), m.as_str()));
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn test_the_gate_reference_documents_the_bridge_and_the_eyes_step() {
        let text = read_in_folder("references/gate.md");
        assert!(text.contains("## The bridge"));
        assert!(text.contains("## The eyes"));
        assert!(text.contains("## Before you finish"));
        assert!(!text.contains("## As a hook"));
    }
}

mod port_map_test {
    use super::*;

    #[test]
    fn test_the_map_shape() {
        let data = portmap();
        assert_eq!(
            str_of(get(get(&data, "upstream"), "commit")),
            "f6b76575f949707b4e3c37cd86cb44b0b98643e8"
        );
        let scripts = entries(get(&data, "scripts"));
        assert_eq!(scripts.len(), 20);
        for (rel, spec) in &scripts {
            let action = str_of(get(spec, "action"));
            assert!(ACTIONS.contains(&action), "{rel}");
            assert!(get(spec, "chrome").as_bool().is_some(), "{rel}");
            assert!(!py_strip(str_of(get(spec, "why"))).is_empty(), "{rel}");
            if action == "port" {
                assert!(
                    ["bridge", "trim"].contains(&str_of(get(spec, "how"))),
                    "{rel}"
                );
                assert!(
                    !py_strip(str_of(get(spec, "browseros"))).is_empty(),
                    "{rel}"
                );
            }
        }
        let counts: BTreeMap<&str, usize> = ACTIONS
            .iter()
            .map(|a| {
                let n = scripts
                    .iter()
                    .filter(|(_, s)| str_of(get(s, "action")) == *a)
                    .count();
                (*a, n)
            })
            .collect();
        assert_eq!(
            counts,
            BTreeMap::from([("keep", 9), ("port", 9), ("drop", 2)])
        );
        let mut added: Vec<&str> = entries(get(&data, "added"))
            .iter()
            .map(|(k, _)| *k)
            .collect();
        added.sort_unstable();
        assert_eq!(added, ["scripts/neo.mjs"]);
        assert!(!scripts.iter().any(|(rel, _)| added.contains(rel)));
    }

    #[test]
    fn test_the_shipped_scripts_are_exactly_keep_port_and_added() {
        let data = portmap();
        let mut wanted: BTreeSet<String> = entries(get(&data, "scripts"))
            .into_iter()
            .filter(|(_, s)| ["keep", "port"].contains(&str_of(get(s, "action"))))
            .map(|(rel, _)| rel.to_string())
            .collect();
        wanted.extend(
            entries(get(&data, "added"))
                .iter()
                .map(|(k, _)| k.to_string()),
        );
        assert_eq!(
            portmap_scripts(&folder_path(""), get(&data, "script_rule")),
            wanted
        );
    }

    #[test]
    fn test_no_drop_script_ships() {
        let data = portmap();
        for (rel, spec) in entries(get(&data, "scripts")) {
            if str_of(get(spec, "action")) == "drop" {
                assert!(!folder_path(rel).exists(), "{rel}");
            } else {
                assert!(folder_path(rel).is_file(), "{rel}");
            }
        }
        for (rel, _) in entries(get(&data, "added")) {
            assert!(folder_path(rel).is_file(), "{rel}");
        }
    }

    #[test]
    fn test_every_pristine_script_has_a_valid_action() {
        let data = portmap();
        let scripts = get(&data, "scripts");
        let actions: BTreeSet<&str> = entries(scripts)
            .iter()
            .map(|(_, s)| str_of(get(s, "action")))
            .collect();
        let unknown: Vec<&&str> = actions.iter().filter(|a| !ACTIONS.contains(a)).collect();
        assert!(unknown.is_empty(), "{unknown:?}");
        for rel in ["scripts/shoot.mjs", "assets/backgrounds/check.mjs"] {
            assert_eq!(str_of(get(get(scripts, rel), "action")), "drop");
        }
    }

    #[test]
    fn test_no_script_is_executable() {
        use std::os::unix::fs::PermissionsExt;
        // agents run `node scripts/neo.mjs`; a patch cannot add an executable file
        for (rel, _) in entries(get(&portmap(), "added")) {
            let mode = std::fs::metadata(folder_path(rel))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0, "{rel}");
        }
    }

    #[test]
    fn test_the_sources_entry_points_at_the_map() {
        let sources = read_json("vendor/sources.json");
        let entry = get(&sources, "sources")
            .as_array()
            .expect("sources is a list")
            .iter()
            .find(|e| get(e, "name").as_str() == Some("design-taste-frontend"))
            .expect("StopIteration: no design-taste-frontend source");
        assert_eq!(str_of(get(entry, "portmap")), PORTMAP);
        assert_eq!(
            str_of(get(entry, "commit")),
            "f6b76575f949707b4e3c37cd86cb44b0b98643e8"
        );
        assert_eq!(
            [
                str_of(get(entry, "repo")),
                str_of(get(entry, "path")),
                str_of(get(entry, "ref")),
                str_of(get(entry, "license")),
            ],
            [
                "https://github.com/leonxlnx/taste-skill",
                "skills/taste-skill",
                "v2.5.1",
                "MIT"
            ]
        );
        let want =
            json::loads(r#"[{"patch": "vendor/patches/design-taste-frontend.patch"}]"#).unwrap();
        assert_eq!(get(entry, "transform"), &want);
        assert!(repo::path("vendor/licenses/leonxlnx-taste-skill.txt").is_file());
    }

    #[test]
    fn test_lose_swallows_only_unavailable_methods_never_a_bridge_failure() {
        let text = read_in_folder("scripts/consistency.mjs");
        let marker = "function lose(errors, name, e, msg) {\n";
        let after = text
            .split_once(marker)
            .expect("IndexError: no `function lose`")
            .1;
        let body = after.split('\n').next().unwrap();
        assert_eq!(py_strip(body), "if (e && e.unavailable) return;");
        assert!(!body.contains("e.neo"));
        assert!(read_in_folder("scripts/neo.mjs").contains("e.unavailable = true"));
    }

    #[test]
    fn test_checks_that_never_run_on_neo_print_skip_not_pass() {
        let text = read_in_folder("scripts/consistency.mjs");
        let skip = Regex::new(r"const NEO_SKIP = new Set\(\[(.*?)\]\)")
            .unwrap()
            .captures(&text)
            .expect("no NEO_SKIP set")[1]
            .to_string();
        let ids: BTreeSet<String> = Regex::new(r"'([a-z-]+)'")
            .unwrap()
            .captures_iter(&skip)
            .map(|m| m[1].to_string())
            .collect();
        for check in [
            "contrast",
            "empty-visual",
            "skip-link",
            "fade-cut",
            "dark-ground",
            "photo-set",
            "busy-ground",
            "button-contrast",
            "hover",
            "hover-move",
            "hover-spill",
            "hidden-visual",
            "loop-pause",
            "forms",
            "focus-visible",
            "focus-hidden",
            "reduced-motion",
            "gl-fallback",
        ] {
            assert!(ids.contains(check), "{check}");
        }
        // overlap-text of a mock is DOM-based: it runs outside the pixel (fade) block
        let head = text
            .split_once("const covered = new Map();")
            .expect("IndexError: no `const covered`")
            .0;
        let before = py_splitlines(head.trim_end_matches(is_py_space));
        let opener = before
            .iter()
            .rev()
            .find(|l| l.starts_with("      if ("))
            .expect("IndexError: no opener line");
        assert_eq!(py_strip(opener), "if (m.fades && m.fades.length) {");
    }

    #[test]
    fn test_the_not_checked_paths_are_explicit_in_the_ported_checkers() {
        let text = read_in_folder("scripts/consistency.mjs");
        assert!(text.contains("BrowserOS Neo: no screenshots"));
        assert!(text.contains("notChecked"));
        let layout = read_in_folder("assets/layout/check.mjs");
        assert!(layout.contains("--shots is not available on BrowserOS Neo"));
    }
}
