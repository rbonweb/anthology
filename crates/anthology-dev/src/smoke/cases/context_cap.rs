//! `context-cap` (U8): a SessionStart hook's `additionalContext` is cut at 10,000 characters (the
//! budget is 9,000).

use crate::smoke::runner::*;

pub fn run(ctx: &Ctx) -> Res<Outcome> {
    let prompt = "Without using tools, quote any token that starts with END-, else reply NONE.";
    let mut seen: Vec<(usize, String)> = Vec::new();
    let tmp = scratch()?;
    // POSIX sh, so the smoke needs no python3 (R4); `CAP_N` reaches it through the hook's environment.
    let hooks = jobj(vec![(
        "SessionStart",
        jarr(vec![jobj(vec![(
            "hooks",
            jarr(vec![jobj(vec![
                ("type", jstr("command")),
                ("command", jstr("sh")),
                (
                    "args",
                    jarr(vec![jstr("${CLAUDE_PLUGIN_ROOT}/hooks/ss.sh")]),
                ),
            ])]),
        )])]),
    )]);
    let script = cap_hook_script();
    let probe = write_plugin(
        &tmp.join("cap-probe"),
        "cap-probe",
        Some(&hooks),
        &[("hooks/ss.sh", &script)],
    )?;
    for n in [10000usize, 10001] {
        let mut flags = sv(["--max-turns", "1", "--model", "haiku", "--plugin-dir"]);
        flags.push(probe.to_string_lossy().into_owned());
        let opts = PromptOpts {
            extra_env: Some(EnvMap::from([("CAP_N".to_string(), n.to_string())])),
            ..PromptOpts::default()
        };
        let p = ctx.prompt_run(&tmp, prompt, &flags, &opts)?;
        if p.code != 0 {
            return Ok((
                Status::Fail,
                format!(
                    "claude exited {} at N={n}: {}",
                    p.code,
                    ctx.snippet(&p.text(), 160)
                ),
            ));
        }
        seen.push((n, p.out));
    }
    let (first, second) = (&seen[0].1, &seen[1].1);
    if first.contains("END-10000") && !second.contains("END-10001") {
        return Ok((
            Status::Pass,
            "10,000 characters arrive intact and 10,001 are cut".to_string(),
        ));
    }
    Ok((
        Status::Fail,
        format!(
            "the SessionStart cap moved; re-check the 9,000-char budget \
             (N=10000 quoted its end: {}; N=10001 quoted its end: {})",
            py_bool(first.contains("END-10000")),
            py_bool(second.contains("END-10001"))
        ),
    ))
}
