//! `anthology-dev smoke --prepare-evals DIR` (EV1b): the workspace `claude plugin eval` runs on
//! in `run-evals.sh`, with the staged plugins and the seeded `anthology` binary.
//!
//! `DIR` (new or empty) gets `.claude-plugin/` and `evals/` from the checkout (without
//! `evals/results/` and `evals/smoke/`) and the staged plugin copy under `plugins/`, so every
//! case's `../../plugins/<p>` resolves to the copy. `anthology-dev seed` pins the binary in both
//! locks and packs its archive.
//!
//! Probe U10: `plugin eval` ignores the caller's `CLAUDE_CONFIG_DIR` and `HOME` and gives every
//! run a fresh `/tmp/e-XXXXXX/config`, so a seeded config dir is never seen. The archive is
//! therefore carried in the plugin copy (`<plugin>/eval-seed/`), and each SessionStart hook of
//! anthology and anthology-guardrails runs through `eval-seed/eval-seed.sh`, which copies it to
//! `<config>/anthology/bin/` (the bootstrap's offline seed, verified against the lock like any
//! seed) before it runs the hook unchanged. A guard call that comes first installs from the
//! same seed. Nothing here is shipped: only the scratch copy is changed.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anthology_pycompat::json::{self, Value};

use super::runner::{Ctx, copy_tree, jstr, mkdirs, write_text};

/// The wrapper every SessionStart hook of the copy runs through.
pub const EVAL_SEED_SH: &str = r#"#!/bin/sh
# Evals only: written by `anthology-dev smoke --prepare-evals` into a scratch copy of the
# plugin, never shipped. `claude plugin eval` gives hooks a fresh config dir (probe U10), so the
# seeded archive beside this script is copied to where the bootstrap looks for its offline seed
# (<config>/anthology/bin/anthology-<target>.tar.gz; checked against anthology.lock like any
# seed). Then the wrapped SessionStart hook runs unchanged, with its own arguments.
seed_dir=${0%/*}
if [ -n "${CLAUDE_CONFIG_DIR:-}" ]; then
    bin_dir=$CLAUDE_CONFIG_DIR/anthology/bin
else
    bin_dir=${HOME:-}/.claude/anthology/bin
fi
if mkdir -p "$bin_dir" 2>/dev/null; then
    for f in "$seed_dir"/anthology-*.tar.gz; do
        [ -f "$f" ] || continue
        tmp=$bin_dir/.evalseed.$$
        if cp "$f" "$tmp" 2>/dev/null; then
            mv -f "$tmp" "$bin_dir/${f##*/}" 2>/dev/null || rm -f "$tmp"
        fi
    done
fi
exec sh "$@"
"#;

/// The wrapper's path as a hook argument.
const WRAPPER_ARG: &str = "${CLAUDE_PLUGIN_ROOT}/eval-seed/eval-seed.sh";

fn os(p: &Path, e: std::io::Error) -> String {
    format!("OSError: {}: {e}", p.display())
}

/// Puts `eval-seed.sh` in front of every SessionStart command hook of `hooks_json`
/// (`sh <script> ARGS` becomes `sh eval-seed.sh <script> ARGS`). Every such hook must be an
/// `sh` command with `args`, as the staged hooks are; anything else is an error.
pub fn wrap_session_start(text: &str) -> Result<String, String> {
    let mut data = json::loads(text).map_err(|e| format!("ValueError: hooks.json: {}", e.msg))?;
    let groups = data
        .as_object_mut()
        .and_then(|o| o.get_mut("hooks"))
        .and_then(Value::as_object_mut)
        .and_then(|h| h.get_mut("SessionStart"))
        .and_then(Value::as_array_mut)
        .ok_or("RuntimeError: hooks.json has no SessionStart list")?;
    let mut wrapped = 0;
    for group in groups.iter_mut() {
        let Some(hooks) = group
            .as_object_mut()
            .and_then(|g| g.get_mut("hooks"))
            .and_then(Value::as_array_mut)
        else {
            continue;
        };
        for hook in hooks.iter_mut() {
            let Some(obj) = hook.as_object_mut() else {
                continue;
            };
            if obj.get("type").and_then(Value::as_str) != Some("command") {
                continue;
            }
            if obj.get("command").and_then(Value::as_str) != Some("sh") {
                return Err(
                    "RuntimeError: a SessionStart hook is not an `sh` command; cannot wrap it"
                        .to_string(),
                );
            }
            let args = obj
                .get_mut("args")
                .and_then(Value::as_array_mut)
                .ok_or("RuntimeError: a SessionStart `sh` hook has no args list")?;
            args.insert(0, jstr(WRAPPER_ARG));
            wrapped += 1;
        }
    }
    if wrapped == 0 {
        return Err("RuntimeError: hooks.json has no SessionStart command hook".to_string());
    }
    let mut out = json::dumps(&data, &json::INDENT2);
    out.push('\n');
    Ok(out)
}

/// Builds the eval workspace in `dir` and returns it.
pub fn prepare(ctx: &Ctx, dir: &Path) -> Result<PathBuf, String> {
    if fs::read_dir(dir).is_ok_and(|mut it| it.next().is_some()) {
        return Err(format!("{} is not empty", dir.display()));
    }
    let root = ctx.root()?;
    let bin = ctx.anthology_bin()?;
    let staged = ctx.staged_plugins()?;
    mkdirs(dir)?;
    copy_tree(&root.join(".claude-plugin"), &dir.join(".claude-plugin"))?;
    // evals/ without results/ (earlier runs' output) and smoke/ (smoke.py).
    let evals = root.join("evals");
    mkdirs(&dir.join("evals"))?;
    for entry in fs::read_dir(&evals).map_err(|e| os(&evals, e))? {
        let entry = entry.map_err(|e| os(&evals, e))?;
        let name = entry.file_name();
        if name == "results" || name == "smoke" {
            continue;
        }
        copy_tree(&entry.path(), &dir.join("evals").join(&name))?;
    }
    let plugins = dir.join("plugins");
    copy_tree(&staged, &plugins)?;
    let seed_cfg = dir.join(".seed-config");
    (ctx.deps.seed)(&bin, &plugins, &seed_cfg)?;
    let bin_dir = seed_cfg.join("anthology").join("bin");
    let archives: Vec<PathBuf> = fs::read_dir(&bin_dir)
        .map_err(|e| os(&bin_dir, e))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name().is_some_and(|n| {
                let n = n.to_string_lossy();
                n.starts_with("anthology-") && n.ends_with(".tar.gz")
            })
        })
        .collect();
    let [archive] = archives.as_slice() else {
        return Err(format!(
            "RuntimeError: seed left {} archives in {}, want 1",
            archives.len(),
            bin_dir.display()
        ));
    };
    let name = archive.file_name().unwrap_or_default();
    for p in ["anthology", "anthology-guardrails"] {
        let plugin = plugins.join(p);
        let seed_dir = plugin.join("eval-seed");
        mkdirs(&seed_dir)?;
        fs::copy(archive, seed_dir.join(name)).map_err(|e| os(&seed_dir, e))?;
        let script = seed_dir.join("eval-seed.sh");
        write_text(&script, EVAL_SEED_SH)?;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755))
            .map_err(|e| os(&script, e))?;
        let hooks = plugin.join("hooks").join("hooks.json");
        let text = fs::read_to_string(&hooks).map_err(|e| os(&hooks, e))?;
        write_text(&hooks, &wrap_session_start(&text)?)?;
    }
    fs::remove_dir_all(&seed_cfg).map_err(|e| os(&seed_cfg, e))?;
    Ok(dir.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_start_hooks_are_wrapped_and_the_rest_is_kept() {
        let text = r#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"sh","args":["${CLAUDE_PLUGIN_ROOT}/hooks/session-start.sh"],"timeout":60}]}],"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"sh","args":["x"]}]}]}}"#;
        let out = wrap_session_start(text).expect("wraps");
        let v = json::loads(&out).expect("json");
        let ss = &v.as_object().expect("obj").get("hooks").expect("hooks");
        let args = |event: &str| {
            ss.as_object()
                .expect("obj")
                .get(event)
                .expect("event")
                .as_array()
                .expect("arr")[0]
                .as_object()
                .expect("group")
                .get("hooks")
                .expect("hooks")
                .as_array()
                .expect("arr")[0]
                .as_object()
                .expect("hook")
                .get("args")
                .expect("args")
                .as_array()
                .expect("arr")
                .iter()
                .map(|a| a.as_str().expect("str").to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            args("SessionStart"),
            [WRAPPER_ARG, "${CLAUDE_PLUGIN_ROOT}/hooks/session-start.sh"]
        );
        assert_eq!(args("PreToolUse"), ["x"]);
        assert!(out.ends_with("}\n"));
    }

    #[test]
    fn an_unwrappable_session_start_is_refused() {
        assert!(wrap_session_start(r#"{"hooks":{}}"#).is_err());
        assert!(
            wrap_session_start(
                r#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"python3","args":[]}]}]}}"#
            )
            .is_err()
        );
        assert!(
            wrap_session_start(
                r#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"sh $X"}]}]}}"#
            )
            .is_err()
        );
    }

    #[test]
    fn the_wrapper_seeds_then_runs_the_hook_with_its_arguments() {
        let tmp = tempfile::tempdir().expect("tmp");
        let seed_dir = tmp.path().join("plugin/eval-seed");
        fs::create_dir_all(&seed_dir).expect("mkdir");
        fs::write(seed_dir.join("eval-seed.sh"), EVAL_SEED_SH).expect("write");
        fs::write(seed_dir.join("anthology-x-y.tar.gz"), "archive").expect("write");
        let hook = tmp.path().join("hook.sh");
        fs::write(
            &hook,
            "printf 'ran %s %s from %s\\n' \"$1\" \"$2\" \"$0\"\n",
        )
        .expect("write");
        for (config, home) in [(Some("cfg"), "home1"), (None, "home2")] {
            let mut cmd = std::process::Command::new("/bin/sh");
            cmd.arg(seed_dir.join("eval-seed.sh"))
                .arg(&hook)
                .args(["session", "--plugin"])
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env("HOME", tmp.path().join(home));
            if let Some(c) = config {
                cmd.env("CLAUDE_CONFIG_DIR", tmp.path().join(c));
            }
            let out = cmd.output().expect("runs");
            assert!(out.status.success());
            assert_eq!(
                String::from_utf8_lossy(&out.stdout),
                format!("ran session --plugin from {}\n", hook.display())
            );
            let seeded = match config {
                Some(c) => tmp.path().join(c),
                None => tmp.path().join(home).join(".claude"),
            }
            .join("anthology/bin/anthology-x-y.tar.gz");
            assert_eq!(fs::read_to_string(seeded).expect("seeded"), "archive");
        }
    }
}
