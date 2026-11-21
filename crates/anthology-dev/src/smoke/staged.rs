//! Helpers of the EV1b smokes that run the staged shims directly, without `claude`: the hook
//! commands a hooks.json declares, the hook envelope, a narrowed `PATH`, and the environment a
//! hook process gets.

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use anthology_pycompat::json::{self, Value};

use super::compat::shlex_split;
use super::runner::{EnvMap, field, jobj, jstr, py_str};

/// The argv of every `command` hook of `event` in a plugin's hooks.json, with
/// `${CLAUDE_PLUGIN_ROOT}` replaced by `plugin_root` (as Claude Code expands it).
pub fn hook_commands(plugin_root: &Path, event: &str) -> Result<Vec<Vec<String>>, String> {
    let path = plugin_root.join("hooks").join("hooks.json");
    let text =
        fs::read_to_string(&path).map_err(|e| format!("OSError: {}: {e}", path.display()))?;
    let data =
        json::loads(&text).map_err(|e| format!("ValueError: {}: {}", path.display(), e.msg))?;
    let root = plugin_root.to_string_lossy();
    let expand = |s: &str| {
        s.replace("${CLAUDE_PLUGIN_ROOT}", &root)
            .replace("$CLAUDE_PLUGIN_ROOT", &root)
    };
    let mut out = Vec::new();
    let groups = field(&data, "hooks")
        .and_then(|h| field(h, event))
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    for group in groups {
        let hooks = field(group, "hooks")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        for hook in hooks {
            if field(hook, "type").and_then(Value::as_str) != Some("command") {
                continue;
            }
            let Some(cmd) = field(hook, "command").and_then(Value::as_str) else {
                continue;
            };
            let argv: Vec<String> = match field(hook, "args").and_then(Value::as_array) {
                Some(args) => std::iter::once(cmd.to_string())
                    .chain(args.iter().map(py_str))
                    .collect(),
                None => shlex_split(cmd).map_err(|e| format!("ValueError: {e}"))?,
            };
            out.push(argv.iter().map(|a| expand(a)).collect());
        }
    }
    Ok(out)
}

/// The one hook of `event` whose argv names `script`, or an error naming what is there.
pub fn hook_command(plugin_root: &Path, event: &str, script: &str) -> Result<Vec<String>, String> {
    let all = hook_commands(plugin_root, event)?;
    all.iter()
        .find(|argv| argv.iter().any(|a| a.ends_with(script)))
        .cloned()
        .ok_or_else(|| {
            format!(
                "RuntimeError: {}: no {event} hook runs {script} (found {all:?})",
                plugin_root.display()
            )
        })
}

/// A PreToolUse hook envelope, as Claude Code writes it on the hook's stdin.
pub fn envelope(tool_name: &str, tool_input: Value, cwd: &Path) -> String {
    json::dumps(
        &jobj(vec![
            ("session_id", jstr("smoke")),
            ("cwd", jstr(&cwd.to_string_lossy())),
            ("hook_event_name", jstr("PreToolUse")),
            ("tool_name", jstr(tool_name)),
            ("tool_input", tool_input),
        ]),
        &json::PY_DEFAULT,
    )
}

/// A Bash envelope for `command`.
pub fn bash(command: &str, cwd: &Path) -> String {
    envelope("Bash", jobj(vec![("command", jstr(command))]), cwd)
}

/// The downloaders the bootstrap knows (GC-17 sources), and BusyBox, which has `wget`.
pub const DOWNLOADERS: [&str; 5] = ["curl", "wget", "node", "nodejs", "busybox"];

/// Fills `bin` with a link to the first executable of each name found on `path` (the caller's
/// `PATH`), leaving out `skip`: a `PATH` with every tool but those.
pub fn narrowed_path(path: &str, bin: &Path, skip: &[&str]) -> Result<(), String> {
    fs::create_dir_all(bin).map_err(|e| format!("OSError: {}: {e}", bin.display()))?;
    for dir in path.split(':').filter(|d| d.starts_with('/')) {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(text) = name.to_str() else {
                continue;
            };
            if skip.contains(&text) || text.starts_with('.') {
                continue;
            }
            let link = bin.join(&name);
            if fs::symlink_metadata(&link).is_ok() {
                continue;
            }
            let target = entry.path();
            let executable = fs::metadata(&target)
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0);
            if executable {
                symlink(&target, &link).map_err(|e| format!("OSError: {}: {e}", link.display()))?;
            }
        }
    }
    Ok(())
}

/// The environment of a hook or wrapper process: nothing but these.
pub fn hook_env(
    path: &str,
    home: &Path,
    config: &Path,
    tmpdir: &Path,
    plugin_root: &Path,
    project: &Path,
) -> EnvMap {
    let s = |p: &Path| p.to_string_lossy().into_owned();
    EnvMap::from([
        ("PATH".to_string(), path.to_string()),
        ("HOME".to_string(), s(home)),
        ("CLAUDE_CONFIG_DIR".to_string(), s(config)),
        ("TMPDIR".to_string(), s(tmpdir)),
        ("LANG".to_string(), "C.UTF-8".to_string()),
        ("CLAUDE_PLUGIN_ROOT".to_string(), s(plugin_root)),
        ("CLAUDE_PROJECT_DIR".to_string(), s(project)),
    ])
}

/// The `<config>/anthology/bin` entries whose name starts with `.broken-` (quarantined binaries).
pub fn quarantined(config: &Path) -> Vec<PathBuf> {
    let dir = config.join("anthology").join("bin");
    let mut out: Vec<PathBuf> = fs::read_dir(dir)
        .map(|it| {
            it.flatten()
                .filter(|e| e.file_name().to_string_lossy().starts_with(".broken-"))
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

/// The one JSON object a SessionStart hook printed, and its `systemMessage`.
pub fn system_message(stdout: &str) -> Option<String> {
    let value = json::loads(stdout.trim_end_matches('\n')).ok()?;
    field(&value, "systemMessage")
        .and_then(Value::as_str)
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_commands_expand_the_plugin_root_and_keep_args() {
        let tmp = tempfile::tempdir().expect("tmp");
        fs::create_dir_all(tmp.path().join("hooks")).expect("mkdir");
        fs::write(
            tmp.path().join("hooks/hooks.json"),
            r#"{"hooks":{"PreToolUse":[{"matcher":"*","hooks":[
                {"type":"command","command":"sh","args":["${CLAUDE_PLUGIN_ROOT}/hooks/guard.sh"]},
                {"type":"command","command":"sh $CLAUDE_PLUGIN_ROOT/x.sh --q"},
                {"type":"prompt","prompt":"p"}]}]}}"#,
        )
        .expect("write");
        let root = tmp.path().to_string_lossy().into_owned();
        let all = hook_commands(tmp.path(), "PreToolUse").expect("read");
        assert_eq!(
            all,
            [
                vec!["sh".to_string(), format!("{root}/hooks/guard.sh")],
                vec!["sh".to_string(), format!("{root}/x.sh"), "--q".to_string()],
            ]
        );
        assert_eq!(
            hook_command(tmp.path(), "PreToolUse", "guard.sh").expect("found")[1],
            format!("{root}/hooks/guard.sh")
        );
        assert!(hook_command(tmp.path(), "SessionStart", "guard.sh").is_err());
    }

    #[test]
    fn the_narrowed_path_leaves_out_the_skipped_names() {
        let tmp = tempfile::tempdir().expect("tmp");
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        for (dir, name) in [(&a, "curl"), (&a, "tool"), (&b, "tool"), (&b, "other")] {
            fs::create_dir_all(dir).expect("mkdir");
            let f = dir.join(name);
            fs::write(&f, "#!/bin/sh\n").expect("write");
            fs::set_permissions(&f, fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        fs::write(b.join("data"), "x").expect("write");
        let bin = tmp.path().join("bin");
        let path = format!("{}:{}:relative", a.display(), b.display());
        narrowed_path(&path, &bin, &DOWNLOADERS).expect("narrow");
        let mut names: Vec<String> = fs::read_dir(&bin)
            .expect("list")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["other", "tool"]);
        assert_eq!(
            fs::read_link(bin.join("tool")).expect("link"),
            a.join("tool")
        );
    }

    #[test]
    fn a_system_message_is_read_from_one_json_line() {
        assert_eq!(
            system_message("{\"systemMessage\":\"hi\"}\n").as_deref(),
            Some("hi")
        );
        assert_eq!(system_message("nope"), None);
        assert_eq!(system_message("{\"x\":1}"), None);
    }
}
