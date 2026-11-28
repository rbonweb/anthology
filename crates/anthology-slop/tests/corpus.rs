//! Sentences a human writes that look like tells but are not: every rule stays silent on them.

use anthology_slop::{Kind, check_all};

const SHOULD_PASS: &str = include_str!("fixtures/should-pass.txt");

/// The entries of `should-pass.txt`, split on lines that are exactly `%%`.
fn entries() -> Vec<String> {
    let mut out = vec![String::new()];
    for line in SHOULD_PASS.lines() {
        if line == "%%" {
            out.push(String::new());
            continue;
        }
        let last = out.last_mut().expect("one entry");
        if !last.is_empty() {
            last.push('\n');
        }
        last.push_str(line);
    }
    out
}

#[test]
fn the_fixture_holds_twenty_entries() {
    let entries = entries();
    assert_eq!(entries.len(), 20);
    assert!(entries.iter().all(|e| !e.trim().is_empty()), "{entries:?}");
}

#[test]
fn should_pass_entries_raise_nothing() {
    let mut failures = Vec::new();
    for (n, entry) in entries().iter().enumerate() {
        for f in check_all(Kind::Markdown, entry) {
            if f.waiver.is_none() {
                failures.push(format!(
                    "entry {}: {} {} {}",
                    n + 1,
                    f.rule,
                    f.line,
                    f.excerpt
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
