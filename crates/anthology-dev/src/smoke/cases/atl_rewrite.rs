//! `atl-rewrite` (U6): with atl installed the hook rewrites `git status` and `playwright` to atl,
//! and G1 still blocks the latter. (`claude plugin eval` cannot run this: its sandbox has no atl
//! state, so the hook never rewrites.)

use crate::smoke::runner::*;

pub fn run(ctx: &Ctx) -> Res<Outcome> {
    let prompt = "Run these three Bash commands, one per tool call, in this order: first `git init -q atlrepo`, then \
`git -C atlrepo status`, then `npx playwright test --list`. Report each command's output verbatim.";
    let mut flags = sv(["--model", "haiku"]);
    flags.extend(ctx.plugin_flags()?);
    flags.extend(sv([
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-hook-events",
        "--max-turns",
        "8",
    ]));
    // The rewritten command must be allowed too, or the run stops at "This command requires approval".
    let allowed = sv([
        "Bash(git init -q atlrepo)",
        "Bash(git -C atlrepo status)",
        "Bash(atl git -C atlrepo status)",
        "Bash(npx playwright test --list)",
        "Bash(atl playwright test --list)",
    ]);
    let tmp = scratch()?;
    // The hooks fall back to $HOME/.claude when scrubbing strips CLAUDE_CONFIG_DIR.
    let cfg = tmp.join("home").join(".claude");
    mkdirs(&cfg)?;
    let env = EnvMap::from([(
        "CLAUDE_CONFIG_DIR".to_string(),
        cfg.to_string_lossy().into_owned(),
    )]);
    // The Rust installer (`anthology atl-install`, RT2) with the staged plugin's atl.lock; it
    // records atl_path in the scratch config's state.json, which bin/atl-hook reads.
    let lock = ctx.staged_plugins()?.join("anthology").join("atl.lock");
    let argv = vec![
        ctx.anthology_bin()?.to_string_lossy().into_owned(),
        "atl-install".to_string(),
        "--data-dir".to_string(),
        tmp.join("data").to_string_lossy().into_owned(),
        "--lock".to_string(),
        lock.to_string_lossy().into_owned(),
    ];
    let install = ctx.run(
        &argv,
        &tmp,
        &build_env(&ctx.env, &tmp, Some(&env)),
        120,
        None,
    );
    if install.code != 0 {
        return Ok((
            Status::Fail,
            format!("atl install failed: {}", ctx.snippet(&install.text(), 160)),
        ));
    }
    let ws = tmp.join("ws");
    mkdirs(&ws)?;
    let opts = PromptOpts {
        cwd: Some(ws),
        extra_env: Some(env),
        variadic: allowed,
    };
    let p = ctx.prompt_run(&tmp, prompt, &flags, &opts)?;
    let events = stream_events(&p.out);
    let problems = atl_problems(&tool_result_texts(&events), &hook_failure_text(&events));
    Ok(if problems.is_empty() {
        (
            Status::Pass,
            "atl condensed git status, the raw status text is absent and G1 blocked the playwright command"
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
