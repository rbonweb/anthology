//! `live-mem` (no model): `anthology-mem` through the staged `bin/anthology-mem` wrapper and the
//! seeded binary (MI1). In an approved scratch repo (MS1's `approve`), `save` writes the scout's
//! note into its own agent-memory folder, and `search` then finds it through the index.

use std::fs;

use crate::smoke::runner::*;

const SLUG: &str = "live-probe";
const NEEDLE: &str = "aquamarine";

pub fn run(ctx: &Ctx) -> Res<Outcome> {
    let anthology = ctx.staged_plugins()?.join("anthology");
    let tmp = scratch()?;
    let repo = ctx.approved_repo(&tmp)?;
    ctx.seed_config(&tmp.join("cfg"))?;
    let data = tmp.join("data");
    mkdirs(&data)?;
    let extra = EnvMap::from([(
        "CLAUDE_PLUGIN_DATA".to_string(),
        data.to_string_lossy().into_owned(),
    )]);
    let env = without_auth(build_env(&ctx.env, &tmp, Some(&extra)));
    let wrapper = anthology
        .join("bin")
        .join("anthology-mem")
        .to_string_lossy()
        .into_owned();
    let mem = |args: &[&str]| {
        let mut argv = vec![wrapper.clone()];
        argv.extend(args.iter().map(|a| a.to_string()));
        ctx.run(&argv, &repo, &env, 120, None)
    };
    let description = format!("the live probe colour is {NEEDLE}");
    let saved = mem(&[
        "save",
        "--agent",
        "scout",
        "--type",
        "project",
        "--slug",
        SLUG,
        "--name",
        SLUG,
        "--description",
        &description,
        "--body",
        "This smoke checks that the Rust anthology-mem saves a note and finds it again.",
    ]);
    if saved.code != 0 {
        return Ok((
            Status::Fail,
            format!(
                "anthology-mem save exited {}: {}",
                saved.code,
                ctx.snippet(&saved.text(), 160)
            ),
        ));
    }
    let note = repo
        .join(".claude/agent-memory/anthology-scout")
        .join(format!("project-{SLUG}.md"));
    let text = fs::read_to_string(&note).unwrap_or_default();
    if !text.contains(NEEDLE) {
        return Ok((
            Status::Fail,
            format!(
                "anthology-mem save exited 0 but {} does not hold the note: {}",
                note.display(),
                ctx.snippet(&saved.text(), 120)
            ),
        ));
    }
    let found = mem(&["search", NEEDLE, "--json"]);
    if found.code != 0 || !found.out.contains(SLUG) {
        return Ok((
            Status::Fail,
            format!(
                "anthology-mem search {NEEDLE} did not find {SLUG} (exit {}): {}",
                found.code,
                ctx.snippet(&found.text(), 160)
            ),
        ));
    }
    Ok((
        Status::Pass,
        "anthology-mem saved the scout's note in its own folder and search found it".to_string(),
    ))
}
