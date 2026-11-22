//! `live-seed` (no model; RL1's live seed check, N-I4): the staged `guard.sh` runs the real
//! `anthology` binary that `anthology-dev seed` installed, on the hot path. A plain command is
//! allowed (the binary's allow token), a Playwright install is blocked by G1, a read-only tool
//! is allowed, and the binary is never quarantined.

use std::fs;

use crate::smoke::runner::*;
use crate::smoke::staged::{bash, envelope, hook_command, hook_env, quarantined};

pub fn run(ctx: &Ctx) -> Res<Outcome> {
    let guardrails = ctx.staged_plugins()?.join("anthology-guardrails");
    let tmp = scratch()?;
    let config = tmp.join("cfg");
    ctx.seed_config(&config)?;
    let (home, tmpdir, project) = (tmp.join("home"), tmp.join("t"), tmp.join("project"));
    for d in [&home, &tmpdir, &project] {
        mkdirs(d)?;
    }
    let mut env = hook_env(
        ctx.env.get("PATH").unwrap_or(""),
        &home,
        &config,
        &tmpdir,
        &guardrails,
        &project,
    );
    // The seeded binary must be the one that answers: no download may stand in for it.
    env.insert("ANTHOLOGY_GUARD_NO_FETCH".to_string(), "1".to_string());
    let guard = hook_command(&guardrails, "PreToolUse", "guard.sh")?;
    let call = |stdin: &str| ctx.run(&guard, &project, &env, 60, Some(stdin));

    let mut problems: Vec<String> = Vec::new();
    let allowed = call(&bash("ls", &project));
    if allowed.code != 0 || !allowed.err.is_empty() || !allowed.out.is_empty() {
        problems.push(format!(
            "`ls` was not allowed cleanly (exit {}): {}",
            allowed.code,
            ctx.snippet(&allowed.text(), 120)
        ));
    }
    let g1 = call(&bash("npx playwright install chromium", &project));
    if g1.code != 2 || !g1.err.starts_with("Blocked by anthology-guardrails (G1): ") {
        problems.push(format!(
            "the Playwright install was not blocked by G1 (exit {}): {}",
            g1.code,
            ctx.snippet(&g1.err, 160)
        ));
    }
    let read = call(&envelope(
        "Read",
        jobj(vec![("file_path", jstr("README.md"))]),
        &project,
    ));
    if read.code != 0 {
        problems.push(format!(
            "Read was not allowed (exit {}): {}",
            read.code,
            ctx.snippet(&read.err, 120)
        ));
    }
    let broken = quarantined(&config);
    if !broken.is_empty() {
        problems.push(format!(
            "the seeded binary was quarantined: {}",
            broken
                .iter()
                .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let bin_dir = config.join("anthology").join("bin");
    let installed = fs::read_dir(&bin_dir).is_ok_and(|it| {
        it.flatten().any(|e| {
            !e.file_name().to_string_lossy().starts_with('.')
                && e.path().join("anthology").is_file()
        })
    });
    if !installed {
        problems.push(format!(
            "no installed binary is left in {}",
            bin_dir.display()
        ));
    }
    Ok(if problems.is_empty() {
        (
            Status::Pass,
            "the staged guard.sh ran the seeded binary: ls allowed, G1 blocked, Read allowed, nothing quarantined"
                .to_string(),
        )
    } else {
        (Status::Fail, problems.join("; "))
    })
}
