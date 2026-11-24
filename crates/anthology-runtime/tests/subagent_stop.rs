//! The SubagentStop hook (`anthology hook subagent-stop`): a subagent that ends its turn while
//! its own shell or monitor is still running gets one more turn, with a reason that says to wait
//! in the foreground or stop the work. The inputs are the shapes Claude Code sends: the hook JSON
//! carries `stop_hook_active` and a `background_tasks` list of `{id, type, status, ...}` entries.

use anthology_runtime::subagent_stop::decide;

const SHELL: &str = r#"{"id":"b1","type":"shell","status":"running","command":"tail -f x"}"#;
const MONITOR: &str = r#"{"id":"m1","type":"monitor","status":"running","description":"watch"}"#;
const SUBAGENT: &str = r#"{"id":"a1","type":"subagent","status":"running","agent_type":"x"}"#;

fn input(active: &str, tasks: &[&str]) -> String {
    format!(
        r#"{{"hook_event_name":"SubagentStop","stop_hook_active":{active},"last_assistant_message":"done","background_tasks":[{}],"session_crons":[]}}"#,
        tasks.join(",")
    )
}

fn block(shells: usize, monitors: usize) -> String {
    format!(
        r#"{{"decision":"block","reason":"You still have background work running: {shells} shell, {monitors} monitor. Wait for it in the foreground (one blocking command with the Bash timeout parameter), or TaskStop it, then finish with your report."}}"#
    )
}

#[test]
fn a_running_shell_blocks_once() {
    // The subagent entry is the agent itself and is never counted.
    let first = decide(&input("false", &[SUBAGENT, SHELL]));
    assert_eq!(first, Some(block(1, 0)));
    // The same input after the one extra turn (stop_hook_active) lets the agent stop.
    assert_eq!(decide(&input("true", &[SUBAGENT, SHELL])), None);
}

#[test]
fn a_running_monitor_blocks() {
    assert_eq!(decide(&input("false", &[MONITOR])), Some(block(0, 1)));
    // Counts are per type.
    assert_eq!(
        decide(&input("false", &[SHELL, MONITOR, SHELL])),
        Some(block(2, 1))
    );
}

#[test]
fn stop_hook_active_allows() {
    assert_eq!(decide(&input("true", &[SHELL, MONITOR])), None);
}

#[test]
fn finished_tasks_allow() {
    let done_shell = r#"{"id":"b2","type":"shell","status":"completed"}"#;
    let killed_monitor = r#"{"id":"m2","type":"monitor","status":"killed"}"#;
    assert_eq!(decide(&input("false", &[done_shell, killed_monitor])), None);
    // A finished one next to a running one counts only the running one.
    assert_eq!(
        decide(&input("false", &[done_shell, MONITOR])),
        Some(block(0, 1))
    );
    assert_eq!(decide(&input("false", &[])), None);
}

#[test]
fn a_running_subagent_does_not_block() {
    assert_eq!(decide(&input("false", &[SUBAGENT])), None);
}

#[test]
fn bad_input_allows() {
    for raw in [
        "",
        "   ",
        "not json",
        "{",
        "[]",
        "null",
        "42",
        r#""text""#,
        "{}",
        r#"{"background_tasks":null}"#,
        r#"{"background_tasks":"shell"}"#,
        r#"{"background_tasks":{"type":"shell","status":"running"}}"#,
        r#"{"background_tasks":[1,"shell",null,[]]}"#,
        r#"{"background_tasks":[{"type":"shell"}]}"#,
        r#"{"background_tasks":[{"status":"running"}]}"#,
        r#"{"background_tasks":[{"type":7,"status":"running"}]}"#,
        r#"{"background_tasks":[{"type":"shell","status":true}]}"#,
        r#"{"background_tasks":[{"type":"other","status":"running"}]}"#,
    ] {
        assert_eq!(decide(raw), None, "{raw:?}");
    }
    // Only a real `true` ends the extra turn: a malformed flag does not hide a running shell.
    let odd = format!(r#"{{"stop_hook_active":"true","background_tasks":[{SHELL}]}}"#);
    assert_eq!(decide(&odd), Some(block(1, 0)));
    let absent = format!(r#"{{"background_tasks":[{SHELL}]}}"#);
    assert_eq!(decide(&absent), Some(block(1, 0)));
}
