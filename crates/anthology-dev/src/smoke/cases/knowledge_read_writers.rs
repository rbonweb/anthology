//! `knowledge-read-writers` (U1): each `isolation: worktree` agent reads its Startup files in
//! order before its first Glob (`plugin eval` cannot run these: it refuses worktree isolation).
//! Preload agents must not call Skill and quote the heading.

use crate::smoke::runner::*;

pub fn run(ctx: &Ctx) -> Res<Outcome> {
    let root = ctx.root()?;
    let agents = load_agents(&root.join("plugins/anthology/agents"))?;
    let writers: Vec<&(String, AgentSpec)> = agents.iter().filter(|(_, v)| v.worktree).collect();
    if writers.is_empty() {
        return Ok((
            Status::Fail,
            format!(
                "no isolation: worktree agent found among {} agent file(s)",
                agents.len()
            ),
        ));
    }
    let empty: Vec<&str> = writers
        .iter()
        .filter(|(_, v)| v.startup.is_empty())
        .map(|(a, _)| a.as_str())
        .collect();
    if !empty.is_empty() {
        return Ok((
            Status::Fail,
            format!("no Startup paths parsed for: {}", empty.join(", ")),
        ));
    }
    let mut bad: Vec<String> = Vec::new();
    for (agent, spec) in &writers {
        let headings = spec
            .preloads
            .iter()
            .map(|p| skill_heading(&root, p))
            .collect::<Result<Vec<String>, String>>()?;
        let ending = if headings.is_empty() {
            "reply DONE."
        } else {
            "reply DONE followed by the first heading of each skill preloaded into your context."
        };
        let prompt = format!(
            "Use the Agent tool to run the anthology:{agent} agent with exactly this prompt: 'Startup check only: \
             do your Startup reading, then use Glob once to list *.md files in the current folder, then {ending}' \
             Then quote its reply."
        );
        let tmp = scratch()?;
        // The agent may run in a worktree of this repo; that is expected, not a failure.
        let repo = ctx.make_repo(&tmp)?;
        let mut flags = sv(["--model", "sonnet"]);
        flags.extend(ctx.plugin_flags()?);
        flags.extend(["--settings".to_string(), dumps(&ctx.read_allow()?)]);
        // Scrub off: on Linux the scrub stops Claude Code creating the agent's worktree.
        let opts = PromptOpts {
            cwd: Some(repo),
            variadic: sv(["Agent", "Read", "Glob"]),
            extra_env: Some(no_scrub()),
        };
        let p = ctx.prompt_run(&tmp, &prompt, &flags, &opts)?;
        let files = subagent_files(&tmp.join("cfg"));
        if files.is_empty() {
            bad.push(format!(
                "{agent}: no subagent transcript (exit {}: {})",
                p.code,
                ctx.snippet(&p.text(), 80)
            ));
            continue;
        }
        let mut calls = Vec::new();
        for f in &files {
            calls.extend(transcript_calls(&read_lossy(f)?));
        }
        let mut problems = startup_problems(&calls, &spec.startup)?;
        if !headings.is_empty() {
            if calls.iter().any(|(n, _)| n == "Skill") {
                problems.push("called Skill for a preload".to_string());
            }
            problems.extend(
                headings
                    .iter()
                    .filter(|h| !p.out.contains(h.as_str()))
                    .map(|h| format!("reply lacks the heading {}", py_repr_str(h))),
            );
        }
        if !problems.is_empty() {
            bad.push(format!("{agent}: {}", problems.join(", ")));
        }
    }
    Ok(if bad.is_empty() {
        (
            Status::Pass,
            format!(
                "all {} worktree agents read their Startup files in order",
                writers.len()
            ),
        )
    } else {
        (Status::Fail, bad.join("; "))
    })
}
