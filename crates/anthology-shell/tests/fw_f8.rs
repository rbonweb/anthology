//! SH-FW final F-8 (controller ruling): a program word hidden in an expansion is read as well as the shell reads it.
//!
//! (a) A program word `${x:-W}` (`${x-W}`, `${x:+W}`, `${x+W}`, `${x:=W}`, `${x=W}`) also reads as the same command
//!     with the literal `W` as its program word: an alternate, checked against G1 like the other alternates. A `W`
//!     that is not literal, or text around the expansion, is too complex.
//! (b) A program word that is, or holds, a command substitution or a backtick is too complex, except that
//!     `$(which X)`, `$(command -v X)`, `$(type -p X)` and `$(realpath X)` read as `X`.
//!
//! Each payload must be blocked: an install entry or alternate, or too complex. The controls stay complete and
//! install nothing. `npm ${x:-i} puppeteer` (a default in an argument, not the program word) is not covered by
//! F-8 and is left as it was.

use anthology_shell::{Analysis, Entry, analyze};

const HOME: &str = "/home/tester";

const PLAYWRIGHT: &[&str] = &[
    "playwright",
    "playwright-core",
    "@playwright/test",
    "@playwright/cli",
];
const PLAYWRIGHT_SUBCOMMANDS: &[&str] = &["install", "install-deps", "test", "codegen", "open"];

/// guard_rules.py `_g1`, for the playwright and puppeteer installs these payloads use.
fn g1(e: &Entry) -> bool {
    if e.exempt {
        return false;
    }
    let words: Vec<&str> = e
        .args
        .iter()
        .map(String::as_str)
        .filter(|a| !a.starts_with('-'))
        .collect();
    if PLAYWRIGHT.contains(&e.prog.as_str()) {
        return words
            .first()
            .is_some_and(|s| PLAYWRIGHT_SUBCOMMANDS.contains(s));
    }
    if ["npm", "pnpm", "yarn", "bun"].contains(&e.prog.as_str()) {
        return words
            .first()
            .is_some_and(|v| ["i", "install", "add"].contains(v))
            && words[1..].contains(&"puppeteer");
    }
    false
}

/// An entry or an alternate reading of `a` is a G1 install.
fn installs(a: &Analysis) -> bool {
    a.entries.iter().chain(&a.alternates).any(g1)
}

fn summary(a: &Analysis) -> String {
    let show = |e: &Entry| format!("{} {:?}", e.prog, e.args);
    format!(
        "entries {:?}, alternates {:?}, incomplete {}",
        a.entries.iter().map(show).collect::<Vec<_>>(),
        a.alternates.iter().map(show).collect::<Vec<_>>(),
        a.incomplete
    )
}

/// The payloads that are blocked by an install entry or alternate, or by being too complex.
const PAYLOADS: &[&str] = &[
    "${x:-npx} playwright install",
    "${x-npx} playwright install",
    "x=1; ${x:+npx} playwright install",
    "${x:=npm} i puppeteer",
    "$(echo npx playwright install)",
    "`echo npx` playwright install",
    "\"$(echo npx)\" playwright install",
    "sudo $(echo npx) playwright install",
    "$(which npx) playwright install",
];

/// The forms that must stay complete and install nothing.
const CONTROLS: &[&str] = &[
    "$(which python3) -m venv .venv",
    "\"$(command -v node)\" --version",
    "${EDITOR:-vi} file.txt",
    "${PYTHON:-python3} script.py",
    "echo $(date)",
    "x=$(git rev-parse HEAD)",
];

#[test]
fn every_f8_payload_is_blocked() {
    let missed: Vec<String> = PAYLOADS
        .iter()
        .filter_map(|command| {
            let a = analyze(command, HOME);
            (!a.incomplete && !installs(&a)).then(|| format!("{command:?}: {}", summary(&a)))
        })
        .collect();
    assert!(missed.is_empty(), "allowed:\n{}", missed.join("\n"));
}

#[test]
fn a_default_or_alternate_program_also_reads_its_default() {
    // (a): the main entry stays, and the alternate is the command with W as the program word
    for command in [
        "${x:-npx} playwright install",
        "${x-npx} playwright install",
        "x=1; ${x:+npx} playwright install",
        "${x+npx} playwright install",
        "${x:=npx} playwright install",
        "${x=npx} playwright install",
        "sudo ${x:-npx} playwright install",
        "${x:-\"npx\"} playwright install",
    ] {
        let a = analyze(command, HOME);
        assert!(!a.incomplete, "{command}: {}", summary(&a));
        assert!(a.alternates.iter().any(g1), "{command}: {}", summary(&a));
        // the main entry is the command as it stands: its program word is the expansion
        assert!(
            a.entries.iter().any(|e| e.prog.contains("${")),
            "{command}: {}",
            summary(&a)
        );
    }
    // the alternate of `${x:=npm} i puppeteer` is the install itself
    let a = analyze("${x:=npm} i puppeteer", HOME);
    assert!(
        a.alternates
            .iter()
            .any(|e| e.prog == "npm" && e.args == ["i", "puppeteer"])
    );
}

#[test]
fn a_default_that_is_not_literal_is_too_complex() {
    for command in [
        "${x:-np*} playwright install",
        "${x:-npx}/bin/tool install",
        "pre${x:-npx} playwright install",
        "${x:-${y}} playwright install",
        "${x:-$(echo npx)} playwright install",
        "${x:-`echo npx`} playwright install",
        "${x:-npx y} playwright install",
    ] {
        assert!(
            analyze(command, HOME).incomplete,
            "{command}: not too complex"
        );
    }
}

#[test]
fn a_substitution_at_program_position_is_too_complex() {
    for command in [
        "$(echo npx playwright install)",
        "`echo npx` playwright install",
        "\"$(echo npx)\" playwright install",
        "sudo $(echo npx) playwright install",
        "$(echo npm) i puppeteer",
    ] {
        assert!(
            analyze(command, HOME).incomplete,
            "{command}: not too complex"
        );
    }
}

#[test]
fn a_lookup_of_one_literal_word_reads_that_word() {
    // (b): `$(which npx)` is `npx`, which runs the same program as `npx playwright install`
    let a = analyze("$(which npx) playwright install", HOME);
    assert!(!a.incomplete, "{}", summary(&a));
    assert!(installs(&a), "{}", summary(&a));
    assert!(
        a.entries
            .iter()
            .any(|e| e.prog == "which" && e.args == ["npx"])
    );

    let a = analyze("sudo $(which npx) playwright install", HOME);
    assert!(!a.incomplete && installs(&a), "{}", summary(&a));

    let a = analyze("`which npx` playwright install", HOME);
    assert!(!a.incomplete && installs(&a), "{}", summary(&a));

    // a lookup whose word is not literal is too complex
    for command in [
        "$(which \"$x\") playwright install",
        "$(which -a npx) playwright install",
        "$(type -P npx) playwright install",
    ] {
        assert!(
            analyze(command, HOME).incomplete,
            "{command}: not too complex"
        );
    }
}

#[test]
fn the_controls_stay_complete_and_install_nothing() {
    for command in CONTROLS {
        let a = analyze(command, HOME);
        assert!(!a.incomplete && !installs(&a), "{command}: {}", summary(&a));
    }
    // the program words the controls read
    let a = analyze("$(which python3) -m venv .venv", HOME);
    assert!(
        a.entries
            .iter()
            .any(|e| e.prog == "which" && e.args == ["python3"])
    );
    let a = analyze("\"$(command -v node)\" --version", HOME);
    assert!(
        a.entries
            .iter()
            .any(|e| e.prog == "node" && e.args == ["--version"])
    );
    // a substitution that is an argument (or a flag's value) is not a program word
    let a = analyze("sudo -u $(whoami) npm i puppeteer", HOME);
    assert!(!a.incomplete && installs(&a), "{}", summary(&a));
    assert!(a.entries.iter().any(|e| e.prog == "whoami"));
}
