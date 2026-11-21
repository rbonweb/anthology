//! `python-missing` (U8, no model): the sh SessionStart wrapper prints a systemMessage naming
//! python3 when python3 is missing. The guardrails sentinel (PLAN U8) must also print nothing
//! when python3 is present. Keeps the Python-era semantics until CO2; once CO2 has deleted the
//! Python plugin scripts it skips, and `binary-missing` checks the missing-runtime messages.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anthology_pycompat::json;
use regex::{NoExpand, Regex};

use crate::smoke::compat::strip_ws;
use crate::smoke::runner::*;

static ROOT_VAR_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\$\{?CLAUDE_PLUGIN_ROOT\}?").expect("ROOT_VAR_RE is a fixed pattern")
});

const TOOLS: [&str; 11] = [
    "sh", "cat", "printf", "env", "dirname", "basename", "head", "tr", "sed", "mkdir", "uname",
];

/// `shutil.which(tool)` against the PATH of the smoke's environment.
fn which(ctx: &Ctx, tool: &str) -> Option<PathBuf> {
    ctx.env
        .get("PATH")
        .unwrap_or("")
        .split(':')
        .map(|dir| Path::new(dir).join(tool))
        .find(|p| {
            use std::os::unix::fs::PermissionsExt;
            fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

/// `msg = json.loads(out.strip())["systemMessage"]`, ok when it is a non-blank string naming python3.
fn names_python3(out: &str) -> bool {
    json::loads(strip_ws(out))
        .ok()
        .and_then(|v| field(&v, "systemMessage").and_then(|m| m.as_str().map(str::to_string)))
        .is_some_and(|msg| !strip_ws(&msg).is_empty() && msg.contains("python3"))
}

pub fn run(ctx: &Ctx) -> Res<Outcome> {
    let root = ctx.root()?;
    if !root.join("plugins/anthology/scripts").is_dir() {
        return Ok((
            Status::Skip,
            "no Python plugin scripts (after CO2); binary-missing covers the missing runtime"
                .to_string(),
        ));
    }
    let mut cmds: Vec<(PathBuf, Vec<String>)> = Vec::new();
    for plugin in ["anthology", "anthology-guardrails"] {
        let plugin_root = root.join("plugins").join(plugin);
        for argv in sh_session_start_commands(&plugin_root.join("hooks").join("hooks.json"))? {
            cmds.push((plugin_root.clone(), argv));
        }
    }
    if cmds.is_empty() {
        return Ok((
            Status::Skip,
            "no sh SessionStart wrapper yet (stream F)".to_string(),
        ));
    }
    let mut problems: Vec<String> = Vec::new();
    let tmp = scratch()?;
    let mut bins: Vec<(&str, PathBuf)> = Vec::new();
    for (label, extra) in [("missing", None), ("present", Some("python3"))] {
        let bin = tmp.join(format!("bin-{label}"));
        mkdirs(&bin)?;
        for tool in TOOLS.iter().copied().chain(extra) {
            // Resolved before PATH is narrowed.
            if let Some(found) = which(ctx, tool) {
                symlink(&found, bin.join(tool))
                    .map_err(|e| format!("OSError: {}: {e}", bin.join(tool).display()))?;
            }
        }
        bins.push((label, bin));
    }
    let bin_of = |label: &str| {
        bins.iter()
            .find(|(l, _)| *l == label)
            .map(|(_, b)| b.clone())
    };
    let wrapper = |plugin_root: &Path, argv: &[String], label: &str| -> Proc {
        let root_text = plugin_root.to_string_lossy().into_owned();
        let argv: Vec<String> = argv
            .iter()
            .map(|a| {
                ROOT_VAR_RE
                    .replace_all(a, NoExpand(&root_text))
                    .into_owned()
            })
            .collect();
        let env = EnvMap::from([
            (
                "PATH".to_string(),
                bin_of(label)
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
            ),
            ("HOME".to_string(), tmp.to_string_lossy().into_owned()),
            ("LANG".to_string(), "C.UTF-8".to_string()),
            ("CLAUDE_PLUGIN_ROOT".to_string(), root_text),
            (
                "CLAUDE_PROJECT_DIR".to_string(),
                tmp.to_string_lossy().into_owned(),
            ),
        ]);
        ctx.run(&argv, &tmp, &env, 60, Some("{}"))
    };
    for (plugin_root, argv) in &cmds {
        let name = plugin_root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let p = wrapper(plugin_root, argv, "missing");
        if p.code != 0 || !names_python3(&p.out) {
            let last = argv.last().map(String::as_str).unwrap_or("");
            let last = Path::new(last)
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            problems.push(format!(
                "{name}: {last} must exit 0 and print one JSON object whose systemMessage mentions python3 (exit {}): {}",
                p.code,
                ctx.snippet(&p.text(), 100)
            ));
        }
        if name == "anthology-guardrails"
            && bin_of("present").is_some_and(|b| b.join("python3").exists())
        {
            let present = wrapper(plugin_root, argv, "present");
            if !strip_ws(&present.out).is_empty() {
                problems.push(format!(
                    "{name}: prints output although python3 is present: {}",
                    ctx.snippet(&present.out, 80)
                ));
            }
        }
    }
    Ok(if problems.is_empty() {
        (
            Status::Pass,
            format!("{} wrapper(s) warn about the missing python3", cmds.len()),
        )
    } else {
        (Status::Fail, problems.join("; "))
    })
}
