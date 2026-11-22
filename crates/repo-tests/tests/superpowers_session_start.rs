//! `plugins/anthology-superpowers/hooks/session-start` and subagents.
//!
//! When a subagent compacts, Claude Code runs SessionStart inside it and puts a non-empty
//! `agent_id` in the hook input. The superpowers introduction is for the main session, so the
//! hook must print nothing then, and keep printing it for everything else: the main session, a
//! session started with `--agent` (`agent_type` only), other harnesses and no input at all.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use repo_tests::repo;

const HOOK: &str = "plugins/anthology-superpowers/hooks/session-start";
const SUBAGENT: &str = r#"{"hook_event_name":"SessionStart","source":"compact","agent_id":"a0b1c2d3e4f5a6b7c","agent_type":"anthology:implementer"}"#;
const MAIN: &str = r#"{"hook_event_name":"SessionStart","source":"startup","model":"m"}"#;
const INTRO: &str = "anthology-superpowers:using-superpowers";

/// What the hook did.
struct Ran {
    stdout: String,
    code: Option<i32>,
    took: Duration,
}

impl Ran {
    fn silent(&self) -> bool {
        self.code == Some(0) && self.stdout.is_empty()
    }

    fn intro(&self) -> bool {
        self.code == Some(0)
            && self.stdout.contains("hookSpecificOutput")
            && self.stdout.contains(INTRO)
    }
}

fn command(env: &[(&str, &str)], path: &str, home: &std::path::Path) -> Command {
    let mut cmd = Command::new("/bin/bash");
    cmd.arg(repo::path(HOOK))
        .env_clear()
        .env("PATH", path)
        .env("HOME", home)
        .env(
            "CLAUDE_PLUGIN_ROOT",
            repo::path("plugins/anthology-superpowers"),
        )
        .envs(env.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

/// Runs the hook with `stdin` written and closed.
fn run_with(stdin: &str, env: &[(&str, &str)], path: &str) -> Ran {
    let home = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let mut child = command(env, path, home.path()).spawn().unwrap();
    // The hook may exit without reading it all.
    let _ = child.stdin.take().unwrap().write_all(stdin.as_bytes());
    let out = child.wait_with_output().unwrap();
    Ran {
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        code: out.status.code(),
        took: started.elapsed(),
    }
}

fn run(stdin: &str) -> Ran {
    run_with(stdin, &[], "/usr/bin:/bin")
}

#[test]
fn a_subagent_gets_no_introduction() {
    let ran = run(&format!("{SUBAGENT}\n"));
    assert!(ran.silent(), "{} / {:?}", ran.stdout, ran.code);
}

#[test]
fn the_input_may_have_no_newline_or_spaces_around_the_colon() {
    for input in [
        SUBAGENT.to_string(),
        SUBAGENT.replace("\"agent_id\":", "\"agent_id\" :  "),
        r#"{"agent_type":"x","agent_id":"a1"}"#.to_string(),
    ] {
        assert!(run(&input).silent(), "{input}");
    }
}

#[test]
fn a_subagent_with_an_empty_path_still_gets_none() {
    // The hook must keep to builtins (anthropics/claude-code#43127): no `cat`, no `grep`.
    let ran = run_with(SUBAGENT, &[], "");
    assert!(ran.silent(), "{} / {:?}", ran.stdout, ran.code);
}

#[test]
fn the_main_session_still_gets_the_introduction() {
    for input in [MAIN.to_string(), format!("{MAIN}\n")] {
        let ran = run(&input);
        assert!(ran.intro(), "{} / {:?}", ran.stdout, ran.code);
    }
}

#[test]
fn only_a_non_empty_agent_id_marks_a_subagent() {
    for extra in [
        r#""agent_type":"reviewer""#, // `--agent` sets this in the main session
        r#""agent_id":"""#,
        r#""agent_id":7"#,
        r#""agent_id":null"#,
        r#""note":"agent_id""#,
        r#""note":"\"agent_id\":\"x\"""#, // text inside a string is not a key
    ] {
        let input = format!(r#"{{"hook_event_name":"SessionStart","source":"compact",{extra}}}"#);
        assert!(run(&input).intro(), "{input}");
    }
}

#[test]
fn no_input_at_all_still_gets_the_introduction() {
    assert!(run("").intro());
}

#[test]
fn an_input_that_never_ends_cannot_hang_the_hook() {
    let home = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let mut child = command(&[], "/usr/bin:/bin", home.path()).spawn().unwrap();
    let held = child.stdin.take().unwrap(); // open, never written
    let status = child.wait().unwrap();
    let mut stdout = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    drop(held);
    assert_eq!(status.code(), Some(0));
    assert!(stdout.contains(INTRO));
    assert!(started.elapsed() < Duration::from_secs(20));
}

#[test]
fn other_harnesses_are_not_asked() {
    // They pass no `agent_id`, and the hook must not wait on their stdin.
    for env in [
        ("CURSOR_PLUGIN_ROOT", "/x"),
        ("COPILOT_CLI", "1"),
        ("MUSE_PLUGIN_ROOT", "/x"),
    ] {
        let ran = run_with(SUBAGENT, &[env], "/usr/bin:/bin");
        assert_eq!(ran.code, Some(0), "{env:?}");
        assert!(ran.stdout.contains(INTRO), "{env:?}");
        assert!(ran.took < Duration::from_secs(20));
    }
}
