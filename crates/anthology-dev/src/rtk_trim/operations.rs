//! The trim, in order: the operations table.
//!
//! This table was generated once from `scripts/rtk_trim.py` (`operations()`) at `rust@043ef72d`
//! and is not retyped: the 235 entries keep the Python's text byte for byte. The Python stays the
//! oracle until CO2; a change to the Python table before then is re-generated here, never
//! hand-edited.

use super::Op;

/// Every operation of the trim, in order. Each anchor is exact text of the merged tree
/// (fork + upstream).
pub fn operations() -> Vec<Op<'static>> {
    vec![
        // ---- 1. BUSL crates and everything only they serve
        Op::Rm(&["engine", "plugin", "proxy", "examples"]),
        Op::Sub {
            path: "Cargo.toml",
            old: r#"members = [".", "text", "engine", "plugin"]"#,
            new: r#"members = [".", "text"]"#,
            done: None,
        },
        Op::Cut {
            path: "Cargo.toml",
            old: r#"default = ["engine"]
engine = ["dep:rtk-engine"]
example-plugins = ["engine", "dep:rtk-plugin"]
"#,
            gone: "dep:rtk-engine",
        },
        Op::Cut {
            path: "Cargo.toml",
            old: r#"rtk-engine = { path = "engine", optional = true }
rtk-plugin = { path = "plugin", optional = true }
"#,
            gone: "rtk-plugin = ",
        },
        Op::Sub {
            path: "Cargo.toml",
            old: r#"[lints.rust]
unsafe_code = "deny"
warnings = "deny"
"#,
            new: r#"[lints.rust]
unsafe_code = "deny"
warnings = "deny"
unexpected_cfgs = { level = "deny", check-cfg = ['cfg(feature, values("engine", "example-plugins"))'] }
dead_code = "allow"
"#,
            done: None,
        },
        Op::Rm(&[
            "src/example_plugins.rs",
            "src/core/call_context.rs",
            "src/core/classify.rs",
            "src/core/pipeline_cmd.rs",
            "src/core/pipeline_config.rs",
            "src/core/pipeline_log.rs",
            "src/core/pipeline_remote.rs",
            "src/core/plugins_cmd.rs",
            "src/core/task_cmd.rs",
            "src/hooks/posthook.rs",
            "src/hooks/session_start.rs",
            "src/hooks/hook_cmd",
        ]),
        // ---- 2. telemetry
        Op::Rm(&[
            "src/core/telemetry.rs",
            "src/core/telemetry_cmd.rs",
            "docs/TELEMETRY.md",
            "docs/guide/resources/telemetry.md",
        ]),
        Op::Cut {
            path: "src/core/mod.rs",
            old: r"pub mod telemetry;
pub mod telemetry_cmd;
",
            gone: "pub mod telemetry",
        },
        Op::Cut {
            path: "src/lib.rs",
            old: r"    /// Manage telemetry consent and data (RGPD/GDPR)
    Telemetry {
        #[command(subcommand)]
        command: core::telemetry_cmd::TelemetrySubcommand,
    },

",
            gone: "telemetry_cmd::TelemetrySubcommand",
        },
        Op::Cut {
            path: "src/lib.rs",
            old: r"        Commands::Telemetry { command } => {
            core::telemetry_cmd::run(&command)?;
            0
        }

",
            gone: "Commands::Telemetry",
        },
        Op::Cut {
            path: "src/lib.rs",
            old: r"    // Fire-and-forget telemetry ping (1/day, non-blocking)
    if pings_telemetry(&parsed) {
        core::telemetry::maybe_ping();
    }

",
            gone: "maybe_ping",
        },
        Op::Drop {
            path: "src/lib.rs",
            sig: "fn pings_telemetry(parsed: &Result<Cli, clap::Error>) -> bool {",
            gone: None,
        },
        Op::Drop {
            path: "src/lib.rs",
            sig: "    fn hook_commands_skip_the_telemetry_ping() {",
            gone: None,
        },
        Op::Drop {
            path: "src/lib.rs",
            sig: "    fn other_invocations_still_run_the_ping() {",
            gone: None,
        },
        Op::Cut {
            path: "src/hooks/init/mod.rs",
            old: r"        prompt_telemetry_consent()?;
",
            gone: "prompt_telemetry_consent()?",
        },
        Op::Drop {
            path: "src/hooks/init/mod.rs",
            sig: "pub fn save_telemetry_consent(accepted: bool) -> Result<()> {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/init/mod.rs",
            sig: "fn prompt_telemetry_consent() -> Result<()> {",
            gone: None,
        },
        Op::Cut {
            path: "src/core/test_isolation/mod.rs",
            old: r#"        "TELEMETRY_DISABLED_ENV" => crate::core::telemetry_cmd::TELEMETRY_DISABLED_ENV,
"#,
            gone: "telemetry_cmd::",
        },
        // ---- 3. hooks/init: Claude only
        Op::Rm(&[
            "src/hooks/init/antigravity.rs",
            "src/hooks/init/codex.rs",
            "src/hooks/init/copilot.rs",
            "src/hooks/init/cursor.rs",
            "src/hooks/init/droid.rs",
            "src/hooks/init/gemini.rs",
            "src/hooks/init/hermes.rs",
            "src/hooks/init/instructions_agents.rs",
            "src/hooks/init/opencode.rs",
            "src/hooks/init/pi.rs",
            "src/hooks/init/trae.rs",
            "src/hooks/init/vibe.rs",
        ]),
        Op::Sub {
            path: "src/hooks/init/mod.rs",
            old: r"mod agents_md;
pub(crate) mod antigravity;
pub(crate) mod claude;
pub(crate) mod codex;
mod copilot;
pub(crate) mod cursor;
pub(crate) mod droid;
pub(crate) mod gemini;
pub(crate) mod hermes;
mod instructions_agents;
pub(crate) mod opencode;
pub(crate) mod pi;
pub(crate) mod trae;
pub(crate) mod vibe;

// `agents_md` and `pi` hold helpers that several submodules share, so they are
// glob-imported and reach siblings through `use super::*`; the remaining submodules are
// imported by name here (and agent modules with hook probes are `pub(crate)` for `hook_check`).
use agents_md::*;
use claude::{
    hook_already_present, hook_gaps, remove_settings_entries, run_claude_md_mode, run_default_mode,
    run_hook_only_mode,
};
pub use codex::UninstallCodexOutcome;
use codex::{run_codex_mode, show_codex_config, uninstall_codex};
use cursor::{
    cursor_hook_already_present, install_cursor_hooks, remove_cursor_hooks, resolve_cursor_dir,
};
use gemini::uninstall_gemini;
use instructions_agents::{run_cline_mode, run_windsurf_mode};
use opencode::{
    opencode_plugin_path, remove_opencode_plugin, resolve_opencode_dir, run_opencode_only_mode,
};
use pi::*;

pub use antigravity::{run_antigravity_mode, uninstall_antigravity_mode};
pub use claude::claude_hook_gaps;
pub(crate) use copilot::{COPILOT_HOOK_JSON, copilot_user_dir};
pub use copilot::{run_copilot, run_copilot_global, uninstall_copilot, uninstall_copilot_global};
pub use droid::{run_droid_mode, uninstall_droid};
pub use gemini::run_gemini;
pub use hermes::{run_hermes_mode, uninstall_hermes};
pub use instructions_agents::{run_kilocode_mode, run_kimi_mode};
pub use pi::{run_omp_mode_with_patch_mode, run_pi_mode_with_patch_mode};
pub use trae::{run_trae_mode, uninstall_trae_mode};
pub use vibe::{run_vibe_mode, uninstall_vibe};
",
            new: r"mod agents_md;
pub(crate) mod claude;

// anthology rtk-trim: Claude Code is the only init target. `agents_md` holds the
// marker-block helpers claude.rs reaches through `use super::*`.
use agents_md::*;
use claude::{
    hook_already_present, hook_gaps, remove_settings_entries, run_claude_md_mode, run_default_mode,
    run_hook_only_mode,
};

pub use claude::claude_hook_gaps;
",
            done: None,
        },
        // ---- 3b. init::run -> Claude only
        Op::Sub {
            path: "src/hooks/init/mod.rs",
            old: r#"/// Main entry point for `rtk init`
#[allow(clippy::too_many_arguments)]
pub fn run(
    global: bool,
    install_claude: bool,
    install_opencode: bool,
    install_cursor: bool,
    install_windsurf: bool,
    install_cline: bool,
    claude_md: bool,
    hook_only: bool,
    codex: bool,
    patch_mode: PatchMode,
    ctx: InitContext,
) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    // Validation: Codex mode conflicts
    if codex {
        if install_opencode {
            anyhow::bail!("--codex cannot be combined with --opencode");
        }
        if claude_md {
            anyhow::bail!("--codex cannot be combined with --claude-md");
        }
        if hook_only {
            anyhow::bail!("--codex cannot be combined with --hook-only");
        }
        if matches!(patch_mode, PatchMode::Auto) {
            anyhow::bail!("--codex cannot be combined with --auto-patch");
        }
        if matches!(patch_mode, PatchMode::Skip) {
            anyhow::bail!("--codex cannot be combined with --no-patch");
        }
        run_codex_mode(global, ctx)?;
    } else {
        // Validation: Global-only features
        if install_opencode && !global {
            anyhow::bail!("OpenCode plugin is global-only. Use: rtk init -g --opencode");
        }

        if install_cursor && !global {
            anyhow::bail!("Cursor hooks are global-only. Use: rtk init -g --agent cursor");
        }

        if install_windsurf && !global {
            anyhow::bail!("Windsurf support is global-only. Use: rtk init -g --agent windsurf");
        }

        // The OpenCode plugin is an independent target: install it before any
        // agent so a Claude/Cursor failure cannot leave it missing, and vice
        // versa. Its error is reported after the agent's.
        let opencode_result = install_opencode.then(|| run_opencode_only_mode(ctx));
        let opencode_plugin = opencode_result
            .as_ref()
            .and_then(|result| result.as_ref().ok())
            .and_then(|path| path.as_deref());

        let agent_result = (|| -> Result<()> {
            if install_windsurf {
                run_windsurf_mode(ctx)?;
            } else if install_cline {
                run_cline_mode(ctx)?;
            } else {
                // Mode selection (Claude Code / OpenCode)
                match (install_claude, install_opencode, claude_md, hook_only) {
                    // The plugin was installed above, independently.
                    (false, true, _, _) => {}
                    (true, _, true, _) => run_claude_md_mode(global, ctx)?,
                    (true, _, false, true) => {
                        run_hook_only_mode(global, patch_mode, opencode_plugin, ctx)?
                    }
                    (true, _, false, false) => {
                        run_default_mode(global, patch_mode, opencode_plugin, ctx)?
                    }
                    (false, false, _, _) => {
                        if !install_cursor {
                            anyhow::bail!(
                                "at least one of install_claude or install_opencode must be true"
                            )
                        }
                    }
                }

                // Cursor hooks: on their own, or next to the OpenCode plugin with --opencode
                if install_cursor {
                    install_cursor_hooks(ctx)?;
                }
            }
            Ok(())
        })();

        match (opencode_result, agent_result) {
            (Some(Err(opencode_error)), Err(agent_error)) => {
                return Err(anyhow::anyhow!(
                    "OpenCode install failed: {opencode_error:#}\nAgent install failed: {agent_error:#}"
                ));
            }
            (Some(Err(error)), Ok(())) => {
                return Err(error.context("OpenCode install failed"));
            }
            (_, Err(error)) => return Err(error),
            _ => {}
        }
    }

    if !dry_run {
"#,
            new: r"/// Main entry point for `rtk init` (anthology rtk-trim: Claude Code only)
pub fn run(
    global: bool,
    claude_md: bool,
    hook_only: bool,
    patch_mode: PatchMode,
    ctx: InitContext,
) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    if claude_md {
        run_claude_md_mode(global, ctx)?;
    } else if hook_only {
        run_hook_only_mode(global, patch_mode, None, ctx)?;
    } else {
        run_default_mode(global, patch_mode, None, ctx)?;
    }

    if !dry_run {
",
            done: None,
        },
        // ---- 3c. init uninstall/show_config -> Claude only
        Op::Sub {
            path: "src/hooks/init/mod.rs",
            old: r#"    if codex {
        let outcome = uninstall_codex(global, ctx)?;
        if dry_run {
            print_dry_run_footer();
        }
        let code = match outcome {
            UninstallCodexOutcome::Clean => 0,
            UninstallCodexOutcome::HookRefused => 2,
        };
        return Ok(code);
    }

    if cursor {
        if !global {
            anyhow::bail!("Cursor uninstall only works with --global flag");
        }
        let cursor_removed = remove_cursor_hooks(ctx).context("Failed to remove Cursor hooks")?;
        if !cursor_removed.is_empty() {
            let header = if dry_run {
                "[dry-run] would uninstall RTK (Cursor):"
            } else {
                "RTK uninstalled (Cursor):"
            };
            println!("{}", header);
            for item in &cursor_removed {
                println!("  - {}", item);
            }
            if !dry_run {
                println!("\nRestart Cursor to apply changes.");
            }
        } else {
            println!("RTK Cursor support was not installed (nothing to remove)");
        }
        if dry_run {
            print_dry_run_footer();
        }
        return Ok(0);
    }

    if pi {
        uninstall_pi_with_patch_mode(global, patch_mode, ctx)?;
        return Ok(0);
    }

    if omp {
        uninstall_omp_with_patch_mode(global, patch_mode, ctx)?;
        return Ok(0);
    }

"#,
            new: r#"    // anthology rtk-trim: Claude Code is the only uninstall target.
    if gemini || codex || cursor || pi || omp {
        anyhow::bail!("only Claude Code is supported by this rtk build");
    }
    let _ = patch_mode;

"#,
            done: None,
        },
        Op::Cut {
            path: "src/hooks/init/mod.rs",
            old: r#"    // Also uninstall Gemini artifacts if --gemini or always (clean everything)
    if gemini {
        let gemini_removed = uninstall_gemini(ctx)?;
        removed.extend(gemini_removed);
        if !removed.is_empty() {
            let header = if dry_run {
                "[dry-run] would uninstall RTK (Gemini):"
            } else {
                "RTK uninstalled (Gemini):"
            };
            println!("{}", header);
            for item in &removed {
                println!("  - {}", item);
            }
            if !dry_run {
                println!("\nRestart Gemini CLI to apply changes.");
            }
        } else {
            println!("RTK Gemini support was not installed (nothing to remove)");
        }
        if dry_run {
            print_dry_run_footer();
        }
        return Ok(0);
    }

"#,
            gone: "uninstall_gemini(",
        },
        Op::Cut {
            path: "src/hooks/init/mod.rs",
            old: r#"    // 5. Remove OpenCode plugin
    let opencode_removed = remove_opencode_plugin(ctx)?;
    for path in opencode_removed {
        removed.push(format!("OpenCode plugin: {}", path.display()));
    }

    // 6. Remove Cursor hooks
    let cursor_removed = remove_cursor_hooks(ctx)?;
    removed.extend(cursor_removed);

"#,
            gone: "remove_opencode_plugin(",
        },
        Op::Sub {
            path: "src/hooks/init/mod.rs",
            old: r#"println!("\nRestart Claude Code, OpenCode, and Cursor (if used) to apply changes.");"#,
            new: r#"println!("\nRestart Claude Code to apply changes.");"#,
            done: None,
        },
        Op::Sub {
            path: "src/hooks/init/mod.rs",
            old: r"/// Show current rtk configuration
pub fn show_config(codex: bool, omp: bool) -> Result<()> {
    if omp {
        return show_omp_config();
    }
    if codex {
        return show_codex_config();
    }

    show_claude_config()
}
",
            new: r"/// Show current rtk configuration
pub fn show_config() -> Result<()> {
    show_claude_config()
}
",
            done: None,
        },
        // ---- 3d. init show_claude_config -> Claude only
        Op::Cut {
            path: "src/hooks/init/mod.rs",
            old: r#"    // Check OpenCode plugin
    match resolve_opencode_dir() {
        Ok(opencode_dir) => {
            let plugin = opencode_plugin_path(&opencode_dir);
            if plugin.exists() {
                println!("[ok] OpenCode: plugin installed ({})", plugin.display());
            } else {
                println!("[--] OpenCode: plugin not found");
            }
        }
        _ => println!("[--] OpenCode: config dir not found"),
    }

    // Check Cursor hooks
    match resolve_cursor_dir() {
        Ok(cursor_dir) => {
            let cursor_hook = cursor_dir.join(HOOKS_SUBDIR).join(REWRITE_HOOK_FILE);
            let cursor_hooks_json = cursor_dir.join(HOOKS_JSON);

            // Check for binary command in hooks.json first
            let cursor_binary_registered = if cursor_hooks_json.exists() {
                let content = fs::read_to_string(&cursor_hooks_json).unwrap_or_default();
                if let Ok(root) = from_json_str::<serde_json::Value>(&content) {
                    cursor_hook_already_present(&root)
                } else {
                    false
                }
            } else {
                false
            };

            if cursor_binary_registered {
                println!("[ok] Cursor hook: registered in hooks.json");
            } else if cursor_hook.exists() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let meta = fs::metadata(&cursor_hook)?;
                    let is_executable = meta.permissions().mode() & 0o111 != 0;

                    if !is_executable {
                        println!(
                            "[warn] Cursor hook: {} (legacy script, NOT executable)",
                            cursor_hook.display()
                        );
                    } else {
                        println!(
                            "[warn] Cursor hook: {} (legacy script — run `rtk init -g --agent cursor` to upgrade)",
                            cursor_hook.display()
                        );
                    }
                }

                #[cfg(not(unix))]
                {
                    println!(
                        "[warn] Cursor hook: {} (legacy script — run `rtk init -g --agent cursor` to upgrade)",
                        cursor_hook.display()
                    );
                }
            } else {
                println!("[--] Cursor hook: not found");
            }
        }
        _ => println!("[--] Cursor: home dir not found"),
    }

"#,
            gone: "resolve_opencode_dir()",
        },
        Op::Cut {
            path: "src/hooks/init/mod.rs",
            old: r#"    println!("  rtk init --codex            # Configure local AGENTS.md + RTK.md + hooks.json");
    println!("  rtk init -g --codex         # Configure global AGENTS.md + RTK.md + hooks.json");
    println!("  rtk init -g --opencode      # Claude setup + OpenCode plugin");
    println!("  rtk init -g --agent cursor  # Install Cursor Agent hooks");
"#,
            gone: "rtk init -g --agent cursor  #",
        },
        // ---- 4. lib.rs: `rtk init` CLI -> Claude only
        Op::Drop {
            path: "src/lib.rs",
            sig: "pub enum AgentTarget {",
            gone: None,
        },
        Op::Cut {
            path: "src/lib.rs",
            old: r"        /// Install OpenCode plugin (in addition to Claude Code)
        #[arg(long)]
        opencode: bool,

        /// Initialize for Gemini CLI instead of Claude Code
        #[arg(long)]
        gemini: bool,

        /// Target agent to install hooks for (default: claude)
        #[arg(long, value_enum)]
        agent: Option<AgentTarget>,

",
            gone: "agent: Option<AgentTarget>",
        },
        Op::Cut {
            path: "src/lib.rs",
            old: r"        /// Target Codex CLI (uses PreToolUse hook + AGENTS.md + RTK.md)
        #[arg(long)]
        codex: bool,

        /// Install GitHub Copilot integration (VS Code + CLI)
        #[arg(long)]
        copilot: bool,
",
            gone: "copilot: bool,",
        },
        Op::Drop {
            path: "src/lib.rs",
            sig: "fn uninstall_init_dispatch<UninstallHermes, UninstallStandard>(",
            gone: None,
        },
        Op::Sub {
            path: "src/lib.rs",
            old: r"        Commands::Init {
            global,
            opencode,
            gemini,
            agent,
            show,
",
            new: r"        Commands::Init {
            global,
            show,
",
            done: None,
        },
        Op::Sub {
            path: "src/lib.rs",
            old: r"            uninstall,
            codex,
            copilot,
            dry_run,
        } => {",
            new: r"            uninstall,
            dry_run,
        } => {",
            done: None,
        },
        Op::Sub {
            path: "src/lib.rs",
            old: r#"            if show {
                hooks::init::show_config(codex, agent == Some(AgentTarget::Omp))?;
            } else if uninstall && copilot {
                if global {
                    hooks::init::uninstall_copilot_global(ctx)?;
                } else {
                    hooks::init::uninstall_copilot(ctx)?;
                }
            } else if uninstall {
                return uninstall_init_dispatch(
                    agent,
                    global,
                    gemini,
                    codex,
                    patch_mode,
                    ctx,
                    hooks::init::uninstall_hermes,
                    hooks::init::uninstall_with_patch_mode,
                );
            } else if gemini {
                hooks::init::run_gemini(global, hook_only, patch_mode, ctx)?;
            } else if copilot {
                if global {
                    hooks::init::run_copilot_global(ctx)?;
                } else {
                    hooks::init::run_copilot(ctx)?;
                }
            } else if agent == Some(AgentTarget::Pi) {
                hooks::init::run_pi_mode_with_patch_mode(global, patch_mode, ctx)?
            } else if agent == Some(AgentTarget::Trae) {
                hooks::init::run_trae_mode(global, ctx)?
            } else if agent == Some(AgentTarget::Omp) {
                hooks::init::run_omp_mode_with_patch_mode(global, patch_mode, ctx)?
            } else if agent == Some(AgentTarget::Kilocode) {
                if global {
                    anyhow::bail!("Kilo Code is project-scoped. Use: rtk init --agent kilocode");
                }
                hooks::init::run_kilocode_mode(ctx)?;
            } else if agent == Some(AgentTarget::Antigravity) {
                hooks::init::run_antigravity_mode(global, ctx)?;
            } else if agent == Some(AgentTarget::Kimi) {
                if global {
                    anyhow::bail!("Kimi AI is project-scoped. Use: rtk init --agent kimi");
                }
                hooks::init::run_kimi_mode(ctx)?;
            } else if agent == Some(AgentTarget::Hermes) {
                hooks::init::run_hermes_mode(ctx)?;
            } else if agent == Some(AgentTarget::Droid) {
                hooks::init::run_droid_mode(global, ctx)?;
            } else if agent == Some(AgentTarget::Vibe) {
                hooks::init::run_vibe_mode(global, hook_only, patch_mode, ctx)?;
            } else {
                let install_opencode = opencode;
                let install_cursor = agent == Some(AgentTarget::Cursor);
                let install_windsurf = agent == Some(AgentTarget::Windsurf);
                let install_cline = agent == Some(AgentTarget::Cline);
                // Sibling agents (Cursor, Windsurf, Cline) fall through to this
                // shared init path; only the default (no --agent) or an explicit
                // Claude target should install Claude Code files. Without this
                // guard, `rtk init -g --agent cursor` writes into ~/.claude (#2097).
                let install_claude = !install_cursor && !install_windsurf && !install_cline;

                hooks::init::run(
"#,
            new: r"            if show {
                hooks::init::show_config()?;
            } else if uninstall {
                return hooks::init::uninstall_with_patch_mode(
                    global, false, false, false, false, false, patch_mode, ctx,
                );
            } else {
                hooks::init::run(
",
            done: Some(
                r"                return hooks::init::uninstall_with_patch_mode(
",
            ),
        },
        Op::Sub {
            path: "src/lib.rs",
            old: r"                hooks::init::run(
                    global,
                    install_claude,
                    install_opencode,
                    install_cursor,
                    install_windsurf,
                    install_cline,
                    claude_md,
                    hook_only,
                    codex,
                    patch_mode,
                    ctx,
                )?;",
            new: "                hooks::init::run(global, claude_md, hook_only, patch_mode, ctx)?;",
            done: None,
        },
        Op::Drop {
            path: "src/lib.rs",
            sig: "    fn test_try_parse_init_agent_hermes() {",
            gone: None,
        },
        Op::Drop {
            path: "src/lib.rs",
            sig: "    fn test_try_parse_init_agent_trae_and_uninstall() {",
            gone: None,
        },
        Op::Drop {
            path: "src/lib.rs",
            sig: "    fn test_try_parse_init_agent_hermes_uninstall() {",
            gone: None,
        },
        Op::Drop {
            path: "src/lib.rs",
            sig: "    fn test_init_uninstall_dispatch_routes_hermes_to_hermes_cleanup() {",
            gone: None,
        },
        Op::Drop {
            path: "src/lib.rs",
            sig: "    fn test_try_parse_init_agent_omp() {",
            gone: None,
        },
        Op::Drop {
            path: "src/lib.rs",
            sig: "    fn test_try_parse_init_agent_omp_uninstall() {",
            gone: None,
        },
        Op::Drop {
            path: "src/lib.rs",
            sig: "    fn test_init_uninstall_dispatch_routes_omp_to_standard_cleanup() {",
            gone: None,
        },
        Op::Drop {
            path: "src/lib.rs",
            sig: "    fn test_init_agent_pi_parses() {",
            gone: None,
        },
        Op::Drop {
            path: "src/lib.rs",
            sig: "    fn test_init_uninstall_agent_pi_parses() {",
            gone: None,
        },
        // ---- 4b. init/lib/decision tests and imports
        Op::Sub {
            path: "src/hooks/init/mod.rs",
            old: r"use crate::hooks::constants::{
    CLAUDE_DIR, CLAUDE_HOOK_COMMAND, HOOKS_JSON, HOOKS_SUBDIR, PRE_TOOL_USE_KEY, REWRITE_HOOK_FILE,
    SETTINGS_JSON,
};",
            new: r"use crate::hooks::constants::{
    CLAUDE_DIR, CLAUDE_HOOK_COMMAND, HOOKS_SUBDIR, PRE_TOOL_USE_KEY, REWRITE_HOOK_FILE,
    SETTINGS_JSON,
};",
            done: None,
        },
        Op::Sub {
            path: "src/hooks/init/mod.rs",
            old: r"    use super::claude::{remove_hook_from_json, remove_legacy_hook_entries_from_json};
    use super::codex::{codex_hook_already_present, remove_codex_hook_from_json};
    use super::cursor::remove_legacy_cursor_hook_entries_from_json;
    use super::trae::{
        insert_trae_hook_entry, remove_trae_hook_from_json, trae_hook_already_present,
    };
    use super::*;
    use crate::hooks::constants::{
        CLAUDE_PRE_MATCHER, CODEX_HOOK_COMMAND, CURSOR_HOOK_COMMAND, TRAE_HOOK_COMMAND,
        TRAE_RUN_COMMAND_MATCHER,
    };
",
            new: r"    use super::claude::remove_legacy_hook_entries_from_json;
    use super::*;
    use crate::hooks::constants::CLAUDE_PRE_MATCHER;
",
            done: None,
        },
        Op::Cut {
            path: "src/hooks/init/mod.rs",
            old: r#"        let mut cursor = serde_json::json!({"hooks": {"preToolUse": [
            {"command": legacy},
            {"type": "prompt", "command": legacy},
            {"command": CURSOR_HOOK_COMMAND}
        ]}});
        assert!(remove_legacy_cursor_hook_entries_from_json(&mut cursor));
        assert!(!remove_legacy_cursor_hook_entries_from_json(&mut cursor));
        assert_eq!(
            cursor["hooks"]["preToolUse"],
            serde_json::json!([
                {"type": "prompt", "command": legacy},
                {"command": CURSOR_HOOK_COMMAND}
            ])
        );
"#,
            gone: "remove_legacy_cursor_hook_entries_from_json(&mut cursor)",
        },
        Op::Sub {
            path: "src/hooks/init/mod.rs",
            old: r#"            let mut root = serde_json::json!({"hooks": {"PreToolUse": [
                {"hooks": [{"command": CODEX_HOOK_COMMAND}]}
            ]}});
            if let Some(matcher) = matcher {
                root["hooks"]["PreToolUse"][0]["matcher"] = serde_json::json!(matcher);
            }
            assert!(codex_hook_already_present(&root), "{matcher:?}");
            root["hooks"]["PreToolUse"][0]["hooks"][0]["command"] =
                serde_json::json!(CLAUDE_HOOK_COMMAND);
"#,
            new: r#"            let mut root = serde_json::json!({"hooks": {"PreToolUse": [
                {"hooks": [{"command": CLAUDE_HOOK_COMMAND}]}
            ]}});
            if let Some(matcher) = matcher {
                root["hooks"]["PreToolUse"][0]["matcher"] = serde_json::json!(matcher);
            }
"#,
            done: None,
        },
        Op::Cut {
            path: "src/hooks/init/mod.rs",
            old: r#"        for matcher in [
            None,
            Some(""),
            Some("*"),
            Some("Shell"),
            Some("Read|Shell"),
            Some("^Sh"),
        ] {
            let mut root =
                serde_json::json!({"hooks": {"preToolUse": [{"command": CURSOR_HOOK_COMMAND}]}});
            if let Some(matcher) = matcher {
                root["hooks"]["preToolUse"][0]["matcher"] = serde_json::json!(matcher);
            }
            assert!(cursor_hook_already_present(&root), "{matcher:?}");
        }
"#,
            gone: "cursor_hook_already_present(&root)",
        },
        Op::Drop {
            path: "src/hooks/init/mod.rs",
            sig: "    fn test_grouped_registration_preserves_user_hooks_and_checks_matcher() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/decision.rs",
            sig: "    fn agent_path_covers_every_install_target() {",
            gone: None,
        },
        Op::Sub {
            path: "src/lib.rs",
            old: "use clap::{Parser, Subcommand, ValueEnum};",
            new: "use clap::{Parser, Subcommand};",
            done: None,
        },
        Op::Cut {
            path: "src/lib.rs",
            old: r"    use std::cell::Cell;
",
            gone: "use std::cell::Cell;",
        },
        // ---- 5. hook_check: no non-Claude probes
        Op::Sub {
            path: "src/hooks/hook_check.rs",
            old: r"const AGENT_PROBES: &[fn() -> bool] = &[
    super::init::antigravity::is_configured,
    super::init::cursor::is_configured,
    super::init::gemini::is_configured,
    super::init::opencode::is_configured,
    super::init::codex::is_configured,
    super::init::droid::is_configured,
    super::init::trae::is_configured,
    super::init::hermes::is_configured,
    super::init::pi::is_configured,
    super::init::vibe::is_configured,
];",
            new: r"/// anthology rtk-trim: Claude Code is the only agent, so there is no other probe.
const AGENT_PROBES: &[fn() -> bool] = &[];",
            done: None,
        },
        Op::Drop {
            path: "src/hooks/hook_check.rs",
            sig: "    fn test_probes_ignore_foreign_hooks_in_agent_files() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_check.rs",
            sig: "    fn test_check_and_warn_suppressed_when_agent_configured() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_check.rs",
            sig: "    fn test_all_agent_probes_and_arms_in_process() {",
            gone: None,
        },
        // ---- 6. hook subcommands and hook_cmd handlers: Claude only
        Op::Sub {
            path: "src/lib.rs",
            old: r"enum HookCommands {
    /// Process Claude Code PreToolUse hook (reads JSON from stdin)
    Claude,
    /// Process Trae PreToolUse hook (reads JSON from stdin)
    Trae,
    /// Process Codex CLI PreToolUse hook (reads JSON from stdin)
    Codex,
    /// Process Cursor Agent hook (reads JSON from stdin)
    Cursor,
    /// Process Gemini CLI BeforeTool hook (reads JSON from stdin)
    Gemini,
    /// Process Copilot preToolUse hook (VS Code + Copilot CLI, reads JSON from stdin)
    Copilot,
    /// Process Factory Droid PreToolUse hook (reads JSON from stdin)
    Droid,
    /// Process Google Antigravity PreToolUse hook (reads JSON from stdin)
    Antigravity,
    /// Process Mistral Vibe CLI pre_tool hook (reads JSON from stdin)
    Vibe,
    /// Answer for OpenCode's plugin: the rewrite as JSON, or `{}` when
    /// rewriting would change what OpenCode's own permission rules decide
    Opencode {
        /// Active OpenCode agent, when its rules scope permissions by one
        #[arg(long)]
        agent: Option<String>,
        /// Raw command to judge and rewrite
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Check how a command would be rewritten by the hook engine (dry-run)
",
            new: r"enum HookCommands {
    /// Process Claude Code PreToolUse hook (reads JSON from stdin)
    Claude,
    /// Check how a command would be rewritten by the hook engine (dry-run)
",
            done: None,
        },
        Op::Cut {
            path: "src/lib.rs",
            old: r#"            HookCommands::Trae => {
                hooks::hook_cmd::run_trae()?;
                0
            }
            HookCommands::Codex => {
                hooks::hook_cmd::run_codex()?;
                0
            }
            HookCommands::Cursor => {
                hooks::hook_cmd::run_cursor()?;
                0
            }
            HookCommands::Gemini => {
                hooks::hook_cmd::run_gemini()?;
                0
            }
            HookCommands::Copilot => {
                hooks::hook_cmd::run_copilot()?;
                0
            }
            HookCommands::Droid => {
                hooks::hook_cmd::run_droid()?;
                0
            }
            HookCommands::Antigravity => {
                hooks::hook_cmd::run_antigravity()?;
                0
            }
            HookCommands::Vibe => {
                hooks::hook_cmd::run_vibe()?;
                0
            }
            HookCommands::Opencode { agent, args } => {
                hooks::hook_cmd::run_opencode(&args.join(" "), agent.as_deref())?;
                0
            }
"#,
            gone: "hooks::hook_cmd::run_trae()",
        },
        Op::Sub {
            path: "src/lib.rs",
            old: r"    /// Hook processors for LLM CLI tools (Gemini CLI, Copilot, etc.)
",
            new: r"    /// Hook processor for Claude Code, and `hook check`
",
            done: None,
        },
        Op::Drop {
            path: "src/lib.rs",
            sig: "    fn test_hook_trae_parses() {",
            gone: None,
        },
        Op::Drop {
            path: "src/lib.rs",
            sig: "    fn test_hook_codex_parses() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "enum HookFormat {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "pub fn run_copilot() -> Result<()> {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "fn detect_format(v: &Value) -> HookFormat {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "fn heal_legacy_copilot_configs() -> Vec<std::path::PathBuf> {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "fn legacy_camelcase_entry() -> Value {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "fn heal_legacy_hook_file(path: &std::path::Path) -> bool {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "fn handle_vscode(cmd: &str, input: &Value) -> Result<()> {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "fn vscode_response(cmd: &str, input: &Value) -> Option<Value> {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "fn vscode_response_from_decision(",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "fn handle_copilot_cli(cmd: &str, args: &Value) -> Result<()> {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "fn handle_copilot_ide(cmd: &str) -> Result<()> {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "fn copilot_ide_response_from_decision(decision: HookDecision, cmd: &str) -> Option<Value> {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "fn copilot_cli_response(cmd: &str, args: &Value) -> Option<Value> {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "fn copilot_cli_response_from_decision(",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "pub fn run_gemini() -> Result<()> {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "fn run_gemini_inner(input: &str) -> serde_json::Result<String> {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "fn run_gemini_inner_with_rules(",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "fn run_gemini_inner_impl(",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "pub fn run_antigravity() -> Result<()> {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "fn run_antigravity_inner(input: &str) -> Value {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "pub fn run_vibe() -> Result<()> {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "fn run_vibe_inner(input: &str) -> Option<String> {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "fn vibe_rewrite_json(rewritten: &str) -> String {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "fn gemini_json(decision: &str, rewrite: Option<&str>) -> String {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "struct TraeRewrite {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "fn process_trae_payload(v: &Value) -> Option<TraeRewrite> {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "pub fn run_trae() -> Result<()> {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "fn run_trae_inner(input: &str) -> Option<String> {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "fn is_supported_codex_permission_mode(v: &Value) -> bool {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "fn process_codex_payload(v: &Value) -> PayloadAction {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "fn process_codex_payload_from_decision(",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "pub fn run_codex() -> Result<()> {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "fn run_codex_inner(input: &str) -> Option<String> {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "pub fn run_cursor() -> Result<()> {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "fn cursor_allow(rewritten: &str) -> String {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "fn cursor_ask(rewritten: &str) -> String {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "fn run_cursor_inner(input: &str) -> String {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "fn run_cursor_inner_with_rules(",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "fn process_droid_payload(v: &Value) -> Option<Value> {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "fn droid_execute_command(v: &Value) -> Option<&str> {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "fn droid_response_from_decision(v: &Value, cmd: &str, decision: HookDecision) -> Option<Value> {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "pub fn run_opencode(cmd: &str, agent: Option<&str>) -> Result<()> {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "fn opencode_answer_for(cmd: &str, agent: Option<&str>) -> Value {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "fn opencode_answer(cmd: &str, rules: &[permissions_opencode::Rule]) -> Value {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "pub fn run_droid() -> Result<()> {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "fn droid_payload(input: &str) -> serde_json::Result<Option<Value>> {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "fn run_droid_inner(input: &str) -> Option<String> {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "fn run_droid_inner_with_rules(",
            gone: None,
        },
        // ---- 6b. hook_cmd tests of removed handlers (pass 1)
        Op::Cut {
            path: "src/hooks/hook_cmd.rs",
            old: r"use super::permissions_opencode;
",
            gone: "use super::permissions_opencode;",
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_detect_vscode_bash() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_detect_vscode_run_terminal_command() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_detect_vscode_run_in_terminal() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_detect_copilot_cli_bash() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_detect_copilot_ide_run_in_terminal() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_detect_copilot_cli_powershell() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_detect_non_bash_is_passthrough() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_copilot_bom_prefixed_payload_is_recognized() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_detect_unknown_is_passthrough() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_vscode_rewrite_preserves_tool_input_fields() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_vscode_allow_rewrite_sets_permission_allow() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_vscode_ask_rewrite_omits_permission_decision() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_vscode_deny_returns_none() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_vscode_defer_returns_none() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_copilot_cli_ask_rewrite_omits_permission_decision() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_copilot_cli_allow_rewrite_returns_allow() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_copilot_cli_deny_returns_none() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_copilot_cli_defer_returns_none() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_copilot_ide_rewrite_returns_deny_with_suggestion() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_copilot_ide_allow_rewrite_returns_deny_with_suggestion() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_copilot_ide_permission_deny_is_enforced() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_copilot_ide_defer_is_silent() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_copilot_cli_passthrough_unsupported() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_copilot_cli_passthrough_already_rtk() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_copilot_cli_passthrough_heredoc() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_copilot_cli_preserves_env_prefix() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_copilot_cli_preserves_extra_args_fields() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn end_to_end(cmd: &str) -> Option<Value> {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_trae_rewrite_preserves_tool_input_without_permission_override() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_trae_skips_non_run_command_and_unsafe_commands() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_codex_rewrite_uses_required_allow_shape() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_codex_shared_decision_preserves_host_approval_and_deny() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_codex_skip_reasons_remain_distinct() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_codex_rewrites_all_documented_permission_modes() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_codex_unknown_or_missing_permission_mode_passes_through() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_codex_rewrites_commands_that_still_need_native_approval() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_codex_rewrite_preserves_tool_input_fields() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_codex_ignores_other_events_and_tools() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_codex_passthrough_for_unsupported_or_unattestable_commands() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_codex_rewrites_compound_command_directly() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_codex_malformed_json_and_empty_command_pass_through() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn run_cursor_allowed(input: &str) -> String {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_cursor_default_verdict_rewrites() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_cursor_unallowed_segment_asks() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_cursor_passthrough_empty_json() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_cursor_empty_input_empty_json() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_cursor_heredoc_passthrough() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_cursor_already_rtk_passthrough() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    mod opencode_answer {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn gemini_render(cmd: &str, deny: &[String], ask: &[String], allow: &[String]) -> String {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_gemini_strips_utf8_bom() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_gemini_inner_preserves_serde_diagnostic() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_droid_rewrites_execute_tool() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_droid_strips_utf8_bom() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_droid_unlisted_command_omits_decision() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_droid_denylisted_command_steps_aside() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_droid_blocklisted_command_steps_aside() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_droid_allowlist_never_auto_allows() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_droid_project_scope_deny_steps_aside() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_droid_ignores_non_execute_tool() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_droid_bash_tool_name_accepted_defensively() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_droid_deny_steps_aside() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_droid_allow_decision_emits_no_permission_decision() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_droid_substitution_defers() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_droid_file_redirect_defers() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_droid_empty_command_passthrough() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_droid_no_rewrite_passthrough() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_vibe_rewrites_bash_command() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_vibe_rewrites_namespaced_bash_command() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_vibe_ignores_namespaced_non_bash_tool() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_vibe_ignores_other_namespace_bash_tool() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_vibe_strips_utf8_bom() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_vibe_ignores_non_bash_tool() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_vibe_empty_command_passthrough() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_vibe_malformed_json_returns_none() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_vibe_unknown_binary_passthrough() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_vibe_substitution_defers() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_antigravity_rewrite_git_status() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_antigravity_passthrough_non_rewritable() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_antigravity_non_command_tool_passthrough() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_antigravity_empty_and_corrupt_input_passthrough() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_antigravity_leading_bom_and_whitespace_trimmed() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_antigravity_complex_quoted_command_preserved() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_copilot_cli_shell_wrapper_rewrites_without_auto_allow() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_antigravity_fallback_command_args_keys() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_antigravity_pre_prefixed_command_defers() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_antigravity_shell_redirection_and_subshells_defer() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_antigravity_unknown_binary_passthrough() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_droid_shell_wrapper_rewrites_without_permission_decision() {",
            gone: None,
        },
        // ---- 6c. hook_cmd tests of removed handlers (pass 2)
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_copilot_cli_cve_safe_forms_still_rewrite() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_copilot_cli_cve_newline_bypass_never_auto_allows() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_copilot_cli_cve_background_bypass_never_auto_allows() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_copilot_cli_cve_command_substitution_returns_none() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_copilot_cli_cve_backtick_substitution_returns_none() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_copilot_cli_cve_file_redirect_amp_returns_none() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_copilot_cli_cve_file_redirect_returns_none() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_cursor_rewrite_flat_format() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_cursor_substitution_defers_even_when_allowed() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_cursor_no_hook_specific_output() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_cursor_compound_rewrite_includes_continue() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_cursor_strips_single_utf8_bom() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_cursor_strips_double_utf8_bom() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_gemini_allow_emits_rewrite() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_gemini_default_asks_user() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_gemini_substitution_asks_user_without_rewrite() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_gemini_deny_decision() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_cursor_shell_wrapper_rewrites_without_auto_allow() {",
            gone: None,
        },
        Op::Drop {
            path: "src/hooks/hook_cmd.rs",
            sig: "    fn test_gemini_shell_wrapper_asks_user_with_rewrite() {",
            gone: None,
        },
        // ---- 7. paths: docs, packaging, other hosts, dev tooling, non-Claude tests
        Op::Rm(&[
            "README_es.md",
            "README_fr.md",
            "README_ja.md",
            "README_ko.md",
            "README_pt.md",
            "README_zh.md",
            "Formula",
            "install.sh",
            "INSTALL.md",
            ".release-please-manifest.json",
            "release-please-config.json",
            ".claude",
            ".github",
            ".semgrep.yml",
            "openclaw",
            "scripts",
        ]),
        Op::Rm(&[
            "hooks/antigravity",
            "hooks/cline",
            "hooks/codex",
            "hooks/copilot",
            "hooks/cursor",
            "hooks/hermes",
            "hooks/kilocode",
            "hooks/opencode",
            "hooks/pi",
            "hooks/vibe",
            "hooks/windsurf",
        ]),
        Op::Rm(&[
            "docs/guide/pipelines",
            "docs/guide/proxy",
            "docs/guide/resources/agent-coverage.md",
            "docs/guide/resources/editions.md",
            "docs/guide/getting-started/supported-agents.md",
        ]),
        Op::Rm(&[
            "tests/agent_coverage_doc_test.rs",
            "tests/antigravity_hook_test.rs",
            "tests/copilot_repo_hook_test.rs",
            "tests/copilot_selfheal_test.rs",
            "tests/fork_8_upstream_headers.rs",
            "tests/hook_detection_multi_agent_test.rs",
            "tests/init_agent_cursor_test.rs",
            "tests/init_codex_uninstall_test.rs",
            "tests/init_opencode_test.rs",
            "tests/omp_init_test.rs",
            "tests/trae_hook_test.rs",
        ]),
        // ---- 1b. Cargo.toml: comments, unused getrandom, engine/LICENSE assets
        Op::Sub {
            path: "Cargo.toml",
            old: r#"# Two crates, one binary. `rtk` (Apache-2.0) is the CLI; `engine` (BUSL-1.1)
# is `rtk-engine`, the pure compression core the CLI LINKS -- so the shipped
# `rtk` binary contains BUSL-1.1 code, and engine/LICENSE carries an
# Additional Use Grant covering production use as part of that binary.
#
# `proxy/` is DELIBERATELY NOT a member, but note what that does and does
# not buy: cargo scans every manifest in the repository regardless of
# membership, so a bare `cargo install --git <url>` still aborts with
# "multiple packages with binaries found" as long as proxy/ declares a
# [[bin]]. Non-membership only keeps tokio/axum out of the CLI's dependency
# graph and lockfile. The documented install command therefore names the
# package (`cargo install --git <url> rtk`), and the daemon is built
# explicitly from its own directory:
#   cargo build --release --manifest-path proxy/Cargo.toml
#
# The cost of the split is that `libsqlite3-sys` (which declares
# `links = "sqlite3"`) is pinned by hand in both lockfiles: rusqlite 0.34 here
# and in proxy/Cargo.toml, matching what icm-store links. Do not bump one
# alone. CI lints and tests the daemon in its own job.
"#,
            new: r#"# anthology rtk-trim: the Apache-2.0 edition only. The BUSL-1.1 crates
# (engine/, plugin/, proxy/) and the `engine` feature are removed; the
# `#[cfg(feature = "engine")]` code left in src/ can never be enabled (see
# `unexpected_cfgs` below). Workspace: the CLI and its Apache-2.0 text crate.
"#,
            done: None,
        },
        Op::Sub {
            path: "Cargo.toml",
            old: r"# The full edition. `--no-default-features` builds the Apache-2.0 hook core
# alone: no rtk-engine in the dependency graph, no BUSL code in the binary.
",
            new: r"# anthology rtk-trim: no features; the BUSL `engine` feature is removed.
",
            done: None,
        },
        Op::Cut {
            path: "Cargo.toml",
            old: r#"getrandom = "0.4"
"#,
            gone: "getrandom = ",
        },
        Op::Cut {
            path: "Cargo.toml",
            old: r"# The binary links the BUSL-1.1 rtk-engine; its text ships alongside.
",
            gone: "BUSL-1.1 rtk-engine; its text",
        },
        Op::Cut {
            path: "Cargo.toml",
            old: r#"    ["engine/LICENSE", "usr/share/doc/rtk/LICENSE-ENGINE", "644"],
"#,
            gone: r#"["engine/LICENSE""#,
        },
        Op::Cut {
            path: "Cargo.toml",
            old: r#"    { source = "engine/LICENSE", dest = "/usr/share/doc/rtk/LICENSE-ENGINE", mode = "644", doc = true },
"#,
            gone: r#"source = "engine/LICENSE""#,
        },
        // ---- 8. tests that reference removed files or hosts
        Op::Sub {
            path: "src/core/test_isolation/mod.rs",
            old: "const DIRECT_READS: [(&str, &str); 10] = [",
            new: "const DIRECT_READS: [(&str, &str); 5] = [",
            done: None,
        },
        Op::Cut {
            path: "src/core/test_isolation/mod.rs",
            old: r##"    // The same, for the Windows test that runs itself again with a `.cmd`
    // wrapper's directory in front of its `PATH`.
    ("src/core/task_cmd.rs", r#"var_os("PATH")"#),
    // Tells that re-run apart from the test that spawned it. The parent sets
    // it on the child's command, and the child is the test binary itself, so
    // the process environment is the only place it arrives: `user_env` holds
    // what the calling thread set, and nothing more, in a test build.
    ("src/core/task_cmd.rs", "var_os(WRAPPER_DIR)"),
"##,
            gone: r#"("src/core/task_cmd.rs", "var_os(WRAPPER_DIR)")"#,
        },
        Op::Cut {
            path: "src/core/test_isolation/mod.rs",
            old: r#"    // Credential variables a pipeline names at run time, so no accessor
    // could resolve or redirect them for a child.
    ("src/core/classify.rs", "var(env)"),
    ("src/core/pipeline_cmd.rs", "var(&env)"),
    ("src/core/task_cmd.rs", "var(var)"),
"#,
            gone: r#"("src/core/classify.rs", "var(env)")"#,
        },
        Op::Cut {
            path: "tests/hook_decision_protocol_test.rs",
            old: r#"            (
                ".claude/hooks/rtk-rewrite.sh",
                "/hookSpecificOutput/updatedInput/command",
                "/hookSpecificOutput/permissionDecision",
            ),
            (
                "hooks/cursor/rtk-rewrite.sh",
                "/updated_input/command",
                "/permission",
            ),
"#,
            gone: r#""hooks/cursor/rtk-rewrite.sh""#,
        },
        Op::Rm(&[
            "tests/hook_config_io_test.rs",
            "tests/init_uninstall_exit_code_test.rs",
        ]),
    ]
}
