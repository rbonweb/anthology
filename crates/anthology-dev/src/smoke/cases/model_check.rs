//! `model-check` (U2): each agent's subagent transcript carries the model family its frontmatter names.

use std::collections::BTreeSet;

use crate::smoke::runner::*;

pub fn run(ctx: &Ctx) -> Res<Outcome> {
    let root = ctx.root()?;
    let roster: Vec<(String, String)> = load_agents(&root.join("plugins/anthology/agents"))?
        .into_iter()
        .map(|(name, spec)| (name, spec.model))
        .collect();
    if roster.len() != EXPECTED_AGENTS {
        let mut names: Vec<&str> = roster.iter().map(|(n, _)| n.as_str()).collect();
        names.sort_unstable();
        return Ok((
            Status::Fail,
            format!(
                "found {} agents, want {EXPECTED_AGENTS}: {}",
                roster.len(),
                names.join(", ")
            ),
        ));
    }
    let mut bad: Vec<String> = Vec::new();
    for (agent, family) in &roster {
        let tmp = scratch()?;
        let repo = ctx.make_repo(&tmp)?;
        let prompt = format!(
            "Use the Agent tool to run the anthology:{agent} agent with the prompt 'Reply OK.' and quote its reply."
        );
        // Never the agent's own model.
        let other = if family == "haiku" { "sonnet" } else { "haiku" };
        let mut flags = sv(["--model", other]);
        flags.extend(ctx.plugin_flags()?);
        flags.extend(["--settings".to_string(), dumps(&ctx.read_allow()?)]);
        flags.extend(sv(["--max-turns", "4"]));
        // Scrub off: on Linux the scrub stops Claude Code creating the worktree of an
        // `isolation: worktree` agent.
        let opts = PromptOpts {
            cwd: Some(repo),
            variadic: sv(["Agent", "Read"]),
            extra_env: Some(no_scrub()),
        };
        let p = ctx.prompt_run(&tmp, &prompt, &flags, &opts)?;
        let (models, files) = transcript_models(&tmp.join("cfg"))?;
        if files.is_empty() {
            let dir = tmp
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            bad.push(format!(
                "{agent}: no transcript under {dir}/cfg/projects/*/*/subagents/ (exit {}: {})",
                p.code,
                ctx.snippet(&p.text(), 80)
            ));
        } else if !models.iter().any(|m| m.contains(family.as_str())) {
            let distinct: BTreeSet<&String> = models.iter().collect();
            let shown: Vec<String> = distinct.iter().map(|m| py_repr_str(m)).collect();
            bad.push(format!(
                "{agent}: wanted {family}, saw [{}]",
                shown.join(", ")
            ));
        }
    }
    Ok(if bad.is_empty() {
        (
            Status::Pass,
            format!("all {} agents ran on their frontmatter model", roster.len()),
        )
    } else {
        (Status::Fail, bad.join("; "))
    })
}
