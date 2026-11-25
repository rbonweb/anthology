//! CO2: the hooks once the Python is gone (plan CO2; GC-15 F-1, F-2, F-3). The replacements the
//! ledger names for `test_guard_cli` `HooksJsonTest` and `SourceHygieneTest`, and for
//! `test_session_start` `HooksJsonTest` and `ShellWrapperTest`.
//!
//! The files CO2 moves out of `port/staging/` are read through `anthology_testkit::staged`, so
//! before the cutover these tests check the staged shims and after it the live ones. A test that
//! only holds once the Python plugin scripts are deleted skips while `port/staging/` exists. The
//! shim behaviour itself (no binary, no downloader, a broken install) is BS1's `shims.rs` and the
//! `binary-missing` smoke.

use std::path::PathBuf;

use anthology_pycompat::json::{self, Map, PY_DEFAULT, Value};
use repo_tests::repo;

const PLUGINS: [&str; 2] = ["anthology", "anthology-guardrails"];

/// Whether CO2 has moved `port/staging/` into place (the switch of `anthology_testkit::staged`).
fn cut_over() -> bool {
    !repo::path("port/staging").is_dir()
}

/// `plugins/<plugin>/<rel>` as the plugin ships after the cutover: the staged file while it
/// exists (before), else the live one.
fn plugin_file(plugin: &str, rel: &str) -> PathBuf {
    let rel = format!("plugins/{plugin}/{rel}");
    let staged = anthology_testkit::staged(&rel);
    if staged.exists() {
        staged
    } else {
        repo::path(&rel)
    }
}

fn read(path: &PathBuf) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn hooks_json(plugin: &str) -> (String, Value) {
    let text = read(&plugin_file(plugin, "hooks/hooks.json"));
    let value = json::loads(&text).unwrap_or_else(|e| panic!("{plugin}: {}", e.py_str("hooks")));
    (text, value)
}

fn get<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    v.as_object()?.get(key)
}

/// The `hooks` object of a hooks.json.
fn events(v: &Value) -> &Map {
    get(v, "hooks")
        .and_then(Value::as_object)
        .expect("a hooks object")
}

/// Every (event, hook) of a hooks.json, in file order.
fn hooks_of(v: &Value) -> Vec<(String, &Value)> {
    let mut out = Vec::new();
    for (event, groups) in events(v).iter() {
        for group in groups.as_array().expect("groups") {
            for hook in get(group, "hooks")
                .and_then(Value::as_array)
                .expect("hooks")
            {
                out.push((event.to_string(), hook));
            }
        }
    }
    out
}

fn str_of<'a>(v: &'a Value, key: &str) -> &'a str {
    get(v, key).and_then(Value::as_str).unwrap_or("")
}

/// The lines of a shell script that are not comments.
fn code_lines(text: &str) -> Vec<&str> {
    text.lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect()
}

/// The shims CO2 moves into place.
const SHIMS: [(&str, &str); 8] = [
    ("anthology", "hooks/lib.sh"),
    ("anthology", "hooks/bootstrap.sh"),
    ("anthology", "hooks/session-start.sh"),
    ("anthology", "bin/anthology"),
    ("anthology", "bin/anthology-mem"),
    ("anthology-guardrails", "hooks/lib.sh"),
    ("anthology-guardrails", "hooks/bootstrap.sh"),
    ("anthology-guardrails", "hooks/guard.sh"),
];

/// test_guard_cli `test_hooks_json_is_pretty_printed`, for both plugins.
#[test]
fn every_hooks_json_is_pretty_printed() {
    for plugin in PLUGINS {
        let (text, _) = hooks_json(plugin);
        assert!(text.ends_with('\n'), "{plugin}: ends with a newline");
        assert!(text.lines().count() > 5, "{plugin}: one key per line");
    }
}

/// test_guard_cli `test_hooks_json_is_the_exec_form_pretooluse_hook` and
/// `test_hooks_json_points_at_an_existing_script`, test_session_start
/// `test_hooks_json_has_the_session_start_entry_and_the_atl_pretooluse_entry`: every hook is the
/// exec form `/bin/sh <script under the plugin root>`, the script ships, and a PreToolUse hook
/// blocks when it fails. The exact entries are BS1's `statics.rs`.
#[test]
fn every_hook_is_an_exec_form_sh_command_on_a_shipped_script() {
    for plugin in PLUGINS {
        let (_, v) = hooks_json(plugin);
        let hooks = hooks_of(&v);
        assert!(!hooks.is_empty(), "{plugin}: no hooks");
        for (event, hook) in hooks {
            let at = format!("{plugin} {event}");
            assert_eq!(str_of(hook, "type"), "command", "{at}");
            assert_eq!(
                str_of(hook, "command"),
                "/bin/sh",
                "{at}: exec form, /bin/sh"
            );
            let args = get(hook, "args").and_then(Value::as_array).expect("args");
            let script = args.first().and_then(Value::as_str).expect("args[0]");
            let rel = script
                .strip_prefix("${CLAUDE_PLUGIN_ROOT}/")
                .unwrap_or_else(|| panic!("{at}: {script} is not under the plugin root"));
            let file = plugin_file(plugin, rel);
            assert!(file.is_file(), "{at}: {} does not exist", file.display());
            // The attention alert (crates/repo-tests/tests/notify.rs) is capped at 5 s.
            let timeout = if event == "Notification" { 5 } else { 60 };
            assert_eq!(
                get(hook, "timeout").and_then(Value::as_i64),
                Some(timeout),
                "{at}"
            );
            if event == "PreToolUse" {
                assert_eq!(str_of(hook, "onFailure"), "block", "{at}: fails closed");
            }
        }
    }
}

/// Claude Code looks a bare hook program up on the user's PATH, and hooks run in the folder of
/// the repository the user opened: with `.` or an empty entry on PATH that repository's own
/// `./sh` would replace the guard, and its `exit 0` would allow every tool call. So in every
/// plugin's hooks.json each command hook starts with an absolute path: the exec form's
/// `command` is the interpreter's absolute path, and a shell-form line starts with an absolute
/// path or the plugin root.
#[test]
fn every_hook_command_starts_with_an_absolute_path() {
    let mut seen = 0;
    let mut plugins: Vec<_> = std::fs::read_dir(repo::path("plugins"))
        .expect("plugins")
        .map(|e| e.expect("entry").path())
        .collect();
    plugins.sort();
    for dir in plugins {
        let file = dir.join("hooks/hooks.json");
        if !file.is_file() {
            continue;
        }
        let v = json::loads(&read(&file)).expect("hooks.json");
        for (event, hook) in hooks_of(&v) {
            if str_of(hook, "type") != "command" {
                continue;
            }
            let at = format!("{} {event}", file.display());
            let command = str_of(hook, "command");
            if get(hook, "args").is_some() {
                assert!(
                    command.starts_with('/'),
                    "{at}: `{command}` is not absolute"
                );
            } else {
                let first = command.trim_start().trim_start_matches(['"', '\'']);
                assert!(
                    first.starts_with('/') || first.starts_with("${CLAUDE_PLUGIN_ROOT}/"),
                    "{at}: `{command}` does not start with an absolute path"
                );
            }
            seen += 1;
        }
    }
    assert!(seen >= 8, "command hooks found: {seen}");
}

/// The anthology plugin's `modules` entry names its Claude Code mods module, a TypeScript file
/// beside hooks.json that ships with the plugin. Guardrails stays shell and binary only.
#[test]
fn only_the_anthology_plugin_names_a_shipped_mods_module() {
    for plugin in PLUGINS {
        let (_, v) = hooks_json(plugin);
        let modules: Vec<&str> = get(&v, "modules")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        if plugin != "anthology" {
            assert!(modules.is_empty(), "{plugin}: no mods module");
            continue;
        }
        assert_eq!(modules, ["./mods/index.ts"], "{plugin}");
        let file = plugin_file(plugin, "hooks/mods/index.ts");
        assert!(
            file.is_file(),
            "{plugin}: {} does not exist",
            file.display()
        );
    }
}

/// test_guard_cli `test_only_pretooluse_is_registered`: after the cutover the guardrails plugin
/// also bootstraps its binary at SessionStart (design §9.1); nothing else is registered. The
/// anthology plugin adds the `Notification` attention alert (crates/repo-tests/tests/notify.rs).
#[test]
fn the_hook_events_are_session_start_and_pre_tool_use() {
    for plugin in PLUGINS {
        let (_, v) = hooks_json(plugin);
        let names: Vec<&str> = events(&v).iter().map(|(k, _)| k).collect();
        let want: &[&str] = if plugin == "anthology" {
            &["SessionStart", "PreToolUse", "Notification"]
        } else {
            &["SessionStart", "PreToolUse"]
        };
        assert_eq!(names, want, "{plugin}");
    }
}

/// test_session_start `ShellWrapperTest` (F-3: no python3): no hook and no shim runs Python.
#[test]
fn no_hook_or_shim_runs_python() {
    for plugin in PLUGINS {
        let (_, v) = hooks_json(plugin);
        for (event, hook) in hooks_of(&v) {
            let line = json::dumps(hook, &PY_DEFAULT);
            assert!(
                !line.to_lowercase().contains("python"),
                "{plugin} {event}: {line}"
            );
        }
    }
    for (plugin, rel) in SHIMS {
        let text = read(&plugin_file(plugin, rel));
        for line in code_lines(&text) {
            assert!(
                !line.to_lowercase().contains("python"),
                "{plugin}/{rel}: {line}"
            );
        }
    }
}

/// test_guard_cli `test_no_machine_specific_paths`: the guard's sources and shims.
#[test]
fn the_guard_sources_have_no_machine_specific_paths() {
    let needle = format!("/{}{}/", "Us", "ers");
    let mut files: Vec<PathBuf> = SHIMS
        .iter()
        .filter(|(p, _)| *p == "anthology-guardrails")
        .map(|(p, rel)| plugin_file(p, rel))
        .collect();
    files.push(plugin_file("anthology-guardrails", "hooks/hooks.json"));
    files.push(repo::path("crates/repo-tests/tests/cutover_hooks.rs"));
    for dir in ["crates/anthology-guard", "crates/anthology-shell"] {
        let mut stack = vec![repo::path(&format!("{dir}/src"))];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap_or_else(|e| panic!("{}: {e}", d.display())) {
                let p = e.expect("entry").path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    files.push(p);
                }
            }
        }
    }
    for f in files {
        assert!(!read(&f).contains(&needle), "{}", f.display());
    }
}

/// test_guard_cli `test_python_files_declare_future_annotations`, the Rust counterpart: the
/// language-level header every guard source shares is the crate root's `forbid(unsafe_code)`.
#[test]
fn the_guard_crates_forbid_unsafe_code() {
    for root in [
        "crates/anthology-guard/src/lib.rs",
        "crates/anthology-shell/src/lib.rs",
        "crates/anthology-cli/src/main.rs",
    ] {
        assert!(
            repo::read(root).contains("#![forbid(unsafe_code)]"),
            "{root}"
        );
    }
}

/// test_session_start `ShellWrapperTest` after the cutover: the Python launchers are gone and the
/// live plugins run the shims.
#[test]
fn after_the_cutover_the_live_plugins_run_the_shims() {
    if !cut_over() {
        eprintln!("skipped: before the cutover (port/staging/ exists)");
        return;
    }
    for plugin in PLUGINS {
        let scripts = repo::path(&format!("plugins/{plugin}/scripts"));
        assert!(!scripts.exists(), "{} is deleted", scripts.display());
    }
    for (plugin, rel) in SHIMS {
        let live = repo::path(&format!("plugins/{plugin}/{rel}"));
        assert!(live.is_file(), "{} is in place", live.display());
    }
    let session = json::dumps(&hooks_json("anthology").1, &PY_DEFAULT);
    assert!(
        session.contains("${CLAUDE_PLUGIN_ROOT}/hooks/session-start.sh"),
        "{session}"
    );
}
