//! `worktree-base` (U2): with `worktree.baseRef=head` an isolated agent sees the unpushed commit;
//! the platform default does not.

use std::path::{Path, PathBuf};

use crate::smoke::runner::*;

/// A clone of a bare origin, on `feat` with one unpushed commit carrying the marker.
fn unpushed_repo(ctx: &Ctx, tmp: &Path) -> Result<PathBuf, String> {
    ctx.git(tmp, tmp, &["init", "-q", "--bare", "origin.git"])?;
    ctx.git(tmp, tmp, &["clone", "-q", "origin.git", "repo"])?;
    let repo = tmp.join("repo");
    ctx.set_identity(tmp, &repo)?;
    ctx.git(tmp, &repo, &["checkout", "-q", "-b", "main"])?;
    write_text(&repo.join("README.md"), "smoke\n")?;
    ctx.git(tmp, &repo, &["add", "-A"])?;
    ctx.git(tmp, &repo, &["commit", "-q", "-m", "init"])?;
    ctx.git(tmp, &repo, &["push", "-q", "-u", "origin", "main"])?;
    ctx.git(tmp, &repo, &["checkout", "-q", "-b", "feat"])?;
    write_text(&repo.join("feat.txt"), "x\n")?;
    ctx.git(tmp, &repo, &["add", "-A"])?;
    ctx.git(
        tmp,
        &repo,
        &["commit", "-q", "-m", &format!("feat: {MARKER}")],
    )?;
    Ok(repo)
}

/// One run of the implementer: the process, whether an agent worktree was used, and whether the
/// marker is in a subagent tool result.
fn worktree_run(ctx: &Ctx, base_ref_head: bool) -> Res<(Proc, bool, bool)> {
    let mut settings = ctx.read_allow()?;
    if base_ref_head && let Some(map) = settings.as_object_mut() {
        map.insert(
            "worktree".to_string(),
            jobj(vec![("baseRef", jstr("head"))]),
        );
    }
    let tmp = scratch()?;
    let repo = unpushed_repo(ctx, &tmp)?;
    let mut flags = sv(["--model", "sonnet"]);
    flags.extend(ctx.plugin_flags()?);
    flags.extend(["--settings".to_string(), dumps(&settings)]);
    let opts = PromptOpts {
        cwd: Some(repo.clone()),
        variadic: sv(["Agent", "Read", "Bash(git log*)"]),
        ..PromptOpts::default()
    };
    let p = ctx.prompt_run(&tmp, PROMPT_WORKTREE, &flags, &opts)?;
    let mut texts = Vec::new();
    for f in subagent_files(&tmp.join("cfg")) {
        texts.push(read_lossy(&f)?);
    }
    let listed = ctx
        .git(&tmp, &repo, &["worktree", "list"])?
        .contains(".claude/worktrees/agent-");
    // An agent that changes nothing has its worktree removed again, so the `cwd` of the subagent
    // transcript entries is accepted as evidence too.
    let in_cwd = texts
        .iter()
        .flat_map(|t| transcript_cwds(t))
        .any(|c| c.contains(".claude/worktrees/agent-"));
    // The marker must be what the SUBAGENT's Bash returned: the main session runs on `feat` itself.
    let marker = texts
        .iter()
        .flat_map(|t| tool_result_texts(&stream_events(t)))
        .any(|r| r.contains(MARKER));
    Ok((p, listed || in_cwd, marker))
}

pub fn run(ctx: &Ctx) -> Res<Outcome> {
    let (p, worktree, marker) = worktree_run(ctx, true)?;
    let (_, _, control_marker) = worktree_run(ctx, false)?;
    let note = format!(
        "control without baseRef: marker {} in the agent",
        if control_marker { "present" } else { "absent" }
    );
    if p.code != 0 {
        return Ok((
            Status::Fail,
            format!(
                "claude exited {}: {}; {note}",
                p.code,
                ctx.snippet(&p.text(), 160)
            ),
        ));
    }
    if marker && worktree {
        return Ok((
            Status::Pass,
            format!("the unpushed commit is visible in an agent worktree; {note}"),
        ));
    }
    Ok((
        Status::Fail,
        format!(
            "marker in the agent's tool results: {}; agent worktree seen: {}; {note}",
            py_bool(marker),
            py_bool(worktree)
        ),
    ))
}
