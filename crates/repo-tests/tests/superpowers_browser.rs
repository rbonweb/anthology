//! `tests/test_superpowers_browser.py`: the vendored superpowers plugin never launches an OS browser.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::LazyLock;

use anthology_pycompat::json::{self, Value};
use regex::bytes::{Regex, RegexBuilder};
use repo_tests::repo;

// Each pattern is a launch of an OS browser (or the opt-in that triggers one), with sample
// strings it must match: the self-test below proves no pattern is vacuous. The pattern texts are
// the Python ones, verbatim (`_CALL + _CMD` is joined by `concat!`); `compile` turns them into Rust
// regexes.
const LAUNCH_SAMPLES: &[(&str, &[&str])] = &[
    (r"xdg-open", &["xdg-open $URL"]),
    (r"wslview", &["wslview $URL"]),
    (r"sensible-browser", &["sensible-browser $URL"]),
    (r"x-www-browser", &["x-www-browser $URL"]),
    (r"gio\s+open", &["gio open $URL"]),
    (r"rundll32", &["rundll32.exe url.dll,FileProtocolHandler x"]),
    (r"FileProtocolHandler", &["url.dll,FileProtocolHandler x"]),
    (
        r"BRAINSTORM_OPEN",
        &["BRAINSTORM_OPEN=1", "BRAINSTORM_OPEN_CMD=x"],
    ),
    (r"open\s+-a\b", &["open -a Safari"]),
    (
        r"cmd(\.exe)?\s+/c\s+start",
        &["cmd /c start x", "cmd.exe /c start x"],
    ),
    (r#"start\s+\"\""#, &[r#"start "" http://x"#]),
    (
        r#"\bopen\s+[\"']?https?://"#,
        &["open https://example.com", "open 'http://x'"],
    ),
    (
        r#"\bopen\s+[\"']?\$"#,
        &[
            r#"open "$URL""#,
            "open '$URL'",
            "open '${URL}'",
            "open $URL",
        ],
    ),
    (
        concat!(
            r#"\b(exec|execFile|spawn)(Sync)?\(\s*['\"`](/usr/bin/|/bin/)?"#,
            r"(open|xdg-open|start|wslview)(?![\w.-])"
        ),
        &[
            "execFile('open', [url])",
            r#"cp.execFile( "xdg-open", args)"#,
            "spawn('start', [url])",
            r#"cp.exec("open " + url)"#,
            "exec(`xdg-open ${url}`)",
            "execSync('wslview x')",
            r#"spawn("/usr/bin/open", [url])"#,
        ],
    ),
    (
        r#"bin:\s*['\"](/usr/bin/)?(open|xdg-open|start)['\"]"#,
        &["{ bin: 'open', args: [url] }", r#"{ bin: "xdg-open" }"#],
    ),
    (
        r"\bwebbrowser\b",
        &[
            "import webbrowser",
            "python -m webbrowser -t x",
            "webbrowser.open(u)",
        ],
    ),
    (r"Start-Process", &["Start-Process $url"]),
    (
        r#"explorer(\.exe)?\s+[\"']?https?://"#,
        &["explorer.exe http://x", r#"explorer "https://x""#],
    ),
    (
        r"open\s+location",
        &[r#"osascript -e 'open location "http://x"'"#],
    ),
    (
        r#"require\(\s*['\"]open['\"]\s*\)"#,
        &["require('open')", r#"const o = require( "open" )"#],
    ),
    (
        r#"import\s+\w+\s+from\s+['\"]open['\"]"#,
        &["import open from 'open'"],
    ),
    (r"/usr/bin/open\b", &["/usr/bin/open -g x"]),
    (
        r#"\bstart\s+[\"']?https?://"#,
        &["start http://x", r#"start "https://x""#],
    ),
    (
        r"start-server\.sh[^\n]*--open",
        &["bash scripts/start-server.sh --project-dir p --open"],
    ),
];

// Things that must never trip a pattern: ordinary prose and non-launch code.
const NEGATIVE_SAMPLES: [&str; 15] = [
    "ws.on('open', () => {})",
    "open the URL in BrowserOS Neo",
    "fs.open(file, 'r', cb)",
    "an open question",
    "keep the tab open",
    "the open source license",
    "WebSocket.OPEN",
    "exec('start-server.sh')",
    "spawn('node', [server])",
    "start the server",
    "start the server at the url",
    "execFile('git', ['open'])",
    "import open_things from './x'",
    "require('openai')",
    "/usr/bin/env node",
];

// (relative path, pattern, reason). Historical prose that no skill tells the agent to
// follow. Only that pattern is waived for that file, so a later sync that adds a different
// launch form there still fails; and an exemption that stops matching fails too.
const EXEMPT: [(&str, &str, &str); 1] = [(
    "docs/plans/2026-01-17-visual-brainstorming.md",
    r#"\bopen\s+[\"']?https?://"#,
    "upstream design plan from Jan 2026; line 510 tells the human to open the local URL, no skill \
     sends the agent here",
)];

/// `re.compile(p.encode(), re.IGNORECASE)`: a bytes pattern, so `\w`, `\s`, `\b` and the case
/// folding are ASCII (`unicode(false)`).
///
/// Python `_CMD` ends in the look-ahead `(?![\w.-])`; here it is `(?:[^\w.-]|\z)`: the next byte is
/// not a name byte, or the data ends. It consumes that byte, which cannot change whether a match
/// exists, and `matches` only asks that. The alternation in `_CMD` has no overlapping branches, so
/// no other parse of the same start could pass where this one fails.
fn compile(pattern: &str) -> Regex {
    let rust = pattern.replace(r"(?![\w.-])", r"(?:[^\w.-]|\z)");
    RegexBuilder::new(&rust)
        .case_insensitive(true)
        .unicode(false)
        .build()
        .unwrap_or_else(|e| panic!("{pattern}: {e}"))
}

/// `COMPILED`: the Python pattern text and its regex, in dict order.
static COMPILED: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    LAUNCH_SAMPLES
        .iter()
        .map(|(pattern, _)| (*pattern, compile(pattern)))
        .collect()
});

fn companion() -> String {
    format!(
        "{}/skills/brainstorming/visual-companion.md",
        repo::SUPERPOWERS
    )
}

/// `shipped_files()`: `sorted(SUPERPOWERS.rglob("*"))` files; `rglob` does not follow symlinked
/// directories.
fn shipped_files() -> Vec<PathBuf> {
    fn walk(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
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
    walk(&repo::path(repo::SUPERPOWERS), &mut out);
    out.sort();
    out
}

/// `matches(path)`: the patterns that match the file's bytes.
fn matches(data: &[u8]) -> Vec<&'static str> {
    COMPILED
        .iter()
        .filter(|(_, rx)| rx.is_match(data))
        .map(|(p, _)| *p)
        .collect()
}

fn rel_to_plugin(path: &std::path::Path) -> String {
    path.strip_prefix(repo::path(repo::SUPERPOWERS))
        .unwrap()
        .to_string_lossy()
        .into_owned()
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

fn truthy_str(value: Option<&Value>) -> Option<&str> {
    value.and_then(Value::as_str).filter(|s| !s.is_empty())
}

mod no_os_browser_launch {
    use super::*;

    #[test]
    fn test_no_shipped_file_launches_an_os_browser() {
        let files = shipped_files();
        assert!(files.len() > 100, "the plugin tree is missing");
        let waived: BTreeSet<(&str, &str)> = EXEMPT.iter().map(|(r, p, _)| (*r, *p)).collect();
        let mut bad = Vec::new();
        for path in &files {
            let rel = rel_to_plugin(path);
            for pattern in matches(&std::fs::read(path).unwrap()) {
                if !waived.contains(&(rel.as_str(), pattern)) {
                    bad.push(format!("{rel}: {pattern}"));
                }
            }
        }
        assert!(
            bad.is_empty(),
            "OS browser launch in the shipped superpowers plugin:\n{}",
            bad.join("\n")
        );
    }

    #[test]
    fn test_every_exemption_still_matches_and_has_a_reason() {
        let mut failures = Vec::new();
        for (rel, pattern, reason) in EXEMPT {
            let row = format!("rel={rel} pattern={pattern}");
            if !COMPILED.iter().any(|(p, _)| *p == pattern) {
                failures.push(format!("{row}: exemption names an unknown pattern"));
            }
            if reason.trim().is_empty() {
                failures.push(format!("{row}: no reason"));
            }
            let path = repo::path(&format!("{}/{rel}", repo::SUPERPOWERS));
            if !path.is_file() {
                failures.push(format!("{row}: {rel} no longer exists: drop the exemption"));
                continue;
            }
            if !matches(&std::fs::read(&path).unwrap()).contains(&pattern) {
                failures.push(format!(
                    "{row}: {rel} no longer matches: drop the exemption"
                ));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn test_every_pattern_matches_its_samples() {
        let mut failures = Vec::new();
        for ((pattern, samples), (_, rx)) in LAUNCH_SAMPLES.iter().zip(COMPILED.iter()) {
            if samples.is_empty() {
                failures.push(format!("{pattern}: needs at least one sample"));
            }
            for text in *samples {
                if !rx.is_match(text.as_bytes()) {
                    failures.push(format!("pattern={pattern} text={text}: no match"));
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn test_the_patterns_ignore_ordinary_text() {
        let mut failures = Vec::new();
        for text in NEGATIVE_SAMPLES {
            let hit = matches(text.as_bytes());
            if !hit.is_empty() {
                failures.push(format!("text={text}: matched {hit:?}"));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}

mod visual_companion_uses_browser_os {
    use super::*;

    #[test]
    fn test_visual_companion_points_at_browseros_neo_and_has_no_open_flag() {
        let text = repo::read(&companion());
        assert!(text.contains("mcp__browseros-neo__"));
        assert!(!text.contains("--open"));
    }

    #[test]
    fn test_start_server_rejects_open_like_any_unknown_option() {
        let text = repo::read(&format!(
            "{}/skills/brainstorming/scripts/start-server.sh",
            repo::SUPERPOWERS
        ));
        assert!(!text.contains("--open"));
        assert!(text.contains("Unknown argument"));
    }

    #[test]
    fn test_server_exports_no_launcher() {
        let text = repo::read(&format!(
            "{}/skills/brainstorming/scripts/server.cjs",
            repo::SUPERPOWERS
        ));
        assert!(!text.contains("browserLauncherForPlatform"));
        assert!(!text.contains("child_process"));
    }

    #[test]
    fn test_readme_has_the_marketplace_install_line() {
        let text = repo::read(&format!("{}/README.md", repo::SUPERPOWERS));
        assert!(text.contains("/plugin install anthology-superpowers@anthology"));
    }
}

mod patch_is_wired {
    use super::*;

    #[test]
    fn test_sources_lists_the_patch_after_rename_plugin() {
        let sources = read_json("vendor/sources.json");
        let entry = get(&sources, "sources")
            .as_array()
            .expect("sources is a list")
            .iter()
            .find(|s| get(s, "name").as_str() == Some("superpowers"))
            .expect("StopIteration: no superpowers source");
        let steps = get(entry, "transform")
            .as_array()
            .expect("transform is a list");
        // `s.get("name") or s.get("patch")`
        let names: Vec<Option<&str>> = steps
            .iter()
            .map(|s| {
                let map = s.as_object().expect("a transform step is an object");
                truthy_str(map.get("name")).or_else(|| map.get("patch").and_then(Value::as_str))
            })
            .collect();
        let patch = "vendor/patches/superpowers.patch";
        let index = |name: &str| names.iter().position(|n| *n == Some(name));
        let rename = index("rename-plugin").expect("rename-plugin is not a step");
        let applied = index(patch).expect("the patch is not a step");
        assert!(rename < applied, "{names:?}");
        assert!(repo::path(patch).is_file());
    }
}
