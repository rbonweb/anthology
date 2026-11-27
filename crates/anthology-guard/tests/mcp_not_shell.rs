//! A script a tool runs is code, not a shell command: the G3 scan of the code keys of a tool input
//! (`code`, `script`, `expression`, `func`, `fn`) reads it for a spelled profile path, but a brace
//! group in it is not a shell brace expansion that the guard must expand to the end. A compact
//! object literal next to a `/` and a word such as `config` used to give `(G0) too complex` for
//! the BrowserOS `run` tool, whose `code` is JavaScript. Every check that is meant for MCP tools
//! stays: G5 (the browser servers the routing guide bans), G3 (a profile path in code or under a
//! path key) and G6 (the file tools). Bash keeps its whole analysis.

#[path = "support/gd.rs"]
mod gd;

use anthology_guard::{HookResult, Rule, Verdict, check, decide};
use anthology_pycompat::json::{self, Map, PY_DEFAULT, Value};

use gd::{HOME, obj};

const RUN: &str = "mcp__browseros-neo__run";

/// Thirteen small objects in one compact literal: its braces expand to 2^13 words.
fn objects() -> String {
    let fields: Vec<String> = (0..13).map(|i| format!("k{i}:{{x:1,y:2}}")).collect();
    format!("{{{}}}", fields.join(","))
}

/// A script with template literals, arrow functions and `window.__x = {...}`, of the kind the
/// `run` tool is given. Its last line has a `/` and the word `config` in one unspaced word.
fn script() -> String {
    format!(
        "const t = `n=${{1}}`;\nconst go = async (a) => a + 1;\nwindow.__x={};//config",
        objects()
    )
}

fn run(code: &str) -> (Value, Value) {
    (
        Value::Str(RUN.into()),
        obj(&[
            ("session", Value::Str("s1".into())),
            ("code", Value::Str(code.into())),
        ]),
    )
}

fn verdict(call: &(Value, Value)) -> Verdict {
    check(&call.0, &call.1, HOME)
}

fn raw(call: &(Value, Value)) -> Vec<u8> {
    let mut m = Map::new();
    m.insert("tool_name".into(), call.0.clone());
    m.insert("tool_input".into(), call.1.clone());
    json::dumps(&Value::Object(m), &PY_DEFAULT).into_bytes()
}

#[test]
fn javascript_in_the_run_tool_is_not_too_complex() {
    let call = run(&script());
    assert_eq!(verdict(&call), Verdict::Allow);
    // the whole hook: exit 0 and nothing on stderr (no G0 line)
    assert_eq!(
        decide(&raw(&call), HOME, false),
        HookResult {
            code: 0,
            stderr: String::new()
        }
    );
    // the same text under the other code keys
    for key in ["script", "expression", "func", "fn"] {
        let input = obj(&[(key, Value::Str(script()))]);
        assert_eq!(
            check(&Value::Str("mcp__x__y".into()), &input, HOME),
            Verdict::Allow,
            "{key}"
        );
    }
}

#[test]
fn ordinary_scripts_stay_allowed() {
    for code in [
        "const p = await browser.open('https://example.com'); return await p.read();",
        "const t = `n=${1}`; window.__x = {a: 1, b: 2}; return (() => t)();",
        "return [1, 2, 3].map(i => ({ a: i, b: i }));",
    ] {
        assert_eq!(verdict(&run(code)), Verdict::Allow, "{code}");
    }
}

#[test]
fn a_profile_path_in_code_still_blocks() {
    let profile = "~/.config/chromium/Default/Cookies";
    for code in [
        format!("await browser.navigate('file://{profile}')"),
        // beside the big literal, in the same unspaced word
        format!("window.__x={};//{profile}", objects()),
        format!("{}/.config/chromium/x", objects()),
    ] {
        assert_eq!(
            verdict(&run(&code)),
            Verdict::Block(Rule::G3),
            "{}",
            &code[code.len().saturating_sub(60)..]
        );
    }
    let func = obj(&[
        ("page", Value::Int(json::Int::Small(1))),
        (
            "func",
            Value::Str(
                "() => fetch('file:///home/tester/.config/chromium/Default/Bookmarks')".into(),
            ),
        ),
    ]);
    assert_eq!(
        check(
            &Value::Str("mcp__browseros-neo__evaluate_in_page".into()),
            &func,
            HOME
        ),
        Verdict::Block(Rule::G3)
    );
}

#[test]
fn the_banned_browser_servers_still_block() {
    let input = obj(&[("code", Value::Str(script()))]);
    for tool in [
        "mcp__playwright__browser_evaluate",
        "mcp__puppeteer__x",
        "mcp__chrome-devtools__evaluate_script",
        "mcp__claude-in-chrome__x",
    ] {
        assert_eq!(
            check(&Value::Str(tool.into()), &input, HOME),
            Verdict::Block(Rule::G5),
            "{tool}"
        );
    }
}

#[test]
fn path_keys_and_file_tools_keep_their_rules() {
    // a path key is a path, so a brace group in it is expanded in full, and a profile is found
    let path = obj(&[("path", Value::Str("~/.config/{chromium,x}/Default".into()))]);
    assert_eq!(
        check(&Value::Str("mcp__x__y".into()), &path, HOME),
        Verdict::Block(Rule::G3)
    );
    // ... and a path too large to expand is still too complex
    let big = format!("{}/config", "{a,b}".repeat(13));
    let path = obj(&[("path", Value::Str(big))]);
    assert_eq!(
        check(&Value::Str("mcp__x__y".into()), &path, HOME),
        Verdict::TooComplex
    );
    let write = obj(&[
        ("file_path", Value::Str("~/.claude/memory/notes.md".into())),
        ("content", Value::Str(gd::gh_token())),
    ]);
    assert_eq!(
        check(&Value::Str("Write".into()), &write, HOME),
        Verdict::Block(Rule::G6)
    );
}

#[test]
fn bash_keeps_its_full_analysis() {
    let (tool, input) = gd::bash(&format!("cat {}/config", "{a,b}".repeat(13)));
    assert_eq!(check(&tool, &input, HOME), Verdict::TooComplex);
    let (tool, input) = gd::bash("cat ~/.config/chromium/Default/Cookies");
    assert_eq!(check(&tool, &input, HOME), Verdict::Block(Rule::G3));
    let (tool, input) = gd::bash("npx playwright install");
    assert_eq!(check(&tool, &input, HOME), Verdict::Block(Rule::G1));
    // the code a command runs is read in full too: the same literal after `python3 -c`
    let (tool, input) = gd::bash(&format!("python3 -c 'x={}//config'", objects()));
    assert_eq!(check(&tool, &input, HOME), Verdict::TooComplex);
}

/// The code an interpreter reads from a heredoc is `Entry.code`: it keeps the brace cap, so the
/// same literal is still too complex there (flipping `cap_blocks` for it makes these fail).
#[test]
fn a_python_heredoc_keeps_the_brace_cap() {
    let (tool, input) = gd::bash(&format!("python3 - <<EOF\nx={}//config\nEOF", objects()));
    assert_eq!(check(&tool, &input, HOME), Verdict::TooComplex);
}

#[test]
fn a_node_heredoc_keeps_the_brace_cap() {
    let (tool, input) = gd::bash(&format!("node <<\"EOF\"\nx={}//config\nEOF", objects()));
    assert_eq!(check(&tool, &input, HOME), Verdict::TooComplex);
}

/// A word past the brace cap in a code key is not dropped: each brace group is read as a `*`, so
/// a profile path built from braces still blocks (G3), as a `!` shell line in the code of an IDE
/// tool is read.
#[test]
fn a_profile_path_made_of_brace_groups_in_a_code_key_blocks() {
    let groups = "{,}".repeat(13);
    let path = format!("~/.config/chrom{groups}ium/Default/Cookies");
    for (tool, key, text) in [
        ("mcp__x__y", "script", format!("cat {path}")),
        (
            "mcp__x__y",
            "script",
            format!("cat {path}\nx={}", objects()),
        ),
        ("mcp__ide__executeCode", "code", format!("!cat {path}")),
        ("mcp__ide__executeCode", "code", format!("!cat '{path}'")),
    ] {
        let input = obj(&[(key, Value::Str(text.clone()))]);
        assert_eq!(
            check(&Value::Str(tool.into()), &input, HOME),
            Verdict::Block(Rule::G3),
            "{text}"
        );
    }
    // the same text as a Bash command is still too complex, as before
    let (tool, input) = gd::bash(&format!("cat {path}"));
    assert_eq!(check(&tool, &input, HOME), Verdict::TooComplex);
    // a big literal that names no profile stays allowed
    for code in [
        format!("window.__x={};//config", objects()),
        format!("const a = [{}, {}]; // see /config", objects(), objects()),
    ] {
        assert_eq!(verdict(&run(&code)), Verdict::Allow, "{code}");
    }
    // the cost of the `*` reading: a big literal right before a protected folder name is read as
    // `*/chromium`, where the `*` could be `.config`, so it blocks
    assert_eq!(
        verdict(&run(&format!("x={}/chromium", objects()))),
        Verdict::Block(Rule::G3)
    );
}
