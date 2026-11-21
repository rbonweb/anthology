//! `cli-syntax` (U4): the plugin subcommands and flags the plugins and evals rely on exist (no model).

use std::fs;

use anthology_pycompat::json;

use crate::smoke::runner::*;

/// Each subcommand with the flags its `--help` must show, in smoke.py's order.
const FLAGS: [(&[&str], &[&str]); 10] = [
    (&["marketplace", "add"], &["--scope"]),
    (&["install"], &["-s"]),
    (&["uninstall"], &["--keep-data"]),
    (&["validate"], &["--strict"]),
    (&["tag"], &["--dry-run", "--push"]),
    (
        &["eval"],
        &[
            "--max-cost-usd",
            "--trust-plugin",
            "--ablation",
            "--no-publish",
            "--json",
            "--allow-tools",
            "--tag",
            "--threshold",
            "--output-dir",
            "--runs",
        ],
    ),
    (&["list"], &[]),
    (&["marketplace", "list"], &[]),
    (&["marketplace", "remove"], &[]),
    (&["update"], &[]),
];

pub fn run(ctx: &Ctx) -> Res<Outcome> {
    let root = ctx.root()?;
    let mut problems: Vec<String> = Vec::new();
    let tmp = scratch()?;
    for (sub, wanted) in FLAGS {
        let name = format!("plugin {}", sub.join(" "));
        let mut args = sv(["plugin"]);
        args.extend(sub.iter().map(|s| (*s).to_string()));
        args.push("--help".to_string());
        let p = ctx.claude_run(&tmp, &args, &ClaudeOpts::at(&root, 60));
        if p.code != 0 {
            problems.push(format!("{name} --help exited {}", p.code));
            continue;
        }
        let help = format!("{}{}", p.out, p.err);
        for flag in wanted {
            if !has_flag(&help, flag)? {
                problems.push(format!("{name} lacks {flag}"));
            }
        }
    }
    for plugin in PLUGINS {
        let plugin_dir = root.join("plugins").join(plugin);
        let args = sv([
            "plugin",
            "validate",
            "--strict",
            &plugin_dir.to_string_lossy(),
        ]);
        let p = ctx.claude_run(&tmp, &args, &ClaudeOpts::at(&root, 120));
        if p.code != 0 {
            problems.push(format!(
                "validate --strict {plugin} exited {}: {}",
                p.code,
                first_problem(&ctx.env, &p.text())
            ));
        }
    }
    let manifest = root.join("plugins/anthology/.claude-plugin/plugin.json");
    let text = fs::read_to_string(&manifest)
        .map_err(|e| format!("OSError: {}: {e}", manifest.display()))?;
    let parsed = json::loads(&text).map_err(|e| format!("ValueError: {}", e.msg))?;
    let version = field(&parsed, "version")
        .map(py_str)
        .ok_or_else(|| "KeyError: 'version'".to_string())?;
    let anthology = root.join("plugins/anthology");
    let args = sv(["plugin", "tag", "--dry-run", &anthology.to_string_lossy()]);
    let p = ctx.claude_run(&tmp, &args, &ClaudeOpts::at(&root, 60));
    if !p.text().contains(&format!("anthology--v{version}")) {
        problems.push(format!(
            "tag --dry-run does not name anthology--v{version}: {}",
            ctx.snippet(&p.text(), 100)
        ));
    }
    Ok(if problems.is_empty() {
        (
            Status::Pass,
            "all plugin subcommands, flags, validate and tag behave".to_string(),
        )
    } else {
        (Status::Fail, problems.join("; "))
    })
}
