// Unix only: on Windows the home directory is a known folder the environment cannot redirect.
#![cfg(unix)]
//! Fork change 1 (rbonweb/rtk cedececf): `atl init` for Claude Code adds
//! `Bash(atl *)` to `permissions.allow`, so a rewritten command is allowed
//! (exit 0) instead of asked (exit 3), and heals an install that lacks it.

use std::path::Path;
use std::process::{Command, Stdio};

mod common;

const ATL_ALLOW: &str = "Bash(atl *)";

fn atl_init(home: &Path, project: &Path) -> Command {
    let mut cmd = common::atl_command();
    cmd.current_dir(project)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local").join("share"))
        .env("ATL_DB_PATH", home.join("atl.db"))
        .env("ATL_TELEMETRY_DISABLED", "1")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .args(["init", "-g", "--auto-patch"]);
    cmd
}

fn settings_path(home: &Path) -> std::path::PathBuf {
    home.join(".claude").join("settings.json")
}

fn settings(home: &Path) -> serde_json::Value {
    let text = std::fs::read_to_string(settings_path(home))
        .expect("atl init writes ~/.claude/settings.json");
    serde_json::from_str(&text).expect("settings.json is JSON")
}

fn allow_rules(settings: &serde_json::Value) -> Vec<String> {
    settings["permissions"]["allow"]
        .as_array()
        .map(|rules| {
            rules
                .iter()
                .filter_map(|r| r.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn fork_1_init_adds_the_atl_allow_rule_once() {
    let home = common::tempdir();
    let project = common::tempdir();

    let first = atl_init(home.path(), project.path())
        .output()
        .expect("run atl init");
    assert_eq!(first.status.code(), Some(0), "{first:?}");
    let after_first = allow_rules(&settings(home.path()));
    assert!(
        after_first.iter().any(|r| r == ATL_ALLOW),
        "permissions.allow must carry `{ATL_ALLOW}`, got {after_first:?}"
    );

    let second = atl_init(home.path(), project.path())
        .output()
        .expect("rerun atl init");
    assert_eq!(second.status.code(), Some(0), "{second:?}");
    let after_second = allow_rules(&settings(home.path()));
    assert_eq!(
        after_second.iter().filter(|r| *r == ATL_ALLOW).count(),
        1,
        "the rule is added once, got {after_second:?}"
    );
}

#[test]
fn fork_1_init_heals_an_installed_hook_without_the_allow_rule() {
    let home = common::tempdir();
    let project = common::tempdir();
    let install = atl_init(home.path(), project.path())
        .output()
        .expect("run atl init");
    assert_eq!(install.status.code(), Some(0), "{install:?}");

    // Keep the hook, drop the allow rule: an install made before the fork change.
    let mut stale = settings(home.path());
    stale["permissions"]["allow"] = serde_json::json!(["Bash(ls)"]);
    std::fs::write(
        settings_path(home.path()),
        serde_json::to_string_pretty(&stale).expect("serialize settings"),
    )
    .expect("write stale settings");

    let heal = atl_init(home.path(), project.path())
        .output()
        .expect("rerun atl init");
    assert_eq!(heal.status.code(), Some(0), "{heal:?}");
    let rules = allow_rules(&settings(home.path()));
    assert!(rules.iter().any(|r| r == "Bash(ls)"), "{rules:?}");
    assert!(rules.iter().any(|r| r == ATL_ALLOW), "{rules:?}");
}
