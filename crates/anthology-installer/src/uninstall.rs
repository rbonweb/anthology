//! `anthology-installer uninstall`: the install undone, once the person has confirmed.
//!
//! Order: the settings edits first (they need the binary the plugins carry), then the plugins,
//! then the marketplace, and only when no guardrails plugin is left, the downloaded binary.
//! Anything that fails stops the run, so nothing is left half-removed without saying so.

use std::fs;

use anthology_pycompat::json::Value;

use crate::binary;
use crate::claude::{MARKETPLACE, Outcome, PLUGINS, Plugin};
use crate::ctx::Ctx;
use crate::proc::{Cmd, find_in_path};
use crate::setup::{ANTHOLOGY, result_object};
use crate::state::{self, Lock};
use crate::steps::{EXIT_FAILED, plugin_step};

pub fn run(ctx: &mut Ctx) -> i32 {
    if find_in_path("claude", &ctx.path).is_none() {
        ctx.report.fail("preflight", "claude not found on PATH");
        return EXIT_FAILED;
    }
    if !ctx.yes {
        if !ctx.interactive {
            ctx.report.fail(
                "uninstall",
                "this asks before it removes anything, and there is no terminal; \
                 run it in one, or pass --yes",
            );
            return EXIT_FAILED;
        }
        let question =
            "Uninstall Anthology: its setup edits, its three plugins and its marketplace?";
        if !ctx.ask.confirm(question, false) {
            ctx.report
                .skip("uninstall", "cancelled, nothing was changed");
            return 0;
        }
    }
    let _lock = match Lock::acquire(&ctx.ah, ctx.runner) {
        Ok(lock) => lock,
        Err(e) => {
            ctx.report.fail("lock", &e);
            return EXIT_FAILED;
        }
    };
    match in_order(ctx) {
        Ok(()) => 0,
        Err(code) => code,
    }
}

fn in_order(ctx: &mut Ctx) -> Result<(), i32> {
    let plugins = ctx.claude().plugins().map_err(|e| {
        ctx.report
            .fail("plugins", &format!("cannot read the plugin list: {e}"));
        EXIT_FAILED
    })?;
    remove_setup(ctx, &plugins)?;
    remove_plugins(ctx, &plugins)?;
    remove_marketplace(ctx)?;
    delete_binary(ctx)?;
    forget_state(ctx);
    Ok(())
}

/// Step 1: `setup remove --preview`, shown, then `setup remove`.
fn remove_setup(ctx: &mut Ctx, plugins: &[Plugin]) -> Result<(), i32> {
    let root = plugins
        .iter()
        .find(|p| p.id == ANTHOLOGY)
        .and_then(|p| p.install_path.clone());
    let Some(root) = root.filter(|r| r.join("bin/anthology").is_file()) else {
        ctx.report.skip(
            "setup",
            "the anthology plugin is not installed; no setup edits to remove through it",
        );
        return Ok(());
    };
    if !binary::installed(&ctx.ah) {
        ctx.report.skip(
            "setup",
            "the anthology binary is not installed, so setup cannot be removed through it; \
             if you applied it, run /anthology:setup remove in Claude Code",
        );
        return Ok(());
    }
    let shim = root.join("bin/anthology").to_string_lossy().into_owned();
    let root_arg = root.to_string_lossy().into_owned();
    let _ = fs::create_dir_all(&ctx.ah);
    let run = |ctx: &Ctx, preview: bool| {
        let mut args = vec!["setup", "remove", "--plugin-root", root_arg.as_str()];
        if preview {
            args.push("--preview");
        }
        let out = ctx
            .runner
            .run(&Cmd::new(&shim, &args).cwd(&ctx.ah))
            .map_err(|e| format!("cannot run the anthology binary: {e}"))?;
        result_object(&out, "setup remove")
    };
    let preview = run(ctx, true).map_err(|e| {
        ctx.report
            .fail("setup", &format!("setup remove --preview failed: {e}"));
        EXIT_FAILED
    })?;
    ctx.report.note(&describe_removal(&preview));
    run(ctx, false).map_err(|e| {
        ctx.report
            .fail("setup", &format!("setup remove failed: {e}"));
        EXIT_FAILED
    })?;
    ctx.report.ok("setup", "removed the setup edits");
    Ok(())
}

/// What `setup remove --preview` says it will do, as lines for the person.
fn describe_removal(preview: &anthology_pycompat::json::Map) -> String {
    let mut text = String::new();
    if let Some(settings) = preview.get("settings").and_then(Value::as_object) {
        let path = settings
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or("settings.json");
        let diff = settings.get("diff").and_then(Value::as_str).unwrap_or("");
        if !diff.is_empty() {
            text.push_str(&format!("{path}\n{diff}\n"));
        }
    }
    for key in ["atl", "atl_history"] {
        let Some(item) = preview.get(key).and_then(Value::as_object) else {
            continue;
        };
        if item.get("status").and_then(Value::as_str) == Some("delete")
            && let Some(dir) = item.get("dir").and_then(Value::as_str)
        {
            text.push_str(&format!("will delete {dir}\n"));
        }
    }
    text
}

/// Step 2: `anthology` with `--prune`, then the other two if they are still listed.
fn remove_plugins(ctx: &mut Ctx, plugins: &[Plugin]) -> Result<(), i32> {
    let claude = ctx.claude();
    let step = plugin_step(PLUGINS[0]);
    if plugins.iter().any(|p| p.id == PLUGINS[0]) {
        match claude.plugin_uninstall_prune(PLUGINS[0]) {
            Outcome::Done { .. } => ctx.report.ok(&step, "uninstalled"),
            Outcome::Failed(e) | Outcome::NeedsConfirm(e) => {
                ctx.report.fail(&step, &format!("uninstall failed: {e}"));
                return Err(EXIT_FAILED);
            }
        }
    } else {
        ctx.report.skip(&step, "not installed");
    }
    let left = claude.plugins().map_err(|e| {
        ctx.report
            .fail("plugins", &format!("cannot read the plugin list: {e}"));
        EXIT_FAILED
    })?;
    for id in &PLUGINS[1..] {
        let step = plugin_step(id);
        if !left.iter().any(|p| p.id == *id) {
            let was_there = plugins.iter().any(|p| p.id == *id);
            ctx.report.skip(
                &step,
                if was_there {
                    "already gone"
                } else {
                    "not installed"
                },
            );
            continue;
        }
        match claude.plugin_uninstall(id) {
            Outcome::Done { .. } => ctx.report.ok(&step, "uninstalled"),
            Outcome::Failed(e) | Outcome::NeedsConfirm(e) => {
                ctx.report.fail(&step, &format!("uninstall failed: {e}"));
                return Err(EXIT_FAILED);
            }
        }
    }
    Ok(())
}

/// Step 3: our marketplace, never another source's.
fn remove_marketplace(ctx: &mut Ctx) -> Result<(), i32> {
    let claude = ctx.claude();
    let list = claude.marketplaces().map_err(|e| {
        ctx.report.fail(
            "marketplace",
            &format!("cannot read the marketplace list: {e}"),
        );
        EXIT_FAILED
    })?;
    match list.iter().find(|m| m.name == MARKETPLACE) {
        None => ctx.report.skip("marketplace", "not configured"),
        Some(m) if !m.source.is_ours() => ctx.report.skip(
            "marketplace",
            &format!(
                "comes from another source ({}); left alone",
                m.source.describe()
            ),
        ),
        Some(_) => match claude.marketplace_remove(MARKETPLACE) {
            Outcome::Done { .. } => ctx.report.ok("marketplace", "removed"),
            Outcome::Failed(e) | Outcome::NeedsConfirm(e) => {
                ctx.report
                    .fail("marketplace", &format!("remove failed: {e}"));
                return Err(EXIT_FAILED);
            }
        },
    }
    Ok(())
}

/// Step 4: `<ah>/bin`, the downloaded binary, but only once no guardrails plugin is left (the
/// guard needs it), and only after asking. The literal folder, never a glob; a symlink loses
/// only the link.
fn delete_binary(ctx: &mut Ctx) -> Result<(), i32> {
    let left = ctx.claude().plugins().map_err(|e| {
        ctx.report
            .fail("plugins", &format!("cannot read the plugin list: {e}"));
        EXIT_FAILED
    })?;
    if left
        .iter()
        .any(|p| p.id.starts_with("anthology-guardrails@"))
    {
        ctx.report.skip(
            "binary",
            "kept: an anthology-guardrails plugin is still installed and needs it",
        );
        return Ok(());
    }
    let bin = ctx.ah.join("bin");
    let Ok(meta) = fs::symlink_metadata(&bin) else {
        ctx.report.skip("binary", "no downloaded binary folder");
        return Ok(());
    };
    let question = format!(
        "Delete the downloaded anthology binary folder {}?",
        bin.display()
    );
    if !ctx.yes && !ctx.ask.confirm(&question, false) {
        ctx.report
            .skip("binary", &format!("kept {}", bin.display()));
        return Ok(());
    }
    let removed = if meta.is_dir() {
        fs::remove_dir_all(&bin)
    } else {
        fs::remove_file(&bin)
    };
    match removed {
        Ok(()) => ctx
            .report
            .ok("binary", &format!("deleted {}", bin.display())),
        Err(e) => {
            ctx.report
                .fail("binary", &format!("cannot delete {}: {e}", bin.display()));
            return Err(EXIT_FAILED);
        }
    }
    Ok(())
}

fn forget_state(ctx: &mut Ctx) {
    let path = ctx.ah.join(state::STATE_FILE);
    match fs::remove_file(&path) {
        Ok(()) => ctx.report.ok("state", "removed installer.json"),
        Err(_) => ctx.report.skip("state", "no installer.json"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, Answers, Buf, Script};

    const OK: &str = r#"{"command":"x","outcome":"ok","message":"done"}"#;

    fn plugin_list(ids: &[&str]) -> String {
        let items: Vec<String> = ids
            .iter()
            .map(|id| {
                format!(r#"{{"id":"{id}","version":"1","enabled":true,"installPath":"/p/x"}}"#)
            })
            .collect();
        format!("[{}]", items.join(","))
    }

    const ALL: [&str; 3] = [
        "anthology@anthology",
        "anthology-guardrails@anthology",
        "anthology-superpowers@anthology",
    ];

    fn programs(dir: &std::path::Path) {
        testing::programs(dir, &["claude"]);
    }

    #[test]
    fn without_yes_and_without_a_terminal_it_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        programs(dir.path());
        let script = Script::new();
        let (mut ask, buf) = (Answers::new(&[]), Buf::default());
        let mut ctx = testing::ctx(&script, &mut ask, dir.path(), &buf);
        assert_eq!(run(&mut ctx), EXIT_FAILED);
        assert!(buf.text().contains("--yes"), "{}", buf.text());
        assert!(script.calls().is_empty());
    }

    #[test]
    fn answering_no_at_a_terminal_cancels() {
        let dir = tempfile::tempdir().unwrap();
        programs(dir.path());
        let script = Script::new();
        let (mut ask, buf) = (Answers::new(&[false]), Buf::default());
        let mut ctx = testing::ctx(&script, &mut ask, dir.path(), &buf);
        ctx.interactive = true;
        assert_eq!(run(&mut ctx), 0);
        assert!(
            buf.text().contains("skip uninstall: cancelled"),
            "{}",
            buf.text()
        );
        assert!(script.calls().is_empty());
    }

    /// The commands of an uninstall with no setup to remove and the plugins listed as `before`.
    fn script_for(
        before: &[&str],
        after_prune: &[&str],
        after_all: &[&str],
        market: &str,
    ) -> Script {
        let mut script = Script::new().ok("claude plugin list --json", &plugin_list(before));
        if before.contains(&PLUGINS[0]) {
            script = script.ok(
                "claude plugin uninstall anthology@anthology --prune -y",
                "ok\n",
            );
        }
        script = script.ok("claude plugin list --json", &plugin_list(after_prune));
        for id in &PLUGINS[1..] {
            if after_prune.contains(id) {
                script = script.ok(&format!("claude plugin uninstall {id} --json"), OK);
            }
        }
        script
            .ok("claude plugin marketplace list --json", market)
            .ok("claude plugin marketplace remove anthology --json", OK)
            .ok("claude plugin list --json", &plugin_list(after_all))
    }

    const OURS: &str = r#"[{"name":"anthology","source":"github","repo":"rbonweb/anthology"}]"#;

    #[test]
    fn the_other_two_plugins_are_removed_only_if_they_are_still_listed() {
        let dir = tempfile::tempdir().unwrap();
        programs(dir.path());
        // --prune took guardrails with it; superpowers stayed.
        let script = script_for(&ALL, &["anthology-superpowers@anthology"], &[], OURS);
        let (mut ask, buf) = (Answers::new(&[]), Buf::default());
        let mut ctx = testing::ctx(&script, &mut ask, dir.path(), &buf);
        ctx.yes = true;
        assert_eq!(run(&mut ctx), 0, "{}", buf.text());
        script.assert_done();
        let text = buf.text();
        assert!(
            text.contains("skip plugin-anthology-guardrails: already gone"),
            "{text}"
        );
        assert!(
            text.contains("ok plugin-anthology-superpowers: uninstalled"),
            "{text}"
        );
        assert!(text.contains("ok marketplace: removed"), "{text}");
    }

    #[test]
    fn the_binary_folder_stays_while_a_guardrails_plugin_is_listed() {
        let dir = tempfile::tempdir().unwrap();
        programs(dir.path());
        std::fs::create_dir_all(dir.path().join("anthology/bin/0.2.5")).unwrap();
        // Some other marketplace's guardrails fork is still installed.
        let script = script_for(&ALL, &[], &["anthology-guardrails@fork"], OURS);
        let (mut ask, buf) = (Answers::new(&[]), Buf::default());
        let mut ctx = testing::ctx(&script, &mut ask, dir.path(), &buf);
        ctx.yes = true;
        assert_eq!(run(&mut ctx), 0, "{}", buf.text());
        assert!(dir.path().join("anthology/bin").exists());
        assert!(buf.text().contains("skip binary: kept"), "{}", buf.text());
    }

    #[test]
    fn the_binary_folder_is_deleted_only_after_asking() {
        let dir = tempfile::tempdir().unwrap();
        programs(dir.path());
        std::fs::create_dir_all(dir.path().join("anthology/bin/0.2.5")).unwrap();
        // Confirm the uninstall, then decline the folder.
        let script = script_for(&ALL, &[], &[], OURS);
        let (mut ask, buf) = (Answers::new(&[true, false]), Buf::default());
        let mut ctx = testing::ctx(&script, &mut ask, dir.path(), &buf);
        ctx.interactive = true;
        assert_eq!(run(&mut ctx), 0, "{}", buf.text());
        assert!(dir.path().join("anthology/bin").exists());
        assert!(ask.asked[1].contains("anthology/bin"), "{:?}", ask.asked);
        assert!(buf.text().contains("skip binary: kept"), "{}", buf.text());
    }

    #[test]
    fn a_symlinked_binary_folder_loses_only_the_link() {
        let dir = tempfile::tempdir().unwrap();
        programs(dir.path());
        let target = dir.path().join("elsewhere");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("keep.txt"), "x").unwrap();
        std::fs::create_dir_all(dir.path().join("anthology")).unwrap();
        std::os::unix::fs::symlink(&target, dir.path().join("anthology/bin")).unwrap();
        let script = script_for(&ALL, &[], &[], OURS);
        let (mut ask, buf) = (Answers::new(&[]), Buf::default());
        let mut ctx = testing::ctx(&script, &mut ask, dir.path(), &buf);
        ctx.yes = true;
        assert_eq!(run(&mut ctx), 0, "{}", buf.text());
        assert!(!dir.path().join("anthology/bin").exists());
        assert!(
            target.join("keep.txt").exists(),
            "the target of the link is untouched"
        );
    }

    #[test]
    fn another_sources_marketplace_is_never_removed() {
        let dir = tempfile::tempdir().unwrap();
        programs(dir.path());
        let foreign = r#"[{"name":"anthology","source":"directory","path":"/w"}]"#;
        let script = Script::new()
            .ok("claude plugin list --json", &plugin_list(&[]))
            .ok("claude plugin list --json", &plugin_list(&[]))
            .ok("claude plugin marketplace list --json", foreign)
            .ok("claude plugin list --json", &plugin_list(&[]));
        let (mut ask, buf) = (Answers::new(&[]), Buf::default());
        let mut ctx = testing::ctx(&script, &mut ask, dir.path(), &buf);
        ctx.yes = true;
        assert_eq!(run(&mut ctx), 0, "{}", buf.text());
        assert!(
            buf.text()
                .contains("skip marketplace: comes from another source"),
            "{}",
            buf.text()
        );
        assert!(
            !script
                .calls()
                .iter()
                .any(|c| c.contains("marketplace remove"))
        );
    }

    #[test]
    fn a_failed_plugin_uninstall_stops_before_the_marketplace_goes() {
        let dir = tempfile::tempdir().unwrap();
        programs(dir.path());
        let script = Script::new()
            .ok("claude plugin list --json", &plugin_list(&ALL))
            .exit(
                "claude plugin uninstall anthology@anthology --prune -y",
                1,
                "",
                "locked",
            );
        let (mut ask, buf) = (Answers::new(&[]), Buf::default());
        let mut ctx = testing::ctx(&script, &mut ask, dir.path(), &buf);
        ctx.yes = true;
        assert_eq!(run(&mut ctx), EXIT_FAILED);
        assert!(
            buf.text()
                .contains("fail plugin-anthology: uninstall failed: locked"),
            "{}",
            buf.text()
        );
        script.assert_done();
    }
}
