//! `guard-timeout` (m-8): a `PreToolUse` command hook that outlives its `timeout` with
//! `onFailure: "block"` blocks the call, and the tool result says the hook timed out. A model
//! smoke, modelled on `missing-interpreter`; needs no Anthology binary.
//!
//! The wording is what Claude Code 2.1.311 prints (see `guard_timeout_problems`): the tool result
//! `PreToolUse:Bash hook error: [sleep 30]: timed out; blocking because onFailure is "block"`.

use crate::smoke::runner::*;

pub fn run(ctx: &Ctx) -> Res<Outcome> {
    let prompt = "Run this Bash command once: echo RAN-OK";
    let tmp = scratch()?;
    // `sleep 30` outlives `timeout: 5`, so the host must kill it and apply onFailure.
    let hook = jobj(vec![
        ("type", jstr("command")),
        ("command", jstr("sleep")),
        ("args", jarr(vec![jstr("30")])),
        ("timeout", jint(5)),
        ("onFailure", jstr("block")),
    ]);
    let hooks = jobj(vec![(
        "PreToolUse",
        jarr(vec![jobj(vec![
            ("matcher", jstr("Bash")),
            ("hooks", jarr(vec![hook])),
        ])]),
    )]);
    let probe = write_plugin(
        &tmp.join("guard-timeout-probe"),
        "guard-timeout-probe",
        Some(&hooks),
        &[],
    )?;
    let mut flags = sv([
        "--model",
        "haiku",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-hook-events",
        "--max-turns",
        "3",
        "--plugin-dir",
    ]);
    flags.push(probe.to_string_lossy().into_owned());
    let opts = PromptOpts {
        variadic: sv(["Bash(echo *)"]),
        ..PromptOpts::default()
    };
    let p = ctx.prompt_run(&tmp, prompt, &flags, &opts)?;
    let events = stream_events(&p.out);
    let results = tool_result_texts(&events);
    let bash_called = tool_uses(&events).iter().any(|t| t == "Bash");
    let problems = guard_timeout_problems(bash_called, &hook_failure_text(&events), &results);
    Ok(if problems.is_empty() {
        (
            Status::Pass,
            "a hook that outlives its timeout is reported as timed out and, with onFailure=block, stops the command"
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
