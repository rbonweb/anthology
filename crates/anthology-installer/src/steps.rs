//! The first four install steps: preflight, marketplace, plugins and the engine check.
//!
//! A step reports its own line. `Err` means it failed (the line is already printed) and the
//! install stops there; the value says which exit code that is.

use crate::binary;
use crate::claude::{MARKETPLACE, MARKETPLACE_SOURCE, Marketplace, Outcome, PLUGINS, Plugin};
use crate::ctx::Ctx;
use crate::platform::{self, Platform};
use crate::proc::{Cmd, find_in_path};

/// Where to install Claude Code from.
pub const CLAUDE_INSTALL_URL: &str = "https://code.claude.com/docs/en/setup";

pub const EXIT_FAILED: i32 = 1;
pub const EXIT_UNSUPPORTED: i32 = 2;

/// Step 1. Is this a supported machine with `claude` and `git`, and is it safe to go on?
pub fn preflight(ctx: &mut Ctx) -> Result<Platform, i32> {
    let platform = match platform::detect(ctx.runner, &ctx.osrelease) {
        Ok(platform) => platform,
        Err(why) => {
            ctx.report.fail("preflight", &why.message());
            return Err(EXIT_UNSUPPORTED);
        }
    };
    if find_in_path("claude", &ctx.path).is_none() {
        let detail = format!(
            "claude not found on PATH; install Claude Code first ({CLAUDE_INSTALL_URL}), \
             then run this again. The installer never installs it for you"
        );
        ctx.report.fail("preflight", &detail);
        return Err(EXIT_FAILED);
    }
    if find_in_path("git", &ctx.path).is_none() {
        ctx.report.fail(
            "preflight",
            "git not found on PATH; Claude Code needs it to add the marketplace",
        );
        return Err(EXIT_FAILED);
    }
    let version = match ctx.runner.run(&Cmd::new("claude", &["--version"])) {
        Ok(out) if out.success() => out.stdout.trim().to_string(),
        _ => {
            ctx.report.fail(
                "preflight",
                "claude --version failed; is Claude Code working?",
            );
            return Err(EXIT_FAILED);
        }
    };
    if ctx.in_claude && !binary::installed(&ctx.ah) {
        ctx.report.fail(
            "preflight",
            "this is a Claude Code session and the anthology binary is not installed yet, \
             so the guard would block what comes next; run it in a terminal",
        );
        return Err(EXIT_FAILED);
    }
    let detail = format!("{}, claude {version}, git found", platform.describe());
    ctx.report.ok("preflight", &detail);
    Ok(platform)
}

/// What the marketplace step found.
#[derive(Debug, PartialEq, Eq)]
pub struct MarketFacts {
    /// This run added it (it was not configured before).
    pub added: bool,
    /// An `anthology` marketplace from somewhere else is configured and was left alone.
    pub foreign: bool,
}

/// What to do about the marketplace, from what is configured.
#[derive(Debug, PartialEq, Eq)]
pub enum MarketAction {
    Add,
    Update,
    /// Another source owns the name: say which.
    LeaveAlone(String),
}

pub fn market_action(configured: &[Marketplace]) -> MarketAction {
    match configured.iter().find(|m| m.name == MARKETPLACE) {
        None => MarketAction::Add,
        Some(m) if m.source.is_ours() => MarketAction::Update,
        Some(m) => MarketAction::LeaveAlone(m.source.describe()),
    }
}

/// A failed action as one step line; `what` is `could not add rbonweb/anthology`. A command the
/// marketplace wants confirmed is shown, never accepted: `yourself` is what to run by hand.
fn fail_action(ctx: &mut Ctx, step: &str, what: &str, yourself: &str, outcome: Outcome) -> i32 {
    let detail = match outcome {
        Outcome::NeedsConfirm(command) => format!(
            "the marketplace declares a command that needs your confirmation, so nothing was \
             run: {command} -- read it, then run `{yourself}` yourself"
        ),
        Outcome::Failed(message) => format!("{what}: {message}"),
        Outcome::Done { .. } => unreachable!("a finished action is not a failure"),
    };
    ctx.report.fail(step, &detail);
    EXIT_FAILED
}

/// Step 2.
pub fn marketplace(ctx: &mut Ctx) -> Result<MarketFacts, i32> {
    let claude = ctx.claude();
    let configured = match claude.marketplaces() {
        Ok(list) => list,
        Err(e) => {
            ctx.report.fail(
                "marketplace",
                &format!("cannot read the marketplace list: {e}"),
            );
            return Err(EXIT_FAILED);
        }
    };
    let (outcome, done, what, yourself, facts) = match market_action(&configured) {
        MarketAction::LeaveAlone(source) => {
            let detail = format!(
                "an `{MARKETPLACE}` marketplace from another source ({source}) is configured; \
                 leaving it alone, the plugins come from it"
            );
            ctx.report.skip("marketplace", &detail);
            return Ok(MarketFacts {
                added: false,
                foreign: true,
            });
        }
        MarketAction::Add => (
            claude.marketplace_add(MARKETPLACE_SOURCE),
            format!("added {MARKETPLACE_SOURCE}"),
            format!("could not add {MARKETPLACE_SOURCE}"),
            format!("claude plugin marketplace add {MARKETPLACE_SOURCE}"),
            MarketFacts {
                added: true,
                foreign: false,
            },
        ),
        MarketAction::Update => (
            claude.marketplace_update(MARKETPLACE),
            format!("updated {MARKETPLACE}"),
            format!("could not update {MARKETPLACE}"),
            format!("claude plugin marketplace update {MARKETPLACE}"),
            MarketFacts {
                added: false,
                foreign: false,
            },
        ),
    };
    match outcome {
        Outcome::Done { .. } => {
            ctx.report.ok("marketplace", &done);
            Ok(facts)
        }
        failed => Err(fail_action(ctx, "marketplace", &what, &yourself, failed)),
    }
}

/// What to do about one plugin, from what is installed.
#[derive(Debug, PartialEq, Eq)]
pub enum PluginAction {
    Install,
    Update,
}

pub fn plugin_action(installed: &[Plugin], id: &str) -> PluginAction {
    if installed.iter().any(|p| p.id == id) {
        PluginAction::Update
    } else {
        PluginAction::Install
    }
}

/// `anthology-guardrails@anthology` -> `plugin-anthology-guardrails`: a step name has no `@`.
pub fn plugin_step(id: &str) -> String {
    format!("plugin-{}", id.split('@').next().unwrap_or(id))
}

/// Step 3: install what is missing and update the rest, one plugin at a time. The list is read
/// again after each one, because installing `anthology` installs the other two with it.
pub fn plugins(ctx: &mut Ctx) -> Result<(), i32> {
    let claude = ctx.claude();
    let read = |ctx: &mut Ctx| {
        claude.plugins().map_err(|e| {
            ctx.report
                .fail("plugins", &format!("cannot read the plugin list: {e}"));
            EXIT_FAILED
        })
    };
    let mut installed = read(ctx)?;
    for id in PLUGINS {
        let step = plugin_step(id);
        let (outcome, verb) = match plugin_action(&installed, id) {
            PluginAction::Install => (claude.plugin_install(id), "install"),
            PluginAction::Update => (claude.plugin_update(id), "update"),
        };
        let Outcome::Done { detail } = outcome else {
            let what = format!("{verb} failed");
            let yourself = format!("claude plugin {verb} {id}");
            return Err(fail_action(ctx, &step, &what, &yourself, outcome));
        };
        installed = read(ctx)?;
        let version = installed
            .iter()
            .find(|p| p.id == id)
            .and_then(|p| p.version.clone());
        let line = match (verb, version) {
            ("install", Some(v)) => format!("installed ({v})"),
            ("install", None) => "installed".to_string(),
            _ => detail,
        };
        ctx.report.ok(&step, &line);
    }
    Ok(())
}

/// What the engine check found.
#[derive(Debug)]
pub struct Engine {
    pub plugins: Vec<Plugin>,
    /// Claude Code is older than a plugin's `engines` range: the install is done, but the plugin
    /// does not load until `claude update`.
    pub too_old: bool,
}

/// Step 4.
pub fn engine(ctx: &mut Ctx) -> Result<Engine, i32> {
    let plugins = match ctx.claude().plugins() {
        Ok(list) => list,
        Err(e) => {
            ctx.report
                .fail("engine", &format!("cannot read the plugin list: {e}"));
            return Err(EXIT_FAILED);
        }
    };
    let old: Vec<&Plugin> = plugins
        .iter()
        .filter(|p| PLUGINS.contains(&p.id.as_str()) && p.engine_unsatisfied)
        .collect();
    if old.is_empty() {
        ctx.report
            .ok("engine", "Claude Code is new enough for every plugin");
        return Ok(Engine {
            plugins,
            too_old: false,
        });
    }
    let mut message = old
        .iter()
        .flat_map(|p| p.errors.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ");
    if message.is_empty() {
        message = format!("{} needs a newer Claude Code", old[0].id);
    }
    if !message.to_lowercase().contains("claude update") {
        message.push_str("; run `claude update`");
    }
    ctx.report.fail("engine", &message);
    Ok(Engine {
        plugins,
        too_old: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::Status;
    use crate::testing::{self, Answers, Buf, Script};
    use std::fs;

    fn version_line(v: &str) -> String {
        format!("{v} (Claude Code)\n")
    }

    /// `[{id, version, installPath}]` for `(name@marketplace, version)` pairs.
    fn list(items: &[(&str, &str)]) -> String {
        let entries: Vec<String> = items
            .iter()
            .map(|(id, v)| {
                let name = id.split('@').next().unwrap();
                format!(
                    r#"{{"id":"{id}","version":"{v}","enabled":true,"installPath":"/p/{name}"}}"#
                )
            })
            .collect();
        format!("[{}]", entries.join(","))
    }

    const MARKET_OURS: &str = r#"[{"name":"anthology","source":"github","repo":"rbonweb/anthology","installLocation":"/m"}]"#;
    const MARKET_FOREIGN: &str = r#"[{"name":"anthology","source":"directory","path":"/work/anthology","installLocation":"/work/anthology"}]"#;
    const OK: &str = r#"{"command":"x","outcome":"ok","message":"done"}"#;

    fn lines(buf: &Buf) -> Vec<String> {
        buf.text().lines().map(str::to_string).collect()
    }

    // --- preflight --------------------------------------------------------------------------

    fn uname(script: Script, s: &str, m: &str) -> Script {
        script
            .ok("uname -s", &format!("{s}\n"))
            .ok("uname -m", &format!("{m}\n"))
    }

    #[test]
    fn preflight_names_the_machine_and_the_claude_version() {
        let dir = tempfile::tempdir().unwrap();
        testing::programs(dir.path(), &["claude", "git"]);
        let script = uname(Script::new(), "Darwin", "arm64")
            .ok("claude --version", &version_line("2.1.330"));
        let (mut ask, buf) = (Answers::new(&[]), Buf::default());
        let mut ctx = testing::ctx(&script, &mut ask, dir.path(), &buf);
        let platform = preflight(&mut ctx).unwrap();
        assert_eq!(platform.describe(), "Darwin arm64");
        assert_eq!(
            lines(&buf),
            ["ok preflight: Darwin arm64, claude 2.1.330 (Claude Code), git found"]
        );
    }

    #[test]
    fn preflight_refuses_native_windows_with_exit_2_and_a_pointer_to_wsl() {
        let dir = tempfile::tempdir().unwrap();
        let script = uname(Script::new(), "MINGW64_NT-10.0", "x86_64");
        let (mut ask, buf) = (Answers::new(&[]), Buf::default());
        let mut ctx = testing::ctx(&script, &mut ask, dir.path(), &buf);
        assert_eq!(preflight(&mut ctx).unwrap_err(), EXIT_UNSUPPORTED);
        assert!(buf.text().starts_with("fail preflight: "));
        assert!(buf.text().contains("WSL"));
    }

    #[test]
    fn preflight_without_claude_prints_the_official_install_link_and_stops() {
        let dir = tempfile::tempdir().unwrap();
        testing::programs(dir.path(), &["git"]);
        let script = uname(Script::new(), "Linux", "x86_64");
        let (mut ask, buf) = (Answers::new(&[]), Buf::default());
        let mut ctx = testing::ctx(&script, &mut ask, dir.path(), &buf);
        assert_eq!(preflight(&mut ctx).unwrap_err(), EXIT_FAILED);
        assert!(buf.text().contains(CLAUDE_INSTALL_URL), "{}", buf.text());
        assert!(buf.text().contains("claude"));
        // The installer never installs Claude Code itself: nothing but uname ran.
        assert_eq!(script.calls(), ["uname -s", "uname -m"]);
    }

    #[test]
    fn preflight_without_git_stops() {
        let dir = tempfile::tempdir().unwrap();
        testing::programs(dir.path(), &["claude"]);
        let script = uname(Script::new(), "Linux", "aarch64");
        let (mut ask, buf) = (Answers::new(&[]), Buf::default());
        let mut ctx = testing::ctx(&script, &mut ask, dir.path(), &buf);
        assert_eq!(preflight(&mut ctx).unwrap_err(), EXIT_FAILED);
        assert!(
            buf.text().starts_with("fail preflight: git not found"),
            "{}",
            buf.text()
        );
    }

    #[test]
    fn preflight_fails_when_claude_does_not_run() {
        let dir = tempfile::tempdir().unwrap();
        testing::programs(dir.path(), &["claude", "git"]);
        let script =
            uname(Script::new(), "Linux", "x86_64").exit("claude --version", 1, "", "broken");
        let (mut ask, buf) = (Answers::new(&[]), Buf::default());
        let mut ctx = testing::ctx(&script, &mut ask, dir.path(), &buf);
        assert_eq!(preflight(&mut ctx).unwrap_err(), EXIT_FAILED);
        assert!(
            buf.text().contains("claude --version failed"),
            "{}",
            buf.text()
        );
    }

    #[test]
    fn inside_a_claude_session_without_the_binary_it_says_run_it_in_a_terminal() {
        let dir = tempfile::tempdir().unwrap();
        testing::programs(dir.path(), &["claude", "git"]);
        let script = uname(Script::new(), "Linux", "x86_64")
            .ok("claude --version", &version_line("2.1.330"));
        let (mut ask, buf) = (Answers::new(&[]), Buf::default());
        let mut ctx = testing::ctx(&script, &mut ask, dir.path(), &buf);
        ctx.in_claude = true;
        assert_eq!(preflight(&mut ctx).unwrap_err(), EXIT_FAILED);
        assert!(
            buf.text().contains("run it in a terminal"),
            "{}",
            buf.text()
        );
    }

    #[test]
    fn inside_a_claude_session_with_the_binary_an_upgrade_may_go_on() {
        let dir = tempfile::tempdir().unwrap();
        testing::programs(dir.path(), &["claude", "git"]);
        fs::create_dir_all(dir.path().join("anthology/bin/0.2.5")).unwrap();
        fs::write(dir.path().join("anthology/bin/0.2.5/anthology"), "x").unwrap();
        let script = uname(Script::new(), "Linux", "x86_64")
            .ok("claude --version", &version_line("2.1.330"));
        let (mut ask, buf) = (Answers::new(&[]), Buf::default());
        let mut ctx = testing::ctx(&script, &mut ask, dir.path(), &buf);
        ctx.in_claude = true;
        assert!(preflight(&mut ctx).is_ok());
    }

    // --- marketplace ------------------------------------------------------------------------

    fn parsed(text: &str) -> Vec<Marketplace> {
        crate::claude::parse_marketplaces(text).unwrap()
    }

    #[test]
    fn the_marketplace_action_follows_who_owns_the_name() {
        assert_eq!(market_action(&parsed("[]")), MarketAction::Add);
        assert_eq!(market_action(&parsed(MARKET_OURS)), MarketAction::Update);
        let other = r#"[{"name":"claude-plugins-official","source":"github","repo":"anthropics/claude-plugins-official"}]"#;
        assert_eq!(market_action(&parsed(other)), MarketAction::Add);
        assert_eq!(
            market_action(&parsed(MARKET_FOREIGN)),
            MarketAction::LeaveAlone("directory /work/anthology".into())
        );
    }

    #[test]
    fn a_missing_marketplace_is_added() {
        let dir = tempfile::tempdir().unwrap();
        let script = Script::new()
            .ok("claude plugin marketplace list --json", "[]")
            .ok("claude plugin marketplace add rbonweb/anthology --json", OK);
        let (mut ask, buf) = (Answers::new(&[]), Buf::default());
        let mut ctx = testing::ctx(&script, &mut ask, dir.path(), &buf);
        let facts = marketplace(&mut ctx).unwrap();
        assert_eq!(
            facts,
            MarketFacts {
                added: true,
                foreign: false
            }
        );
        assert_eq!(lines(&buf), ["ok marketplace: added rbonweb/anthology"]);
        script.assert_done();
    }

    #[test]
    fn our_marketplace_is_updated() {
        let dir = tempfile::tempdir().unwrap();
        let script = Script::new()
            .ok("claude plugin marketplace list --json", MARKET_OURS)
            .ok("claude plugin marketplace update anthology --json", OK);
        let (mut ask, buf) = (Answers::new(&[]), Buf::default());
        let mut ctx = testing::ctx(&script, &mut ask, dir.path(), &buf);
        let facts = marketplace(&mut ctx).unwrap();
        assert_eq!(
            facts,
            MarketFacts {
                added: false,
                foreign: false
            }
        );
        assert_eq!(lines(&buf), ["ok marketplace: updated anthology"]);
    }

    #[test]
    fn another_sources_marketplace_is_warned_about_and_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let script = Script::new().ok("claude plugin marketplace list --json", MARKET_FOREIGN);
        let (mut ask, buf) = (Answers::new(&[]), Buf::default());
        let mut ctx = testing::ctx(&script, &mut ask, dir.path(), &buf);
        let facts = marketplace(&mut ctx).unwrap();
        assert_eq!(
            facts,
            MarketFacts {
                added: false,
                foreign: true
            }
        );
        let out = buf.text();
        assert!(out.starts_with("skip marketplace: "), "{out}");
        assert!(out.contains("directory /work/anthology"), "{out}");
        assert!(out.contains("leaving it alone"), "{out}");
        script.assert_done();
    }

    #[test]
    fn a_marketplace_that_cannot_be_added_stops_the_install() {
        let dir = tempfile::tempdir().unwrap();
        let script = Script::new()
            .ok("claude plugin marketplace list --json", "[]")
            .exit(
                "claude plugin marketplace add",
                1,
                r#"{"command":"marketplace-add","outcome":"failed","message":"clone failed"}"#,
                "x clone failed",
            );
        let (mut ask, buf) = (Answers::new(&[]), Buf::default());
        let mut ctx = testing::ctx(&script, &mut ask, dir.path(), &buf);
        assert_eq!(marketplace(&mut ctx).unwrap_err(), EXIT_FAILED);
        assert_eq!(
            lines(&buf),
            ["fail marketplace: could not add rbonweb/anthology: clone failed"]
        );
    }

    #[test]
    fn an_unreadable_marketplace_list_is_a_failure() {
        let dir = tempfile::tempdir().unwrap();
        let script = Script::new().ok("claude plugin marketplace list --json", "oops");
        let (mut ask, buf) = (Answers::new(&[]), Buf::default());
        let mut ctx = testing::ctx(&script, &mut ask, dir.path(), &buf);
        assert_eq!(marketplace(&mut ctx).unwrap_err(), EXIT_FAILED);
        assert!(
            buf.text().starts_with("fail marketplace: "),
            "{}",
            buf.text()
        );
    }

    // --- plugins ----------------------------------------------------------------------------

    fn parsed_plugins(items: &[(&str, &str)]) -> Vec<Plugin> {
        crate::claude::parse_plugins(&list(items)).unwrap()
    }

    #[test]
    fn a_plugin_is_installed_when_missing_and_updated_when_present() {
        let have = parsed_plugins(&[("anthology-guardrails@anthology", "0.2.15")]);
        assert_eq!(
            plugin_action(&have, "anthology@anthology"),
            PluginAction::Install
        );
        assert_eq!(
            plugin_action(&have, "anthology-guardrails@anthology"),
            PluginAction::Update
        );
        // The same plugin name from another marketplace is not ours.
        let other = parsed_plugins(&[("anthology@elsewhere", "1")]);
        assert_eq!(
            plugin_action(&other, "anthology@anthology"),
            PluginAction::Install
        );
    }

    #[test]
    fn plugins_are_installed_or_updated_one_by_one_with_a_fresh_list_after_each() {
        let dir = tempfile::tempdir().unwrap();
        let none = list(&[]);
        let all = list(&[
            ("anthology@anthology", "0.5.5"),
            ("anthology-guardrails@anthology", "0.2.15"),
            ("anthology-superpowers@anthology", "0.2.3"),
        ]);
        let update = |v: &str| {
            format!(
                r#"{{"command":"update","outcome":"ok","message":"m","oldVersion":"{v}","newVersion":"{v}"}}"#
            )
        };
        let script = Script::new()
            .ok("claude plugin list --json", &none)
            .ok("claude plugin install anthology@anthology --json", OK)
            // Installing anthology brought the other two.
            .ok("claude plugin list --json", &all)
            .ok(
                "claude plugin update anthology-guardrails@anthology --json",
                &update("0.2.15"),
            )
            .ok("claude plugin list --json", &all)
            .ok(
                "claude plugin update anthology-superpowers@anthology --json",
                &update("0.2.3"),
            )
            .ok("claude plugin list --json", &all);
        let (mut ask, buf) = (Answers::new(&[]), Buf::default());
        let mut ctx = testing::ctx(&script, &mut ask, dir.path(), &buf);
        plugins(&mut ctx).unwrap();
        assert_eq!(
            lines(&buf),
            [
                "ok plugin-anthology: installed (0.5.5)",
                "ok plugin-anthology-guardrails: up to date (0.2.15)",
                "ok plugin-anthology-superpowers: up to date (0.2.3)",
            ]
        );
        script.assert_done();
        assert!(
            script
                .calls()
                .iter()
                .all(|c| !c.contains(" -y") && !c.contains("--accept-command"))
        );
    }

    #[test]
    fn the_first_failing_plugin_stops_the_run() {
        let dir = tempfile::tempdir().unwrap();
        let script = Script::new()
            .ok("claude plugin list --json", &list(&[]))
            .exit(
                "claude plugin install anthology@anthology",
                1,
                r#"{"command":"install","outcome":"failed","message":"not found","failureCode":"not_found"}"#,
                "",
            );
        let (mut ask, buf) = (Answers::new(&[]), Buf::default());
        let mut ctx = testing::ctx(&script, &mut ask, dir.path(), &buf);
        assert_eq!(plugins(&mut ctx).unwrap_err(), EXIT_FAILED);
        assert_eq!(
            lines(&buf),
            ["fail plugin-anthology: install failed: not found"]
        );
        script.assert_done();
    }

    #[test]
    fn a_command_the_marketplace_declares_is_shown_and_never_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let shown = r#"{"command":"install","outcome":"failed","message":"confirm","shownCommand":{"sha256":"ab","command":"make install"}}"#;
        let script = Script::new()
            .ok("claude plugin list --json", &list(&[]))
            .exit("claude plugin install anthology@anthology", 1, shown, "");
        let (mut ask, buf) = (Answers::new(&[]), Buf::default());
        let mut ctx = testing::ctx(&script, &mut ask, dir.path(), &buf);
        assert_eq!(plugins(&mut ctx).unwrap_err(), EXIT_FAILED);
        let out = buf.text();
        assert!(out.contains("make install"), "{out}");
        assert!(
            out.contains("claude plugin install anthology@anthology"),
            "{out}"
        );
        script.assert_done();
    }

    // --- engine -----------------------------------------------------------------------------

    #[test]
    fn a_claude_that_is_too_old_is_reported_with_its_own_message_and_claude_update() {
        let dir = tempfile::tempdir().unwrap();
        let text = r#"[{"id":"anthology@anthology","version":"0.5.5","enabled":true,"installPath":"/p/a",
            "errors":["Plugin \"anthology\" not loaded: needs Claude Code >=2.1.330, this is 2.1.326."],
            "errorDetails":[{"type":"engine-range-unsatisfied","plugin":"anthology"}]}]"#;
        let script = Script::new().ok("claude plugin list --json", text);
        let (mut ask, buf) = (Answers::new(&[]), Buf::default());
        let mut ctx = testing::ctx(&script, &mut ask, dir.path(), &buf);
        let engine = engine(&mut ctx).unwrap();
        assert!(engine.too_old);
        assert_eq!(engine.plugins.len(), 1);
        let out = buf.text();
        assert!(out.starts_with("fail engine: "), "{out}");
        assert!(out.contains("needs Claude Code >=2.1.330"), "{out}");
        assert!(out.contains("run `claude update`"), "{out}");
        assert_eq!(ctx.report.steps()[0].status, Status::Fail);
    }

    #[test]
    fn the_cli_message_that_already_says_claude_update_is_not_repeated() {
        let dir = tempfile::tempdir().unwrap();
        let text = r#"[{"id":"anthology@anthology","errors":["needs Claude Code >=2.1.330. Run `claude update`"],
            "errorDetails":[{"type":"engine-range-unsatisfied"}]}]"#;
        let script = Script::new().ok("claude plugin list --json", text);
        let (mut ask, buf) = (Answers::new(&[]), Buf::default());
        let mut ctx = testing::ctx(&script, &mut ask, dir.path(), &buf);
        engine(&mut ctx).unwrap();
        assert_eq!(
            buf.text().to_lowercase().matches("claude update").count(),
            1
        );
    }

    #[test]
    fn a_claude_that_is_new_enough_passes_and_other_plugins_are_not_our_business() {
        let dir = tempfile::tempdir().unwrap();
        let text = r#"[{"id":"anthology@anthology","version":"0.5.5","installPath":"/p/a"},
            {"id":"other@x","errors":["x"],"errorDetails":[{"type":"engine-range-unsatisfied"}]}]"#;
        let script = Script::new().ok("claude plugin list --json", text);
        let (mut ask, buf) = (Answers::new(&[]), Buf::default());
        let mut ctx = testing::ctx(&script, &mut ask, dir.path(), &buf);
        let engine = engine(&mut ctx).unwrap();
        assert!(!engine.too_old);
        assert!(buf.text().starts_with("ok engine: "), "{}", buf.text());
    }
}
