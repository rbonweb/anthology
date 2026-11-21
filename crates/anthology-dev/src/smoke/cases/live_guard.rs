//! `live-guard` (model): in a real `claude -p` session with the staged plugins and the seeded
//! binary, the guard hook (`hooks/guard.sh` → `anthology guard hook --shim`, GD2) lets a plain
//! command through and blocks a Playwright install with G1. Neither call may meet the
//! fail-closed `(setup)` block, which would mean the binary never answered.

use crate::smoke::runner::*;

const OK: &str = "live-guard-ok";

pub fn run(ctx: &Ctx) -> Res<Outcome> {
    let prompt = "Run these two Bash commands, one per tool call, in this order: first `echo live-guard-ok`, \
then `npx playwright install --dry-run chromium`. Report each command's output verbatim.";
    let tmp = scratch()?;
    let ws = tmp.join("ws");
    mkdirs(&ws)?;
    let mut flags = sv(["--model", "haiku"]);
    flags.extend(ctx.plugin_flags()?);
    flags.extend(sv([
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-hook-events",
        "--max-turns",
        "6",
    ]));
    let opts = PromptOpts {
        cwd: Some(ws),
        variadic: sv([
            "Bash(echo live-guard-ok)",
            "Bash(npx playwright install --dry-run chromium)",
        ]),
        ..PromptOpts::default()
    };
    let p = ctx.prompt_run(&tmp, prompt, &flags, &opts)?;
    let events = stream_events(&p.out);
    let results = tool_result_texts(&events);
    let hooks = hook_failure_text(&events);
    let all = format!("{}\n{hooks}", results.join("\n"));
    let mut problems: Vec<String> = Vec::new();
    if !tool_uses(&events).iter().any(|t| t == "Bash") {
        problems.push("Bash was never called".to_string());
    }
    let allowed = |r: &String| {
        r.trim() == OK
            || (r.contains(OK) && !blocked(r) && !r.contains("Blocked by anthology-guardrails"))
    };
    if !results.iter().any(allowed) {
        problems.push(format!("`echo {OK}` was not allowed through"));
    }
    if !all.contains(GUARD_G1) {
        problems.push("G1 did not block the Playwright install".to_string());
    }
    if all.contains("anthology-guardrails (setup)") {
        problems
            .push("the guard failed closed with (setup): the binary did not answer".to_string());
    }
    Ok(if problems.is_empty() {
        (
            Status::Pass,
            "the seeded guard binary allowed echo and G1 blocked the Playwright install"
                .to_string(),
        )
    } else {
        (
            Status::Fail,
            format!(
                "{} (exit {}: {})",
                problems.join("; "),
                p.code,
                ctx.snippet(&all, 160)
            ),
        )
    })
}
