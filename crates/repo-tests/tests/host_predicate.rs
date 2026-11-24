//! The embedded-host rule has three readers that must agree: the guard (`host::embedded`, which
//! session start and the G0 message share), the `anthology-superpowers` session-start hook and the
//! delegation mod (`isEmbeddedHost`). The rule: mode `full` turns it off; otherwise
//! `CLAUDE_CODE_SESSION_ATTENDED` decides when it is `0` (embedded) or `1` (not); otherwise an
//! `sdk-` entrypoint marks a host (DECISIONS.md, ADR-037).
//!
//! The truth table lives once, in `plugins/anthology/tests/mods/delegation.test.ts`, between the
//! `host-truth-table` markers. The mods job runs it against `isEmbeddedHost`; this test runs the
//! same rows against the guard and against the real hook, so a reader that drifts fails here.

use std::io::Write;
use std::process::{Command, Stdio};

use anthology_guard::host;
use regex::Regex;
use repo_tests::repo;

const HOOK: &str = "plugins/anthology-superpowers/hooks/session-start";
const TABLE: &str = "plugins/anthology/tests/mods/delegation.test.ts";
const MAIN: &str = r#"{"hook_event_name":"SessionStart","source":"startup","model":"m"}"#;
const INTRO: &str = "anthology-superpowers:using-superpowers";

/// One row: the entrypoint, attended and mode variables (`None` is unset) and the expected answer.
type Row = (Option<String>, Option<String>, Option<String>, bool);

fn rows() -> Vec<Row> {
    let text = repo::read(TABLE);
    let begin = "// host-truth-table:begin";
    let end = "// host-truth-table:end";
    let from = text.find(begin).expect("begin marker") + begin.len();
    let to = text.find(end).expect("end marker");
    let value = r"(undefined|'[^']*')";
    let row = Regex::new(&format!(r"^\[{value}, {value}, {value}, (true|false)\],$")).unwrap();
    let var = |s: &str| (s != "undefined").then(|| s.trim_matches('\'').to_string());
    let mut rows = Vec::new();
    for line in text[from..to].lines().map(str::trim) {
        // The declaration, the closing bracket and the comments are not rows.
        if !line.starts_with('[') || line.starts_with("[]") {
            continue;
        }
        let c = row
            .captures(line)
            .unwrap_or_else(|| panic!("a row the test cannot read: {line}"));
        rows.push((var(&c[1]), var(&c[2]), var(&c[3]), &c[4] == "true"));
    }
    rows
}

fn show(row: &Row) -> String {
    format!(
        "entrypoint {:?}, attended {:?}, mode {:?}",
        row.0, row.1, row.2
    )
}

/// Whether the hook printed its introduction (`true`) or nothing (`false`) in this environment.
fn hook_gives_intro(row: &Row) -> bool {
    let home = tempfile::tempdir().unwrap();
    let mut cmd = Command::new("/bin/bash");
    cmd.arg(repo::path(HOOK))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", home.path())
        .env(
            "CLAUDE_PLUGIN_ROOT",
            repo::path("plugins/anthology-superpowers"),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in [
        (host::ENTRYPOINT_VAR, &row.0),
        (host::ATTENDED_VAR, &row.1),
        (host::MODE_VAR, &row.2),
    ] {
        if let Some(v) = value {
            cmd.env(name, v);
        }
    }
    let mut child = cmd.spawn().unwrap();
    // The hook may exit without reading it all.
    let _ = child.stdin.take().unwrap().write_all(MAIN.as_bytes());
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", show(row));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.is_empty() || stdout.contains(INTRO),
        "{}: {stdout}",
        show(row)
    );
    !stdout.is_empty()
}

#[test]
fn the_table_has_the_rows_that_matter() {
    let rows = rows();
    assert!(rows.len() >= 30, "{} rows", rows.len());
    let has = |f: &dyn Fn(&Row) -> bool| rows.iter().any(f);
    // An interactive session that inherited an `sdk-*` entrypoint is not a host.
    assert!(has(&|r| {
        r.0.as_deref().is_some_and(|e| e.starts_with("sdk-"))
            && r.1.as_deref() == Some("1")
            && r.2.is_none()
            && !r.3
    }));
    // Unattended is a host whatever the entrypoint; mode `full` wins over both signals.
    assert!(has(&|r| r.1.as_deref() == Some("0")
        && r.2.is_none()
        && r.3));
    assert!(has(&|r| r.1.as_deref() == Some("0")
        && r.2.as_deref() == Some("full")
        && !r.3));
    // With attended unset, the `sdk-` entrypoint decides.
    assert!(has(&|r| {
        r.0.as_deref() == Some("sdk-cli") && r.1.is_none() && r.2.is_none() && r.3
    }));
}

#[test]
fn the_guard_follows_the_table() {
    for row in rows() {
        assert_eq!(
            host::embedded(row.0.as_deref(), row.1.as_deref(), row.2.as_deref()),
            row.3,
            "{}",
            show(&row)
        );
    }
}

#[test]
fn the_superpowers_hook_follows_the_table() {
    for row in rows() {
        assert_eq!(hook_gives_intro(&row), !row.3, "{}", show(&row));
    }
}
