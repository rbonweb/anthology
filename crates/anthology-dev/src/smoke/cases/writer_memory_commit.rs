//! `writer-memory-commit` (U7): a writer agent commits its memory note in its worktree under
//! `chore(memory)`, at the right path.

use crate::smoke::compat::{split_ws, splitlines};
use crate::smoke::runner::*;

pub fn run(ctx: &Ctx) -> Res<Outcome> {
    let prompt = "Use the Agent tool to run the anthology:implementer agent with the prompt 'Save a project memory note named \
project-smoke-probe.md saying the smoke probe is teal because this smoke checks writer memory commits; follow \
your Memory rules, then finish.' Then report its reply.";
    let want = ".claude/agent-memory/anthology-implementer/project-smoke-probe.md";
    // bypassPermissions: cwd/HOME/config in this temp dir (not a sandbox), guardrails loaded.
    let tmp = scratch()?;
    let repo = ctx.make_repo(&tmp)?;
    let mut flags = sv([
        "--permission-mode",
        "bypassPermissions",
        "--model",
        "sonnet",
    ]);
    flags.extend(ctx.plugin_flags()?);
    let settings = jobj(vec![("worktree", jobj(vec![("baseRef", jstr("head"))]))]);
    flags.extend(["--settings".to_string(), dumps(&settings)]);
    let opts = PromptOpts {
        cwd: Some(repo.clone()),
        extra_env: Some(no_scrub()),
        ..PromptOpts::default()
    };
    let p = ctx.prompt_run(&tmp, prompt, &flags, &opts)?;
    let mut found = false;
    let branches = ctx.git(
        &tmp,
        &repo,
        &[
            "branch",
            "--list",
            "worktree-agent-*",
            "--format=%(refname:short)",
        ],
    )?;
    for branch in split_ws(&branches) {
        let shas = ctx.git(&tmp, &repo, &["rev-list", branch, "--not", "main"])?;
        for sha in split_ws(&shas) {
            let subject = ctx.git(&tmp, &repo, &["log", "-1", "--format=%s", sha])?;
            let added = ctx.git(
                &tmp,
                &repo,
                &["diff-tree", "--no-commit-id", "--name-status", "-r", sha],
            )?;
            found = found
                || (subject.starts_with("chore(memory)")
                    && splitlines(&added).contains(&format!("A\t{want}").as_str()));
        }
    }
    let stray = colon_memory_paths(&tmp);
    if let Some(first) = stray.first() {
        return Ok((
            Status::Fail,
            format!("a note landed in a path with the agent type's colon: {first}"),
        ));
    }
    if !found {
        return Ok((
            Status::Fail,
            format!(
                "no worktree-agent-* branch has a chore(memory) commit adding {want} (exit {}: {})",
                p.code,
                ctx.snippet(&p.text(), 80)
            ),
        ));
    }
    Ok((
        Status::Pass,
        "chore(memory) commit adds the note in the agent's worktree branch".to_string(),
    ))
}
