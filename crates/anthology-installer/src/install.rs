//! `anthology-installer install`: the steps in order. Every step is idempotent, so running it
//! again upgrades.

use crate::binary;
use crate::claude::{MARKETPLACE_SOURCE, PLUGINS, Plugin};
use crate::ctx::Ctx;
use crate::setup::{self, SetupState};
use crate::state::{self, Lock, State};
use crate::steps::{self, EXIT_FAILED, MarketFacts};

/// The exit code: 0 done, 1 a step failed, 2 unsupported platform, 3 done but Claude Code is too
/// old for a plugin.
pub fn run(ctx: &mut Ctx) -> i32 {
    if let Err(code) = steps::preflight(ctx) {
        return code;
    }
    let _lock = match Lock::acquire(&ctx.ah, ctx.runner) {
        Ok(lock) => lock,
        Err(e) => {
            ctx.report.fail("lock", &e);
            return EXIT_FAILED;
        }
    };
    match in_order(ctx) {
        Ok(code) | Err(code) => code,
    }
}

fn in_order(ctx: &mut Ctx) -> Result<i32, i32> {
    let market = steps::marketplace(ctx)?;
    steps::plugins(ctx)?;
    let engine = steps::engine(ctx)?;
    let binary = binary::step(ctx, &engine.plugins)?;
    let setup = setup::step(ctx, &engine.plugins)?;
    record(ctx, &market, &engine.plugins, &binary, setup)?;
    Ok(if engine.too_old { 3 } else { 0 })
}

/// Step 8: what this run installed, in `installer.json`.
fn record(
    ctx: &mut Ctx,
    market: &MarketFacts,
    plugins: &[Plugin],
    binary: &str,
    setup: SetupState,
) -> Result<(), i32> {
    let before = state::read(&ctx.ah);
    let mut recorded: Vec<(String, String)> = PLUGINS
        .iter()
        .filter_map(|id| {
            let plugin = plugins.iter().find(|p| p.id == *id)?;
            Some((id.to_string(), plugin.version.clone().unwrap_or_default()))
        })
        .collect();
    recorded.sort();
    let state = State {
        installer_version: env!("CARGO_PKG_VERSION").to_string(),
        updated_at: state::now_utc(),
        marketplace: (!market.foreign).then(|| MARKETPLACE_SOURCE.to_string()),
        // Once the installer added it, a later run that finds it there did not un-add it.
        marketplace_added: !market.foreign
            && (market.added || before.is_some_and(|s| s.marketplace_added)),
        plugins: recorded,
        binary: binary.to_string(),
        setup: setup.word().to_string(),
    };
    match state::write(&ctx.ah, &state) {
        Ok(path) => {
            ctx.report
                .ok("state", &format!("recorded in {}", path.display()));
            Ok(())
        }
        Err(e) => {
            ctx.report
                .fail("state", &format!("cannot write installer.json: {e}"));
            Err(EXIT_FAILED)
        }
    }
}
