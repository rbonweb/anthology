//! `live-setup` (no model): `/anthology:setup`'s two steps through the staged `bin/anthology`
//! wrapper and the seeded binary (RT4, F-8): `setup preview` then `setup apply` with the
//! previewed settings hash, in a scratch repo with a remote. Apply must succeed, write the
//! settings, report the guard binary, and approve the repo (MS1's `approve`): its key lands in
//! `<config>/anthology/approved-repos.json`.

use std::fs;

use anthology_pycompat::json::{self, Value};

use crate::smoke::runner::*;

fn parse_object(label: &str, p: &Proc) -> Result<Value, String> {
    if p.code != 0 {
        return Err(format!("{label} exited {}: {}", p.code, p.text()));
    }
    json::loads(&p.out)
        .ok()
        .filter(|v| v.as_object().is_some())
        .ok_or_else(|| format!("{label} printed no JSON object: {}", p.text()))
}

fn path_str<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .try_fold(v, |v, k| field(v, k))
        .and_then(Value::as_str)
}

pub fn run(ctx: &Ctx) -> Res<Outcome> {
    let anthology = ctx.staged_plugins()?.join("anthology");
    let tmp = scratch()?;
    let repo = ctx.make_repo(&tmp)?;
    ctx.git(
        &tmp,
        &repo,
        &[
            "remote",
            "add",
            "origin",
            "https://example.invalid/smoke/setup.git",
        ],
    )?;
    let config = tmp.join("cfg");
    ctx.seed_config(&config)?;
    let data = tmp.join("data");
    mkdirs(&data)?;
    let extra = EnvMap::from([
        (
            "CLAUDE_PLUGIN_ROOT".to_string(),
            anthology.to_string_lossy().into_owned(),
        ),
        (
            "CLAUDE_PLUGIN_DATA".to_string(),
            data.to_string_lossy().into_owned(),
        ),
    ]);
    let env = without_auth(build_env(&ctx.env, &tmp, Some(&extra)));
    let wrapper = anthology.join("bin").join("anthology");
    let setup = |args: &[&str], timeout: u64| {
        let mut argv = vec![wrapper.to_string_lossy().into_owned(), "setup".to_string()];
        argv.extend(args.iter().map(|a| a.to_string()));
        argv.extend([
            "--plugin-root".to_string(),
            anthology.to_string_lossy().into_owned(),
            "--cwd".to_string(),
            repo.to_string_lossy().into_owned(),
        ]);
        ctx.run(&argv, &repo, &env, timeout, None)
    };

    let fail = |msg: String| Ok((Status::Fail, ctx.snippet(&msg, 300)));
    let preview = match parse_object("setup preview", &setup(&["preview", "--json"], 120)) {
        Ok(v) => v,
        Err(e) => return fail(e),
    };
    if field(&preview, "supported").and_then(Value::as_bool) != Some(true) {
        return fail(format!(
            "setup preview: supported is not true: {}",
            py_repr(&preview)
        ));
    }
    if field(&preview, "guard_binary").is_none() {
        return fail("setup preview has no guard_binary (F-15)".to_string());
    }
    let Some(sha) = path_str(&preview, &["settings", "sha256"]).map(str::to_string) else {
        return fail("setup preview has no settings.sha256".to_string());
    };
    // atl is installed for real here (network), so apply gets the longer timeout.
    let applied = match parse_object(
        "setup apply",
        &setup(&["apply", "--json", "--expect-sha256", &sha], 240),
    ) {
        Ok(v) => v,
        Err(e) => return fail(e),
    };
    let mut problems: Vec<String> = Vec::new();
    if field(&applied, "ok").and_then(Value::as_bool) != Some(true) {
        problems.push(format!(
            "setup apply: ok is not true: {}",
            py_repr(&applied)
        ));
    }
    let guard = path_str(&applied, &["guard_binary", "status"]).unwrap_or("");
    // The staged guardrails sit beside anthology, so setup must find them.
    if !matches!(guard, "present" | "installed") {
        problems.push(format!("guard_binary.status is {}", py_repr_str(guard)));
    }
    if !config.join("settings.json").is_file() {
        problems.push("setup apply wrote no settings.json in the config dir".to_string());
    }
    match path_str(&applied, &["memory", "approved_key"]) {
        Some(key) if !key.is_empty() => {
            let approved = fs::read_to_string(config.join("anthology/approved-repos.json"))
                .ok()
                .and_then(|t| json::loads(&t).ok());
            let listed = approved
                .as_ref()
                .and_then(|a| field(a, "repos"))
                .and_then(|r| field(r, key))
                .is_some();
            if !listed {
                problems.push(format!(
                    "the approved key {} is not in approved-repos.json",
                    py_repr_str(key)
                ));
            }
        }
        _ => problems.push(format!(
            "setup apply approved no repo: memory is {}",
            field(&applied, "memory").map_or_else(|| "missing".to_string(), py_repr)
        )),
    }
    Ok(if problems.is_empty() {
        (
            Status::Pass,
            format!(
                "setup preview and apply ran through bin/anthology: settings written, guard binary {guard}, repo approved"
            ),
        )
    } else {
        (Status::Fail, ctx.snippet(&problems.join("; "), 300))
    })
}
