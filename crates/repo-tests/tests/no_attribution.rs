//! The attribution rule: `plugins/anthology/hooks/no-attribution.sh`, a PreToolUse hook on Bash.
//!
//! The script runs for real under `sh` with a Bash hook input on stdin. A `git commit` or
//! `gh pr create|edit` whose text carries an assistant co-author trailer or a "Generated with"
//! line is denied with a reason that names the rule; everything else is allowed with no output.

use std::io::Write;
use std::process::{Command, Output, Stdio};

use anthology_pycompat::json::{self, Value};
use repo_tests::repo;

const SCRIPT: &str = "plugins/anthology/hooks/no-attribution.sh";

/// A Bash PreToolUse input for `command`, spelled as Claude Code sends it.
fn hook_input(command: &str) -> String {
    hook_input_described(command, None)
}

/// The same, with a `description` next to the command in tool_input.
fn hook_input_described(command: &str, description: Option<&str>) -> String {
    let mut tool_input = json::Map::new();
    tool_input.insert("command".into(), Value::Str(command.into()));
    if let Some(d) = description {
        tool_input.insert("description".into(), Value::Str(d.into()));
    }
    let mut m = json::Map::new();
    m.insert("session_id".into(), Value::Str("s".into()));
    m.insert("permission_mode".into(), Value::Str("default".into()));
    m.insert("tool_name".into(), Value::Str("Bash".into()));
    m.insert("tool_input".into(), Value::Object(tool_input));
    json::dumps(&Value::Object(m), &json::PY_DEFAULT)
}

fn run_raw(stdin: &[u8], work: &std::path::Path) -> Output {
    let mut child = Command::new("/bin/sh")
        .arg(repo::path(SCRIPT))
        .current_dir(work)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let _ = child.stdin.take().unwrap().write_all(stdin);
    child.wait_with_output().unwrap()
}

fn run(command: &str) -> Output {
    run_raw(hook_input(command).as_bytes(), &std::env::temp_dir())
}

/// The `permissionDecisionReason` of a deny answer, or a panic that shows the output.
fn deny_reason(out: &Output, what: &str) -> String {
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert_eq!(out.status.code(), Some(0), "{what}: exit");
    let v = json::loads(&text).unwrap_or_else(|e| panic!("{what}: not JSON ({e:?}): {text:?}"));
    let hook = v
        .as_object()
        .and_then(|m| m.get("hookSpecificOutput"))
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("{what}: no hookSpecificOutput: {text:?}"));
    assert_eq!(
        hook.get("hookEventName").and_then(Value::as_str),
        Some("PreToolUse"),
        "{what}"
    );
    assert_eq!(
        hook.get("permissionDecision").and_then(Value::as_str),
        Some("deny"),
        "{what}: {text:?}"
    );
    hook.get("permissionDecisionReason")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{what}: no reason: {text:?}"))
        .to_string()
}

fn assert_allowed(command: &str) {
    let out = run(command);
    assert_eq!(out.status.code(), Some(0), "{command:?}");
    assert_eq!(
        (&out.stdout[..], &out.stderr[..]),
        (&b""[..], &b""[..]),
        "{command:?}: allowed means no output"
    );
}

#[test]
fn a_commit_with_an_assistant_co_author_trailer_is_denied_and_the_reason_names_the_rule() {
    let out = run(
        "git commit -m \"feat: x\n\nCo-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>\"",
    );
    let reason = deny_reason(&out, "commit");
    assert!(reason.contains("git config author"), "{reason}");
    assert!(reason.contains("Co-Authored-By"), "{reason}");
    assert!(reason.contains("Generated with"), "{reason}");
}

#[test]
fn every_attribution_spelling_on_a_commit_or_pr_command_is_denied() {
    let cases = [
        (
            "trailer, name only",
            "git commit -m \"x\n\nCo-Authored-By: Claude <a@b.c>\"",
        ),
        (
            "trailer, address only",
            "git commit -m \"x\n\nCo-Authored-By: Bot <noreply@anthropic.com>\"",
        ),
        (
            "trailer, lower case",
            "git commit -m \"x\n\nco-authored-by: claude <a@b.c>\"",
        ),
        (
            "trailer, upper case",
            "git commit -m \"x\n\nCO-AUTHORED-BY: ANTHROPIC\"",
        ),
        (
            "trailer, no space after the colon",
            "git commit -m \"x\n\nCo-Authored-By:Claude\"",
        ),
        (
            "trailer last, after an earlier human one",
            "git commit -m \"x\n\nCo-Authored-By: Alex <a@e.com>\nCo-Authored-By: Claude <a@b.c>\"",
        ),
        (
            "amend",
            "git commit --amend -m \"x\n\nCo-Authored-By: Claude <a@b.c>\"",
        ),
        (
            "git options before commit",
            "git -C /w commit -m \"x\n\nCo-Authored-By: Claude <a@b.c>\"",
        ),
        (
            "heredoc message in a chain",
            "git add -A && git commit -m \"$(cat <<'EOF'\nx\n\nCo-Authored-By: Claude <a@b.c>\nEOF\n)\"",
        ),
        (
            "escaped quote before the name",
            "git commit -m \"x Co-Authored-By: \\\"Claude\\\"\"",
        ),
        (
            "pr create, Generated with the link",
            "gh pr create --title t --body \"Body\n\n🤖 Generated with [Claude Code](https://claude.com/claude-code)\"",
        ),
        (
            "pr create, Generated with a name",
            "gh pr create --title t --body \"Generated with Claude\"",
        ),
        (
            "pr edit, lower case",
            "gh pr edit 5 --body \"generated with claude code\"",
        ),
        (
            "pr create, trailer in the body",
            "gh pr create --title t --body \"Co-Authored-By: Claude <a@b.c>\"",
        ),
        (
            "commit, Generated with line",
            "git commit -m \"x\n\nGenerated with [Claude Code]\"",
        ),
    ];
    for (what, command) in cases {
        let out = run(command);
        let reason = deny_reason(&out, what);
        assert!(reason.contains("git config author"), "{what}: {reason}");
        assert!(out.stderr.is_empty(), "{what}: stderr");
    }
}

#[test]
fn commits_and_pr_commands_without_attribution_are_allowed() {
    for command in [
        "git commit -m \"feat: add the hook\"",
        "git commit --amend --no-edit",
        "git add -A && git commit -m \"fix: y\"",
        "gh pr create --title t --body \"Adds the hook\"",
        "gh pr edit 5 --body \"More detail\"",
        // A human co-author is fine.
        "git commit -m \"x\n\nCo-Authored-By: Alex <alex@example.com>\"",
        // The words are on another line of the message, not in the trailer.
        "git commit -m \"x\n\nCo-Authored-By: Alex <alex@example.com>\n\nfix the claude parser\"",
        "git commit -m \"Co-Authored-By: Alex\n\nnotes about anthropic\"",
        // "Generated with" on its own is not an attribution.
        "gh pr create --title t --body \"Generated with cargo\"",
    ] {
        assert_allowed(command);
    }
}

#[test]
fn other_commands_that_only_mention_the_text_are_allowed() {
    for command in [
        "echo \"Co-Authored-By: Claude <a@b.c>\"",
        "grep -rn \"Generated with Claude\" docs",
        "git log --grep \"Co-Authored-By: Claude\"",
        "gh pr view 5 --json body",
        "gh pr create-note \"Generated with Claude\"",
        "git status",
        "ls",
    ] {
        assert_allowed(command);
    }
}

#[test]
fn the_hook_catches_the_usual_habit_and_is_not_a_guarantee() {
    // Documented gaps: the text must sit in the command itself, in the `Name: value` spelling.
    for command in [
        "git commit --trailer 'Co-authored-by=Claude <noreply@anthropic.com>' -m x",
        "git commit -F msg.txt",
        "gh pr create --title t --body-file body.md",
    ] {
        assert_allowed(command);
    }
}

#[test]
fn only_the_command_value_is_searched_not_the_description_or_other_fields() {
    let work = std::env::temp_dir();
    let allowed = |command: &str, description: &str| {
        let out = run_raw(
            hook_input_described(command, Some(description)).as_bytes(),
            &work,
        );
        assert_eq!(out.status.code(), Some(0), "{command:?} / {description:?}");
        assert_eq!(
            (&out.stdout[..], &out.stderr[..]),
            (&b""[..], &b""[..]),
            "{command:?} / {description:?}"
        );
    };
    // A human trailer last in the message, then a description that names the assistant.
    allowed(
        "git commit -m \"x\n\nCo-Authored-By: Alex <alex@example.com>\"",
        "Commit the Claude Code hook fix",
    );
    // A command that is not a commit, with a description that says "git commit".
    allowed(
        "echo \"Co-Authored-By: Claude\"",
        "show what git commit adds",
    );
    // The trailer is outside the command, so it is not searched.
    allowed("git commit -m \"x\"", "Co-Authored-By: Claude <a@b.c>");
    allowed("ls", "git commit with Generated with Claude");
    // The same trailer inside the command is still denied, whatever the description says.
    let out = run_raw(
        hook_input_described(
            "git commit -m \"x\n\nCo-Authored-By: Claude <a@b.c>\"",
            Some("A neutral description"),
        )
        .as_bytes(),
        &work,
    );
    deny_reason(&out, "trailer in the command");
}

#[test]
fn invalid_utf8_in_the_input_does_not_hide_a_trailer() {
    // A raw \xff\xfe byte pair must not make the extraction fail (BSD tr aborts on it without
    // LC_ALL=C), which would let the commit through.
    let mut stdin = br#"{"tool_name":"Bash","tool_input":{"command":"git commit -m \"x "#.to_vec();
    stdin.extend_from_slice(&[0xff, 0xfe]);
    stdin.extend_from_slice(br#"\n\nCo-Authored-By: Claude <a@b.c>\""}}"#);
    let out = run_raw(&stdin, &std::env::temp_dir());
    deny_reason(&out, "invalid UTF-8 before the trailer");
}

#[test]
fn odd_input_is_allowed_with_no_output() {
    let work = std::env::temp_dir();
    for stdin in [
        &b""[..],
        b"{}",
        b"not json",
        b"\n\n",
        b"{\"tool_name\":\"Bash\"}",
    ] {
        let out = run_raw(stdin, &work);
        assert_eq!(out.status.code(), Some(0), "{stdin:?}");
        assert_eq!(
            (&out.stdout[..], &out.stderr[..]),
            (&b""[..], &b""[..]),
            "{stdin:?}"
        );
    }
}

#[test]
fn the_input_is_only_searched_never_run() {
    let dir = tempfile::tempdir().unwrap();
    let probe = dir.path().join("PWNED");
    let command = format!(
        "git commit -m \"$(touch {p}) `touch {p}` Co-Authored-By: Claude\"",
        p = probe.display()
    );
    let out = run_raw(hook_input(&command).as_bytes(), dir.path());
    deny_reason(&out, "injection text");
    assert!(!probe.exists(), "nothing in the input is executed");
    assert_allowed(&format!("git commit -m \"$(touch {})\"", probe.display()));
    assert!(!probe.exists(), "nothing in the input is executed");
}

#[test]
fn the_script_is_posix_sh_that_never_evals() {
    let text = repo::read(SCRIPT);
    assert_eq!(text.lines().next(), Some("#!/bin/sh"));
    for line in text.lines().filter(|l| !l.trim_start().starts_with('#')) {
        for bad in ["eval ", "[[ ", "local ", "source "] {
            assert!(!line.contains(bad), "{bad:?} in {line:?}");
        }
    }
}
