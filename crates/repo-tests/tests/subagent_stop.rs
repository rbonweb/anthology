//! The SubagentStop nudge: `plugins/anthology/hooks/subagent-stop.sh` and its `SubagentStop` entry
//! in `plugins/anthology/hooks/hooks.json`. The script's behaviour runs under every shell in
//! `shims/entry.rs`; the decision itself is `crates/anthology-runtime/tests/subagent_stop.rs`.

use std::path::Path;

use anthology_pycompat::json::{self, Value};
use repo_tests::repo;

const SCRIPT: &str = "plugins/anthology/hooks/subagent-stop.sh";

fn subagent_stop_entries() -> Value {
    let hooks = json::loads(&repo::read("plugins/anthology/hooks/hooks.json")).unwrap();
    let events = hooks.as_object().unwrap().get("hooks").unwrap();
    events
        .as_object()
        .unwrap()
        .get("SubagentStop")
        .expect("a SubagentStop event")
        .clone()
}

#[test]
fn hooks_json_registers_subagent_stop() {
    let want = json::loads(
        r#"[
          {"hooks": [{"type": "command", "command": "sh",
              "args": ["${CLAUDE_PLUGIN_ROOT}/hooks/subagent-stop.sh"], "timeout": 10}]}
        ]"#,
    )
    .unwrap();
    assert_eq!(subagent_stop_entries(), want);
}

#[test]
fn the_subagent_stop_script_ships_and_is_executable() {
    assert!(Path::new(&repo::path(SCRIPT)).is_file());
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(repo::path(SCRIPT))
        .unwrap()
        .permissions()
        .mode();
    assert!(mode & 0o111 != 0, "executable");
}
