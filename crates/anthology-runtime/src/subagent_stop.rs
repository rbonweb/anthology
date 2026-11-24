//! The SubagentStop hook (`anthology hook subagent-stop`).
//!
//! A subagent that ends its turn while its own `run_in_background` shell or Monitor is still
//! running makes Claude Code send the lead an interim "waiting on its own background work"
//! notice, and every later event wakes the agent again. This hook gives the agent one more turn
//! with a reason that tells it to wait in the foreground or stop the work.
//!
//! The hook input carries `stop_hook_active` (true on the extra turn this hook caused) and
//! `background_tasks`, a list of `{id, type, status, ...}` entries whose `type` is `shell`,
//! `monitor` or `subagent`. Only a `shell` or `monitor` entry with status `running` counts; a
//! `subagent` entry is never counted.

use anthology_pycompat::json::{self, Value};

/// What the hook prints for `raw` (the hook input JSON): `Some(json line)` to block the stop,
/// `None` to allow it (print nothing, exit 0).
///
/// This fails open on purpose: it is a nudge, not a security gate. Input that is missing, is not
/// JSON, is not an object or has no readable `background_tasks` list allows the stop, so a
/// change in the host's input shape can never trap an agent in a loop.
pub fn decide(raw: &str) -> Option<String> {
    let input = json::loads(raw).ok()?;
    let fields = input.as_object()?;
    // The one extra turn is the whole point: a second block would loop. Only a real `true` ends
    // it, so a malformed flag does not hide running work.
    if fields.get("stop_hook_active").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let (mut shells, mut monitors) = (0usize, 0usize);
    for task in fields.get("background_tasks")?.as_array()? {
        let Some(task) = task.as_object() else {
            continue;
        };
        if task.get("status").and_then(Value::as_str) != Some("running") {
            continue;
        }
        match task.get("type").and_then(Value::as_str) {
            Some("shell") => shells += 1,
            Some("monitor") => monitors += 1,
            _ => {}
        }
    }
    if shells + monitors == 0 {
        return None;
    }
    // The reason holds no character that needs JSON escaping, so the line is built directly.
    Some(format!(
        "{{\"decision\":\"block\",\"reason\":\"You still have background work running: {shells} shell, {monitors} monitor. Wait for it in the foreground (one blocking command with the Bash timeout parameter), or TaskStop it, then finish with your report.\"}}"
    ))
}
