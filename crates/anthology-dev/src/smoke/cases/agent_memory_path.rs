//! `agent-memory-path` (U7): `anthology:scout` saves its memory note through `anthology-mem save`
//! into its own agent-memory folder. (`claude plugin eval` cannot run this: the plugin bin/ is not
//! on its Bash PATH.)

use std::fs;

use crate::smoke::runner::*;

pub fn run(ctx: &Ctx) -> Res<Outcome> {
    let prompt = "Use the Agent tool to run the anthology:scout agent with exactly this prompt: 'Save one project memory \
note named project-eval-probe, following your Memory rules exactly: its description says the eval probe \
colour is teal, and its body says why: this eval checks where agent memory is saved. Then reply DONE.' \
Do not use any tool yourself other than Agent. Then report the agent's reply.";
    let tmp = scratch()?;
    let repo = ctx.make_repo(&tmp)?;
    let mut flags = sv(["--model", "sonnet"]);
    flags.extend(ctx.plugin_flags()?);
    flags.extend(["--settings".to_string(), dumps(&ctx.read_allow()?)]);
    flags.extend(sv([
        "--max-turns",
        "12",
        "--output-format",
        "stream-json",
        "--verbose",
    ]));
    let opts = PromptOpts {
        cwd: Some(repo.clone()),
        variadic: sv([
            "Agent",
            "Read",
            "Glob",
            "Grep",
            "Bash(anthology-mem save *)",
        ]),
        ..PromptOpts::default()
    };
    let p = ctx.prompt_run(&tmp, prompt, &flags, &opts)?;
    // A stream-json event has the same message.content shape as a transcript line.
    let main_calls = transcript_calls(&p.out);
    let mut sub_calls = Vec::new();
    for f in subagent_files(&tmp.join("cfg")) {
        sub_calls.extend(transcript_calls(&read_lossy(&f)?));
    }
    let note_path = repo
        .join(".claude")
        .join("agent-memory")
        .join("anthology-scout")
        .join("project-eval-probe.md");
    let note = if note_path.is_file() {
        Some(
            fs::read_to_string(&note_path)
                .map_err(|e| format!("OSError: {}: {e}", note_path.display()))?,
        )
    } else {
        None
    };
    let problems = memory_save_problems(
        &main_calls,
        &sub_calls,
        note.as_deref(),
        &colon_memory_paths(&tmp),
    );
    Ok(if problems.is_empty() {
        (
            Status::Pass,
            "the scout saved its note with anthology-mem save, no heredoc or write tool, in its own folder"
                .to_string(),
        )
    } else {
        (
            Status::Fail,
            format!(
                "{} (exit {}: {})",
                problems.join("; "),
                p.code,
                ctx.snippet(&p.text(), 80)
            ),
        )
    })
}
