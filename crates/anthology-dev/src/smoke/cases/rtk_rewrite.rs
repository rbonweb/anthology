//! `rtk-rewrite` (U6): with rtk installed the hook rewrites `git status` and `playwright` to rtk,
//! and G1 still blocks the latter. (`claude plugin eval` cannot run this: its sandbox has no rtk
//! state, so the hook never rewrites.)

use crate::smoke::runner::*;

pub fn run(ctx: &Ctx) -> Res<Outcome> {
    let root = ctx.root()?;
    let prompt = "Run these three Bash commands, one per tool call, in this order: first `git init -q rtkrepo`, then \
`git -C rtkrepo status`, then `npx playwright test --list`. Report each command's output verbatim.";
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
        "Bash(git init -q rtkrepo)",
        "Bash(git -C rtkrepo status)",
        "Bash(rtk git -C rtkrepo status)",
        "Bash(npx playwright test --list)",
        "Bash(rtk playwright test --list)",
    ]);
    let tmp = scratch()?;
    // The hooks fall back to $HOME/.claude when scrubbing strips CLAUDE_CONFIG_DIR.
    let cfg = tmp.join("home").join(".claude");
    mkdirs(&cfg)?;
    let env = EnvMap::from([(
        "CLAUDE_CONFIG_DIR".to_string(),
        cfg.to_string_lossy().into_owned(),
    )]);
    // Still the Python installer until CO2; EV1b swaps it for the Rust binary.
    let argv = vec![
        "python3".to_string(),
        root.join("plugins/anthology/scripts/rtk_install.py")
            .to_string_lossy()
            .into_owned(),
        "--data-dir".to_string(),
        tmp.join("data").to_string_lossy().into_owned(),
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
            format!("rtk install failed: {}", ctx.snippet(&install.text(), 160)),
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
    let problems = rtk_problems(&tool_result_texts(&events), &hook_failure_text(&events));
    Ok(if problems.is_empty() {
        (
            Status::Pass,
            "rtk condensed git status, the raw status text is absent and G1 blocked the playwright command"
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
