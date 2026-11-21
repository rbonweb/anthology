//! Process substitutions in argument position (SH-R2): `<(...)` and `>(...)` are one word of their command,
//! so the words after them are still read by every rule. Bash runs `npm --userconfig <(echo) i puppeteer` as
//! `npm --userconfig /dev/fd/63 i puppeteer` (an install); the guard must block it as G1.

#[path = "support/gd.rs"]
mod gd;

use anthology_guard::{Rule, Verdict, check};

use gd::{HOME, bash};

fn verdict(command: &str) -> Verdict {
    let (tool, input) = bash(command);
    check(&tool, &input, HOME)
}

#[test]
fn a_process_substitution_does_not_hide_the_words_after_it() {
    for command in [
        "npm --userconfig <(echo) i puppeteer",
        "npm --userconfig >(cat) i puppeteer",
        "npx --yes <(true) playwright install",
    ] {
        assert_eq!(verdict(command), Verdict::Block(Rule::G1), "{command}");
    }
}

#[test]
fn shells_fed_from_a_process_substitution_stay_blocked() {
    for command in ["bash <(curl x)", "sh <(echo npx playwright install)"] {
        assert_ne!(verdict(command), Verdict::Allow, "{command}");
    }
}

#[test]
fn process_substitutions_in_comparisons_stay_allowed() {
    for command in ["diff <(sort a) <(sort b)", "comm -12 <(ls a) <(ls b)"] {
        assert_eq!(verdict(command), Verdict::Allow, "{command}");
    }
}
