//! GD1 acceptance beyond the Python table: fail-closed limits, the work budget, verdict
//! monotonicity, and the FW-I payloads where G1 is stricter than the Python on `rust`
//! (owner direction; difftest class D-8).

#[path = "support/gd.rs"]
mod gd;

use anthology_guard::{Rule, Verdict, check, check_with};
use anthology_pycompat::json::Value;
use anthology_shell::Limits;
use proptest::prelude::*;

use gd::{HOME, bash, gh_token, obj};

fn verdict(command: &str) -> Verdict {
    let (tool, input) = bash(command);
    check(&tool, &input, HOME)
}

fn low_limit() -> Limits {
    Limits { backtrack_limit: 0 }
}

#[test]
fn a_backtrack_limit_never_allows() {
    // `$HOME` goes through `_HOME_VAR` (fancy-regex): at limit 0 the path cannot be read.
    let (tool, input) = bash("cat $HOME/notes.txt");
    assert_eq!(check(&tool, &input, HOME), Verdict::Allow);
    assert_eq!(
        check_with(&tool, &input, HOME, &low_limit()),
        Verdict::Incomplete
    );
    // a file tool's path, and G6's look-ahead patterns
    let read = obj(&[("file_path", Value::Str("~/notes.txt".into()))]);
    assert_eq!(
        check_with(&Value::Str("Read".into()), &read, HOME, &low_limit()),
        Verdict::Incomplete
    );
    let write = obj(&[
        ("file_path", Value::Str(".claude/memory/a.md".into())),
        ("content", Value::Str(["pass", "word: hunter2xyz"].concat())),
    ]);
    assert_eq!(
        check_with(&Value::Str("Write".into()), &write, HOME, &low_limit()),
        Verdict::Incomplete
    );
}

#[test]
fn a_rule_that_blocks_wins_over_a_limit() {
    let (tool, input) = bash("npx playwright install; cat $HOME/x");
    assert_eq!(
        check_with(&tool, &input, HOME, &low_limit()),
        Verdict::Block(Rule::G1)
    );
    // every path is read through `_TILDE_USER`/`_HOME_VAR`, so under the limit a profile path
    // is unknown: it blocks, as the limit block
    let (tool, input) = bash(&format!("cat {HOME}/.config/chromium/x"));
    assert_eq!(
        check_with(&tool, &input, HOME, &low_limit()),
        Verdict::Incomplete
    );
    // a memory path read under the limit is unknown, so a secret written there is not allowed
    let write = obj(&[
        ("file_path", Value::Str(".claude/memory/a.md".into())),
        ("content", Value::Str(gh_token())),
    ]);
    assert_ne!(
        check_with(&Value::Str("Write".into()), &write, HOME, &low_limit()),
        Verdict::Allow
    );
    // G5 needs no analysis
    assert_eq!(
        check_with(
            &Value::Str("mcp__playwright__x".into()),
            &obj(&[]),
            HOME,
            &low_limit()
        ),
        Verdict::Block(Rule::G5)
    );
}

#[test]
fn an_analysis_over_the_work_budget_is_too_complex() {
    // anthology-shell's own budget test input: quadratic option rescans stop the analysis.
    let command = format!("docker run img{}", " fish -cX".repeat(4000));
    assert_eq!(verdict(&command), Verdict::TooComplex);
    assert!(anthology_guard::TOO_COMPLEX.contains("too complex to check safely"));
    assert!(anthology_guard::TOO_COMPLEX.contains("/plugin"));
}

#[test]
fn deep_find_exec_nesting_fails_closed() {
    // Python raises RecursionError here and allows (D-1); Rust walks it and stops at its cap.
    let command = format!("{}ls ;", "find . -exec ".repeat(1500));
    assert_ne!(verdict(&command), Verdict::Allow);
}

/// Review finding: long ordinary inputs must not hit a search-wide backtrack limit (the G6
/// patterns go through `pycompat::re`, with a budget per start; `_ESCAPED_SPACE` is a scan).
#[test]
fn long_ordinary_inputs_are_allowed() {
    let prose = "The meeting notes say we keep the cache warm and ship on Friday. ".repeat(16_000);
    assert!(prose.len() > 1_000_000);
    let write = obj(&[
        ("file_path", Value::Str("~/.claude/memory/notes.md".into())),
        ("content", Value::Str(prose.clone())),
    ]);
    assert_eq!(
        check(&Value::Str("Write".into()), &write, HOME),
        Verdict::Allow
    );
    let code = format!("{}cd my\\ dir", "x = compute(a, b) + 1\n".repeat(25_000));
    let run = obj(&[("code", Value::Str(code))]);
    assert_eq!(
        check(&Value::Str("mcp__browseros-neo__run".into()), &run, HOME),
        Verdict::Allow
    );
    // a secret at the end of the long text is still found
    let write = obj(&[
        ("file_path", Value::Str("~/.claude/memory/notes.md".into())),
        ("content", Value::Str(format!("{prose}{}", gh_token()))),
    ]);
    assert_eq!(
        check(&Value::Str("Write".into()), &write, HOME),
        Verdict::Block(Rule::G6)
    );
}

/// FW-I bash-verified installs that the Python on `rust` allows (task-FW-I-report.md, rules 12
/// and 27). Each blocks here as G1.
#[test]
fn fw_i_package_manager_and_installer_options_block() {
    for command in [
        "npm --loglevel silent i puppeteer",
        "npm --loglevel silent init playwright",
        "npm --foo-bar silent i puppeteer",
        "npm --foo-bar silent init playwright",
        "sudo apt-get -o Dpkg::Options::=--force-confnew install chromium",
        "apt-get --option x=y install chromium",
        "dnf --setopt x=y install chromium",
        "yum -c /etc/yum.conf install chromium",
        "snap install --channel beta chromium",
        // the Python reading is kept too (union): Python blocks these, so Rust must
        "npm i --tag puppeteer",
        "npm install --save-dev puppeteer@21.0.0",
        "apt install google-chrome-stable",
    ] {
        assert_eq!(verdict(command), Verdict::Block(Rule::G1), "{command}");
    }
    for command in [
        "npm --loglevel silent install",
        "npm --loglevel silent i playwright",
        "apt-get -o Dpkg::Options::=--force-confnew install jq",
        "npm i -D playwright @playwright/test",
        "dnf --setopt x=y install jq",
    ] {
        assert_eq!(verdict(command), Verdict::Allow, "{command}");
    }
}

/// Commands each rule blocks, for the monotonicity property below.
const BLOCKED: &[&str] = &[
    "npx playwright install",
    "pip install playwright",
    "brew install chromium",
    "google-chrome --headless",
    "open -a Chromium",
    "cat ~/.config/chromium/Default/Cookies",
    "ls ~/Library/Application\\ Support/Google",
    "agent-browser open x",
    "python3 -c \"open('~/.config/google-chrome/x')\"",
];

/// Wrappers and neighbours that do not exempt anything (docker would exempt G1 and G2).
const WRAPS: &[&str] = &[
    "{c}",
    "sudo {c}",
    "timeout 5 {c}",
    "env A=1 {c}",
    "nohup {c}",
    "{c}; ls",
    "ls && {c}",
    "true || {c}",
    "echo $({c})",
    "( {c} )",
    "{ {c}; }",
    "bash -c '{q}'",
    "sh -euo pipefail -c '{q}'",
    "echo '{q}' | sh",
    "cd /tmp && {c}",
    "rtk run {c}",
    "x=$({c})",
    "{c} 2>&1 | tee log",
];

proptest! {
    /// Wrapping a blocked command, or chaining it with others, never turns it into an allow.
    #[test]
    fn verdict_monotonicity(base in 0..BLOCKED.len(), outer in 0..WRAPS.len(), inner in 0..WRAPS.len()) {
        let c = BLOCKED[base];
        let q = c.replace('\'', "\"");
        let once = WRAPS[inner].replace("{q}", &q).replace("{c}", c);
        let q2 = once.replace('\'', "\"");
        let twice = WRAPS[outer].replace("{q}", &q2).replace("{c}", &once);
        prop_assert_ne!(verdict(c), Verdict::Allow);
        prop_assert_ne!(verdict(&once), Verdict::Allow, "{}", once);
        prop_assert_ne!(verdict(&twice), Verdict::Allow, "{}", twice);
    }

    /// `check` never panics, whatever the command.
    #[test]
    fn check_never_panics(command in ".{0,200}") {
        let _ = verdict(&command);
    }
}

#[test]
fn an_alternate_reading_is_checked_against_g1() {
    // the brace reading `npx playwright install` is an alternate (SH lead ruling Q3=A): G1 blocks it
    assert_eq!(
        verdict("npx playwright {install,}"),
        Verdict::Block(Rule::G1)
    );
    assert_eq!(verdict("cp a.{js,ts} dir"), Verdict::Allow);
    // SH-seq review M-2: only the sequence alternate shows the install (bash runs `npm -1 i puppeteer`)
    for command in [
        "npm {-1..-1} i puppeteer",
        "npm --prefix ${HOME:+x} {-1..-1} i puppeteer",
    ] {
        assert_eq!(verdict(command), Verdict::Block(Rule::G1), "{command}");
    }
    // C-1, I-1: a reading that may not be bash's is too complex, never allowed
    for command in [
        "npm --prefix \"{1..2}\" {-1..-1} i puppeteer",
        "npm --prefix '{1..2}' i {puppeteer,x}",
        "npm --prefix \"{a,b}\" {-x,} i puppeteer",
        "npm {-1..{1,}} i puppeteer",
        "echo \"npm --prefix '{1..2}'\" {-1..-1} i puppeteer | sh",
        // re-review R-1: bash leaves braces inside `${...}` alone
        "npm --prefix ${x:-{a,b}} {-x,} i puppeteer",
        "npm --prefix ${x:+{a,b}} {-x,} i puppeteer",
        "npm --prefix ${x:-\\{a,b\\}} {-x,} i puppeteer",
        "npm --prefix ${x:-{1..2}} {-1..-1} i puppeteer",
        "npm --prefix \"${x:-{a,b}}\" {-x,} i puppeteer",
    ] {
        assert_ne!(verdict(command), Verdict::Allow, "{command}");
    }
}
