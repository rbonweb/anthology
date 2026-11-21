//! `auto-memory-dir` (U7): the memory sync points `autoMemoryDirectory` at the repo, copies old
//! notes to personal memory only (B3: never into the repo on their own), and new saves go to the repo.

use std::fs;
use std::path::{Path, PathBuf};

use anthology_pycompat::json::{self, Value};

use crate::smoke::runner::*;

/// `os.path.realpath`: symlinks resolved as far as the path exists, the rest kept.
fn realpath(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut existing = absolute.as_path();
    let mut rest: Vec<&std::ffi::OsStr> = Vec::new();
    loop {
        if let Ok(real) = fs::canonicalize(existing) {
            return rest.iter().rev().fold(real, |acc, part| acc.join(part));
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name);
                existing = parent;
            }
            _ => return absolute,
        }
    }
}

/// `autoMemoryDirectory` of `settings.local.json` (`None` when unreadable or not JSON).
fn auto_memory_setting(repo: &Path) -> Option<Value> {
    let text = fs::read_to_string(repo.join(".claude/settings.local.json")).ok()?;
    let mut data = json::loads(&text).ok()?;
    Some(
        data.as_object_mut()?
            .remove("autoMemoryDirectory")
            .unwrap_or(Value::Null),
    )
}

/// Every `<cfg>/projects/*/memory`.
fn default_memory_dirs(cfg: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(cfg.join("projects")) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path().join("memory"))
        .filter(|p| p.exists())
        .collect();
    dirs.sort();
    dirs
}

pub fn run(ctx: &Ctx) -> Res<Outcome> {
    let root = ctx.root()?;
    if !has_session_start(&root.join("plugins/anthology/hooks/hooks.json")) {
        return Ok((
            Status::Skip,
            "no SessionStart memory sync yet (stream F)".to_string(),
        ));
    }
    let mut flags = sv([
        "--permission-mode",
        "bypassPermissions",
        "--model",
        "sonnet",
    ]);
    flags.extend(ctx.plugin_flags()?);
    flags.extend(sv(["--max-turns", "6"]));
    // bypassPermissions: cwd/HOME/config in this temp dir (not a sandbox), guardrails loaded.
    let tmp = scratch()?;
    let repo = ctx.approved_repo(&tmp)?;
    let mem = repo.join(".claude").join("memory");
    let opts = PromptOpts {
        cwd: Some(repo.clone()),
        extra_env: Some(no_scrub()),
        ..PromptOpts::default()
    };
    let session = |prompt: &str| ctx.prompt_run(&tmp, prompt, &flags, &opts);
    session(
        "Remember for future sessions that the smoke probe color is amber; save it to auto memory now.",
    )?;
    // Session 1 starts before the setting exists, so the note goes to the platform's default memory
    // dir; a note already in the repo would make the migration check below pass without migrating
    // anything.
    let in_default = default_memory_dirs(&tmp.join("cfg"))
        .iter()
        .any(|d| find_text(d, "amber"));
    let in_repo = find_text(&mem, "amber");
    if in_repo || !in_default {
        return Ok((
            Status::Fail,
            format!(
                "session 1: want the amber note in the default memory dir (cfg/projects/*/memory) and not yet in \
                 the repo; in the default dir: {}, in the repo: {}",
                py_bool(in_default),
                py_bool(in_repo)
            ),
        ));
    }
    let setting = auto_memory_setting(&repo);
    let setting_ok = match &setting {
        Some(Value::Str(s)) if !s.is_empty() => realpath(Path::new(s)) == realpath(&mem),
        _ => false,
    };
    if !setting_ok {
        let shown = setting.as_ref().map_or_else(|| "None".to_string(), py_repr);
        return Ok((
            Status::Fail,
            format!(
                "settings.local.json autoMemoryDirectory is {shown}, want {}",
                mem.display()
            ),
        ));
    }
    session("Reply OK.")?;
    let personal = tmp.join("cfg").join("anthology").join("personal-memory"); // CLAUDE_CONFIG_DIR is tmp/cfg
    if find_text(&mem, "amber") {
        return Ok((
            Status::Fail,
            "the amber note was copied into the repo's .claude/memory/ after session 2; legacy notes stay personal (B3)"
                .to_string(),
        ));
    }
    if !find_text(&personal, "amber") {
        return Ok((
            Status::Fail,
            "the amber note is not in personal memory (cfg/anthology/personal-memory) after session 2 (migration)"
                .to_string(),
        ));
    }
    session("Remember that the smoke probe shape is cyan-circle; save it to auto memory now.")?;
    if !find_text(&mem, "cyan-circle") {
        return Ok((
            Status::Fail,
            "the cyan-circle note is not under .claude/memory/ after session 3".to_string(),
        ));
    }
    Ok((
        Status::Pass,
        "autoMemoryDirectory set, old note copied to personal memory only, new note saved in the repo"
            .to_string(),
    ))
}
