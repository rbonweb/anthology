//! `missing-interpreter` (U8): a hook whose interpreter is missing is non-blocking by default, and
//! blocks with `onFailure`.

use crate::smoke::runner::*;

/// What one run showed.
struct Seen {
    ran: bool,
    bash: bool,
    not_found: bool,
    blocked: bool,
    text: String,
}

pub fn run(ctx: &Ctx) -> Res<Outcome> {
    let prompt = "Run this Bash command once: echo RAN-OK";
    let flags = sv([
        "--model",
        "haiku",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-hook-events",
        "--max-turns",
        "3",
    ]);
    let mut seen: Vec<Seen> = Vec::new();
    let tmp = scratch()?;
    for (label, on_failure) in [("default", false), ("block", true)] {
        let mut hook = vec![
            ("type", jstr("command")),
            ("command", jstr("python9")),
            (
                "args",
                jarr(vec![jstr("${CLAUDE_PLUGIN_ROOT}/hooks/none.py")]),
            ),
        ];
        if on_failure {
            hook.push(("onFailure", jstr("block")));
        }
        let hooks = jobj(vec![(
            "PreToolUse",
            jarr(vec![jobj(vec![
                ("matcher", jstr("Bash")),
                ("hooks", jarr(vec![jobj(hook)])),
            ])]),
        )]);
        let name = format!("miss-{label}");
        let probe = write_plugin(&tmp.join(&name), &name, Some(&hooks), &[])?;
        let mut run_flags = flags.clone();
        run_flags.extend([
            "--plugin-dir".to_string(),
            probe.to_string_lossy().into_owned(),
        ]);
        let opts = PromptOpts {
            variadic: sv(["Bash(echo *)"]),
            ..PromptOpts::default()
        };
        let p = ctx.prompt_run(&tmp, prompt, &run_flags, &opts)?;
        let events = stream_events(&p.out);
        let results = tool_result_texts(&events);
        let hooks_text = hook_failure_text(&events);
        seen.push(Seen {
            ran: results.iter().any(|t| t.contains("RAN-OK")),
            bash: tool_uses(&events).iter().any(|t| t == "Bash"),
            not_found: not_found(&hooks_text),
            blocked: blocked(&format!("{hooks_text}\n{}", results.join("\n"))),
            text: p.text(),
        });
    }
    let (d, b) = (&seen[0], &seen[1]);
    if !(d.ran && d.not_found) {
        return Ok((
            Status::Fail,
            format!(
                "default: command ran={}, hook reports a missing executable={}: {}",
                py_bool(d.ran),
                py_bool(d.not_found),
                ctx.snippet(&d.text, 160)
            ),
        ));
    }
    if !(b.bash && b.blocked && !b.ran) {
        return Ok((
            Status::Fail,
            format!(
                "onFailure=block: Bash called={}, command ran={}, 'blocking because onFailure is block' seen={}",
                py_bool(b.bash),
                py_bool(b.ran),
                py_bool(b.blocked)
            ),
        ));
    }
    Ok((
        Status::Pass,
        "a missing interpreter reports 'not found' and lets the command run; onFailure=block stops it"
            .to_string(),
    ))
}
