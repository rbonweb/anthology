//! `live-session-start` (model): a real `claude -p` session with the staged plugins and the
//! seeded binary. The anthology SessionStart hook (`hooks/session-start.sh` → `anthology hook
//! session-start`, RT3) puts the routing guide into the session, and no hook reports an install
//! failure, an incomplete start or a failing-closed guard (GC-7, GC-14).

use crate::smoke::runner::*;

/// The routing guide's title (templates/routing-guide.md), as the session context carries it.
const GUIDE: &str = "Anthology routing guide";

/// Texts that mean the start went wrong (GC-7 and GC-14).
const FAILURES: [&str; 5] = [
    "could not install the",
    "session start did not complete",
    "the guardrails are failing closed",
    "guardrails self-test",
    "Blocked by anthology-guardrails (setup)",
];

pub fn run(ctx: &Ctx) -> Res<Outcome> {
    let prompt = "If your context holds a routing guide, reply with its title (the text of its first \
heading) and nothing else; otherwise reply NONE.";
    let tmp = scratch()?;
    let repo = ctx.make_repo(&tmp)?;
    let mut flags = sv(["--model", "haiku"]);
    flags.extend(ctx.plugin_flags()?);
    flags.extend(sv([
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-hook-events",
        "--max-turns",
        "1",
    ]));
    let opts = PromptOpts {
        cwd: Some(repo),
        ..PromptOpts::default()
    };
    let p = ctx.prompt_run(&tmp, prompt, &flags, &opts)?;
    let events = stream_events(&p.out);
    let hooks = hook_failure_text(&events);
    let reply = result_cost_and_text(&p.out).1.unwrap_or_default();
    let mut problems: Vec<String> = Vec::new();
    if !hooks.contains(GUIDE) && !reply.contains(GUIDE) {
        problems.push(
            "the routing guide is not in the session (no hook output or reply names it)"
                .to_string(),
        );
    }
    for bad in FAILURES {
        if hooks.contains(bad) || reply.contains(bad) {
            problems.push(format!("a hook reported {}", py_repr_str(bad)));
        }
    }
    Ok(if problems.is_empty() {
        (
            Status::Pass,
            "session start ran the seeded binary: the routing guide is in context and no hook failed"
                .to_string(),
        )
    } else {
        (
            Status::Fail,
            format!(
                "{} (exit {}: {})",
                problems.join("; "),
                p.code,
                ctx.snippet(&format!("{hooks}\n{reply}"), 160)
            ),
        )
    })
}
