//! `anthology-installer status`: what is installed. Changes nothing.

use crate::binary;
use crate::claude::{MARKETPLACE, PLUGINS};
use crate::ctx::Ctx;
use crate::platform;
use crate::proc::find_in_path;
use crate::state;
use crate::steps::{EXIT_FAILED, EXIT_UNSUPPORTED, plugin_step};

/// 0 everything is there, 1 something is missing, 2 unsupported platform, 3 everything is there
/// but Claude Code is too old to load a plugin.
pub fn run(ctx: &mut Ctx) -> i32 {
    if let Err(why) = platform::detect(ctx.runner, &ctx.osrelease) {
        ctx.report.fail("preflight", &why.message());
        return EXIT_UNSUPPORTED;
    }
    if find_in_path("claude", &ctx.path).is_none() {
        ctx.report.fail("preflight", "claude not found on PATH");
        return EXIT_FAILED;
    }
    let claude = ctx.claude();
    let mut missing = false;
    let mut too_old = false;
    match claude.marketplaces() {
        Err(e) => {
            ctx.report.fail("marketplace", &e);
            missing = true;
        }
        Ok(list) => match list.iter().find(|m| m.name == MARKETPLACE) {
            None => {
                ctx.report.fail("marketplace", "not configured");
                missing = true;
            }
            Some(m) if m.source.is_ours() => ctx.report.ok(
                "marketplace",
                &format!("{MARKETPLACE} ({})", m.source.describe()),
            ),
            Some(m) => ctx.report.skip(
                "marketplace",
                &format!(
                    "{MARKETPLACE} comes from another source ({})",
                    m.source.describe()
                ),
            ),
        },
    }
    match claude.plugins() {
        Err(e) => {
            ctx.report.fail("plugins", &e);
            missing = true;
        }
        Ok(list) => {
            for id in PLUGINS {
                let step = plugin_step(id);
                let Some(plugin) = list.iter().find(|p| p.id == id) else {
                    ctx.report.fail(&step, "not installed");
                    missing = true;
                    continue;
                };
                let version = plugin.version.as_deref().unwrap_or("unknown version");
                if plugin.engine_unsatisfied {
                    too_old = true;
                    let why = plugin
                        .errors
                        .first()
                        .map_or("needs a newer Claude Code", String::as_str);
                    ctx.report
                        .fail(&step, &format!("{version}, not loaded: {why}"));
                } else {
                    let enabled = if plugin.enabled {
                        "enabled"
                    } else {
                        "disabled"
                    };
                    ctx.report.ok(&step, &format!("{version}, {enabled}"));
                }
            }
        }
    }
    if binary::installed(&ctx.ah) {
        ctx.report.ok("binary", "installed");
    } else {
        ctx.report.fail("binary", "not installed");
        missing = true;
    }
    match state::read(&ctx.ah) {
        Some(s) => ctx.report.ok(
            "state",
            &format!(
                "recorded {} by installer {}",
                s.updated_at, s.installer_version
            ),
        ),
        None => ctx.report.skip(
            "state",
            "no installer.json: not installed by this installer",
        ),
    }
    if missing {
        EXIT_FAILED
    } else if too_old {
        3
    } else {
        0
    }
}
