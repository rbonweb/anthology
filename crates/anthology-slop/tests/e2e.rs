//! End to end: every rule fires on a real sample of its kind, and a valid waiver on the same line
//! turns the block into a listed waiver. The ship gate for the slop crate.

use anthology_slop::{Kind, RULES, check_all, kind_of, rule};
use std::path::Path;

const SHOULD_FAIL: &str = include_str!("fixtures/should-fail.txt");

const KINDS: [Kind; 4] = [Kind::Markdown, Kind::Html, Kind::Jsx, Kind::Css];

/// One line of `should-fail.txt`.
struct Case {
    rule: &'static str,
    kind: Kind,
    text: String,
}

/// Reads `rule<TAB>kind<TAB>text` lines; `\n` in the text stands for a newline.
fn cases() -> Vec<Case> {
    let mut out = Vec::new();
    for (n, line) in SHOULD_FAIL.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let mut parts = line.splitn(3, '\t');
        let (Some(id), Some(kind), Some(text)) = (parts.next(), parts.next(), parts.next()) else {
            panic!(
                "should-fail.txt line {}: want rule<TAB>kind<TAB>text",
                n + 1
            );
        };
        let Some(rule) = rule(id) else {
            panic!("should-fail.txt line {}: unknown rule {id}", n + 1);
        };
        let Some(kind) = KINDS.into_iter().find(|k| k.name() == kind) else {
            panic!("should-fail.txt line {}: unknown kind {kind}", n + 1);
        };
        out.push(Case {
            rule: rule.id,
            kind,
            text: text.replace("\\n", "\n"),
        });
    }
    out
}

/// The comment that waives `rule` on the line it ends, in the comment style of `kind`.
fn waiver_comment(kind: Kind, rule: &str, reason: &str) -> String {
    match kind {
        Kind::Markdown | Kind::Html => format!("<!-- slop-ok: {rule} {reason} -->"),
        Kind::Jsx => format!("{{/* slop-ok: {rule} {reason} */}}"),
        Kind::Css => format!("/* slop-ok: {rule} {reason} */"),
    }
}

#[test]
fn the_fixture_holds_twenty_five_cases() {
    assert_eq!(cases().len(), 25);
}

#[test]
fn every_case_uses_a_kind_its_rule_runs_on() {
    let bad: Vec<String> = cases()
        .iter()
        .filter(|c| !rule(c.rule).is_some_and(|r| r.kinds.contains(&c.kind)))
        .map(|c| format!("{} {}: {}", c.rule, c.kind.name(), c.text))
        .collect();
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn kind_names_match_the_extensions_the_gate_reads() {
    for (file, kind) in [
        ("a.md", Kind::Markdown),
        ("a.html", Kind::Html),
        ("a.tsx", Kind::Jsx),
        ("a.css", Kind::Css),
    ] {
        assert_eq!(kind_of(Path::new(file)), Some(kind), "{file}");
    }
}

#[test]
fn every_should_fail_case_raises_its_rule() {
    let missing: Vec<String> = cases()
        .iter()
        .filter(|c| {
            !check_all(c.kind, &c.text)
                .iter()
                .any(|f| f.rule == c.rule && f.waiver.is_none())
        })
        .map(|c| format!("{} {}: {}", c.rule, c.kind.name(), c.text))
        .collect();
    assert!(
        missing.is_empty(),
        "{} case(s) raise no unwaived finding:\n{}",
        missing.len(),
        missing.join("\n")
    );
}

#[test]
fn every_rule_has_a_should_fail_case() {
    let cases = cases();
    let missing: Vec<&str> = RULES
        .iter()
        .map(|r| r.id)
        .filter(|id| !cases.iter().any(|c| c.rule == *id))
        .collect();
    assert!(missing.is_empty(), "no case for: {}", missing.join(", "));
}

#[test]
fn waived_cases_do_not_block() {
    const REASON: &str = "named by the user";
    // The first case of each rule, a one-line sample, with its waiver appended on the same line.
    let mut seen = Vec::new();
    let mut failures = Vec::new();
    for case in cases() {
        if seen.contains(&case.rule) || case.text.contains('\n') {
            continue;
        }
        seen.push(case.rule);
        let doc = format!(
            "{} {}",
            case.text,
            waiver_comment(case.kind, case.rule, REASON)
        );
        let found: Vec<_> = check_all(case.kind, &doc)
            .into_iter()
            .filter(|f| f.rule == case.rule)
            .collect();
        let waived = !found.is_empty() && found.iter().all(|f| f.waiver.as_deref() == Some(REASON));
        if !waived {
            failures.push(format!("{} {}: {found:?}", case.rule, case.kind.name()));
        }
    }
    assert_eq!(
        seen.len(),
        RULES.len(),
        "a rule has no one-line case: {seen:?}"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
