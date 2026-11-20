#!/usr/bin/env python3
"""rtk trim: the Apache-2.0 edition of rtk, Claude Code only, without telemetry.

Usage:
  rtk_trim.py [--root RTK_DIR]               apply the trim to RTK_DIR (default: <repo>/rtk)
  rtk_trim.py --lock-guard BEFORE AFTER      fail unless AFTER's packages are a subset of BEFORE's
  rtk_trim.py --trimmed-conflicts            read `git status --porcelain` on stdin and print the
                                             deleted-by-us (DU) paths the trim removes anyway
  rtk_trim.py --removed-paths                print the paths the trim removes (relative to rtk/)

scripts/rtk-trim.sh is the entry point: it runs this, then prunes Cargo.lock in the pinned
Rust image. The trim is an ordered list of operations (`operations` below) over four
idempotent primitives:

- rm(paths...):  delete a file or directory if present.
- sub(path, old, new, done=None):  replace the single occurrence of `old`. Already applied
  when `old` is absent and `done` (default: `new`) is present; otherwise the anchor is
  missing and the trim stops. `new` must not occur inside `old`, or a changed upstream
  block would look applied: extend the anchor with trailing context instead.
- cut(path, old, gone):  delete the single occurrence of `old`. Already applied when the
  `gone` token is absent; `old` absent but `gone` present stops the trim.
- drop(path, signature):  delete the item whose first line is exactly `signature` (with its
  indentation), the `#[...]`/`///`/`//` lines directly above it, through the first line that
  is `<indent>}` (or `},` `});` `};`). Body-tolerant. Already applied when `<keyword> <name>`
  no longer occurs; the name still present but not as that exact line stops the trim.

Every failure prints `rtk-trim: <file>: <reason>: <anchor line>` and exits 1, so an upstream
refactor of a trimmed region is noticed instead of silently half-applied. A second run
prints `no changes`.

Standard library only; runs on Python 3.9+.
"""
from __future__ import annotations

import argparse
import os
import re
import shutil
import sys
from pathlib import Path

DEFAULT_ROOT = Path(__file__).resolve().parent.parent / "rtk"


class TrimError(Exception):
    """An anchor no longer matches: the trim stops (exit 1)."""


def _first_line(text: str) -> str:
    return text.splitlines()[0] if text.strip() else text


class Trim:
    """The primitives, applied under `root` (the rtk/ directory)."""

    def __init__(self, root) -> None:
        self.root = Path(root)
        self.changed: list = []

    def _fail(self, path: str, reason: str, anchor: str) -> None:
        raise TrimError(f"{path}: {reason}: {_first_line(anchor)!r}")

    def _read(self, path: str, anchor: str) -> str:
        full = self.root / path
        if not full.is_file():
            self._fail(path, "file missing", anchor)
        with open(full, encoding="utf-8", newline="") as f:
            return f.read()

    def _write(self, path: str, text: str) -> None:
        with open(self.root / path, "w", encoding="utf-8", newline="") as f:
            f.write(text)

    def rm(self, *paths: str) -> None:
        for path in paths:
            full = self.root / path
            if full.is_dir() and not full.is_symlink():
                shutil.rmtree(full)
                self.changed.append("rm " + path)
            elif os.path.lexists(full):
                full.unlink()
                self.changed.append("rm " + path)

    def sub(self, path: str, old: str, new: str, done=None) -> None:
        if done is None and new in old:
            self._fail(path, "bad anchor design, `new` occurs inside `old` (add trailing context)", old)
        text = self._read(path, old)
        marker = new if done is None else done
        count = text.count(old)
        applied = old in new and new in text  # `new` extends `old`: present means done
        if count == 1 and not applied:
            self._write(path, text.replace(old, new))
            self.changed.append("edit " + path)
            return
        if count > 1 and not applied:
            self._fail(path, f"anchor occurs {count} times", old)
        if marker and marker in text:
            return
        self._fail(path, "anchor missing", old)

    def cut(self, path: str, old: str, gone: str) -> None:
        text = self._read(path, old)
        count = text.count(old)
        if count == 1:
            self._write(path, text.replace(old, ""))
            self.changed.append("edit " + path)
            return
        if count > 1:
            self._fail(path, f"anchor occurs {count} times", old)
        if gone in text:
            self._fail(path, f"anchor missing but {gone!r} still present", old)

    def drop(self, path: str, sig: str, gone=None) -> None:
        text = self._read(path, sig)
        lines = text.split("\n")
        hits = [i for i, line in enumerate(lines) if line == sig]
        if not hits:
            if gone is None:
                m = re.search(r"\b(fn|struct|enum|mod|const|static|type)\s+(\w+)", sig)
                if not m:
                    self._fail(path, "drop needs gone= for this signature", sig)
                present = re.search(r"\b%s\s+%s\b" % (m.group(1), m.group(2)), text) is not None
                token = m.group(1) + " " + m.group(2)
            else:
                present = gone in text
                token = gone
            if present:
                self._fail(path, f"signature changed? {token!r} present but not as this exact line", sig)
            return
        if len(hits) > 1:
            self._fail(path, f"signature occurs {len(hits)} times", sig)
        i = hits[0]
        indent = sig[: len(sig) - len(sig.lstrip())]
        start = i
        while start > 0:
            prev = lines[start - 1]
            stripped = prev.strip()
            if (prev.startswith(indent) and not prev.startswith(indent + " ")
                    and stripped.startswith(("#[", "///", "//"))):
                start -= 1
            else:
                break
        if sig.rstrip().endswith((";", ",")):
            end = i
        else:
            end = None
            closers = (indent + "}", indent + "},", indent + "});", indent + "};")
            for j in range(i + 1, len(lines)):
                if lines[j] in closers:
                    end = j
                    break
                if lines[j].strip() and not lines[j].startswith(indent):
                    self._fail(path, "item has no closing brace at its indentation", sig)
            if end is None:
                self._fail(path, "no end for item", sig)
        # Drop one adjacent blank line, so no double blank line remains.
        if start > 0 and lines[start - 1].strip() == "" and end + 1 < len(lines) and lines[end + 1].strip() == "":
            end += 1
        del lines[start: end + 1]
        self._write(path, "\n".join(lines))
        self.changed.append("drop " + path + " " + sig.strip())


class _Recorder(Trim):
    """Runs the operations without a tree, recording only what `rm` would remove."""

    def __init__(self) -> None:
        super().__init__(".")
        self.removed: list = []

    def rm(self, *paths: str) -> None:
        self.removed.extend(paths)

    def sub(self, *args, **kwargs) -> None:
        pass

    def cut(self, *args, **kwargs) -> None:
        pass

    def drop(self, *args, **kwargs) -> None:
        pass


def removed_paths() -> list:
    """Every path (relative to rtk/) the trim removes."""
    recorder = _Recorder()
    operations(recorder)
    return recorder.removed


def trimmed_conflicts(status: str, removed=None, prefix: str = "rtk/") -> list:
    """The `DU` (deleted by us, modified by them) paths of a `git status --porcelain` text
    that lie inside a path the trim removes: re-running the trim would delete them again,
    so a sync can resolve them by deletion. Quoted (unusual) paths are left for a human."""
    removed = removed_paths() if removed is None else removed
    out = []
    for line in status.splitlines():
        if not line.startswith("DU "):
            continue
        path = line[3:]
        if path.startswith('"') or not path.startswith(prefix):
            continue
        rel = path[len(prefix):]
        if any(rel == p or rel.startswith(p + "/") for p in removed):
            out.append(path)
    return out


def lock_packages(path) -> set:
    """The (name, version) pairs of the `[[package]]` entries of a Cargo.lock."""
    pairs, name, in_package = set(), None, False
    with open(path, encoding="utf-8") as f:
        for line in f:
            line = line.rstrip("\n")
            if line.startswith("["):
                in_package, name = line == "[[package]]", None
                continue
            if not in_package:
                continue
            m = re.match(r'^(name|version) = "([^"]*)"$', line)
            if m and m.group(1) == "name":
                name = m.group(2)
            elif m and name is not None:
                pairs.add((name, m.group(2)))
                name = None
    return pairs


def lock_guard(before, after) -> list:
    """Raise unless the packages of `after` are a subset of those of `before` (the lockfile
    step may only prune); return the removed packages, sorted."""
    old, new = lock_packages(before), lock_packages(after)
    added = sorted(new - old)
    if added:
        raise TrimError("Cargo.lock: the lockfile step added or changed packages: "
                        + ", ".join(f"{n} {v}" for n, v in added))
    return sorted(old - new)


def operations(t: Trim) -> None:
    """The trim, in order. Each anchor is exact text of the merged tree (fork + upstream)."""

    # ---- 1. BUSL crates and everything only they serve
    t.rm("engine", "plugin", "proxy", "examples")
    t.sub("Cargo.toml", 'members = [".", "text", "engine", "plugin"]', 'members = [".", "text"]')
    t.cut("Cargo.toml", 'default = ["engine"]\nengine = ["dep:rtk-engine"]\nexample-plugins = ["engine", "dep:rtk-plugin"]\n', 'dep:rtk-engine')
    t.cut("Cargo.toml", 'rtk-engine = { path = "engine", optional = true }\nrtk-plugin = { path = "plugin", optional = true }\n', 'rtk-plugin = ')
    t.sub("Cargo.toml", '[lints.rust]\nunsafe_code = "deny"\nwarnings = "deny"\n',
    '[lints.rust]\nunsafe_code = "deny"\nwarnings = "deny"\n'
    "unexpected_cfgs = { level = \"deny\", check-cfg = ['cfg(feature, values(\"engine\", \"example-plugins\"))'] }\n"
    "dead_code = \"allow\"\n")
    t.rm("src/example_plugins.rs",
   "src/core/call_context.rs", "src/core/classify.rs", "src/core/pipeline_cmd.rs",
   "src/core/pipeline_config.rs", "src/core/pipeline_log.rs", "src/core/pipeline_remote.rs",
   "src/core/plugins_cmd.rs", "src/core/task_cmd.rs",
   "src/hooks/posthook.rs", "src/hooks/session_start.rs", "src/hooks/hook_cmd")

    # ---- 2. telemetry
    t.rm("src/core/telemetry.rs", "src/core/telemetry_cmd.rs", "docs/TELEMETRY.md", "docs/guide/resources/telemetry.md")
    t.cut("src/core/mod.rs", "pub mod telemetry;\npub mod telemetry_cmd;\n", "pub mod telemetry")
    t.cut("src/lib.rs", """    /// Manage telemetry consent and data (RGPD/GDPR)
    Telemetry {
        #[command(subcommand)]
        command: core::telemetry_cmd::TelemetrySubcommand,
    },

""", "telemetry_cmd::TelemetrySubcommand")
    t.cut("src/lib.rs", """        Commands::Telemetry { command } => {
            core::telemetry_cmd::run(&command)?;
            0
        }

""", "Commands::Telemetry")
    t.cut("src/lib.rs", """    // Fire-and-forget telemetry ping (1/day, non-blocking)
    if pings_telemetry(&parsed) {
        core::telemetry::maybe_ping();
    }

""", "maybe_ping")
    t.drop("src/lib.rs", "fn pings_telemetry(parsed: &Result<Cli, clap::Error>) -> bool {")
    t.drop("src/lib.rs", "    fn hook_commands_skip_the_telemetry_ping() {")
    t.drop("src/lib.rs", "    fn other_invocations_still_run_the_ping() {")
    t.cut("src/hooks/init/mod.rs", "        prompt_telemetry_consent()?;\n", "prompt_telemetry_consent()?")
    t.drop("src/hooks/init/mod.rs", "pub fn save_telemetry_consent(accepted: bool) -> Result<()> {")
    t.drop("src/hooks/init/mod.rs", "fn prompt_telemetry_consent() -> Result<()> {")
    t.cut("src/core/test_isolation/mod.rs", '        "TELEMETRY_DISABLED_ENV" => crate::core::telemetry_cmd::TELEMETRY_DISABLED_ENV,\n', "telemetry_cmd::")

    # ---- 3. hooks/init: Claude only
    t.rm(*["src/hooks/init/%s.rs" % m for m in ("antigravity", "codex", "copilot", "cursor", "droid", "gemini",
     "hermes", "instructions_agents", "opencode", "pi", "trae", "vibe")])
    t.sub("src/hooks/init/mod.rs", """mod agents_md;
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
""", """mod agents_md;
pub(crate) mod claude;

// anthology rtk-trim: Claude Code is the only init target. `agents_md` holds the
// marker-block helpers claude.rs reaches through `use super::*`.
use agents_md::*;
use claude::{
    hook_already_present, hook_gaps, remove_settings_entries, run_claude_md_mode, run_default_mode,
    run_hook_only_mode,
};

pub use claude::claude_hook_gaps;
""")

    # ---- 3b. init::run -> Claude only
    t.sub('src/hooks/init/mod.rs', '/// Main entry point for `rtk init`\n#[allow(clippy::too_many_arguments)]\npub fn run(\n    global: bool,\n    install_claude: bool,\n    install_opencode: bool,\n    install_cursor: bool,\n    install_windsurf: bool,\n    install_cline: bool,\n    claude_md: bool,\n    hook_only: bool,\n    codex: bool,\n    patch_mode: PatchMode,\n    ctx: InitContext,\n) -> Result<()> {\n    let InitContext { dry_run, .. } = ctx;\n    // Validation: Codex mode conflicts\n    if codex {\n        if install_opencode {\n            anyhow::bail!("--codex cannot be combined with --opencode");\n        }\n        if claude_md {\n            anyhow::bail!("--codex cannot be combined with --claude-md");\n        }\n        if hook_only {\n            anyhow::bail!("--codex cannot be combined with --hook-only");\n        }\n        if matches!(patch_mode, PatchMode::Auto) {\n            anyhow::bail!("--codex cannot be combined with --auto-patch");\n        }\n        if matches!(patch_mode, PatchMode::Skip) {\n            anyhow::bail!("--codex cannot be combined with --no-patch");\n        }\n        run_codex_mode(global, ctx)?;\n    } else {\n        // Validation: Global-only features\n        if install_opencode && !global {\n            anyhow::bail!("OpenCode plugin is global-only. Use: rtk init -g --opencode");\n        }\n\n        if install_cursor && !global {\n            anyhow::bail!("Cursor hooks are global-only. Use: rtk init -g --agent cursor");\n        }\n\n        if install_windsurf && !global {\n            anyhow::bail!("Windsurf support is global-only. Use: rtk init -g --agent windsurf");\n        }\n\n        // The OpenCode plugin is an independent target: install it before any\n        // agent so a Claude/Cursor failure cannot leave it missing, and vice\n        // versa. Its error is reported after the agent\'s.\n        let opencode_result = install_opencode.then(|| run_opencode_only_mode(ctx));\n        let opencode_plugin = opencode_result\n            .as_ref()\n            .and_then(|result| result.as_ref().ok())\n            .and_then(|path| path.as_deref());\n\n        let agent_result = (|| -> Result<()> {\n            if install_windsurf {\n                run_windsurf_mode(ctx)?;\n            } else if install_cline {\n                run_cline_mode(ctx)?;\n            } else {\n                // Mode selection (Claude Code / OpenCode)\n                match (install_claude, install_opencode, claude_md, hook_only) {\n                    // The plugin was installed above, independently.\n                    (false, true, _, _) => {}\n                    (true, _, true, _) => run_claude_md_mode(global, ctx)?,\n                    (true, _, false, true) => {\n                        run_hook_only_mode(global, patch_mode, opencode_plugin, ctx)?\n                    }\n                    (true, _, false, false) => {\n                        run_default_mode(global, patch_mode, opencode_plugin, ctx)?\n                    }\n                    (false, false, _, _) => {\n                        if !install_cursor {\n                            anyhow::bail!(\n                                "at least one of install_claude or install_opencode must be true"\n                            )\n                        }\n                    }\n                }\n\n                // Cursor hooks: on their own, or next to the OpenCode plugin with --opencode\n                if install_cursor {\n                    install_cursor_hooks(ctx)?;\n                }\n            }\n            Ok(())\n        })();\n\n        match (opencode_result, agent_result) {\n            (Some(Err(opencode_error)), Err(agent_error)) => {\n                return Err(anyhow::anyhow!(\n                    "OpenCode install failed: {opencode_error:#}\\nAgent install failed: {agent_error:#}"\n                ));\n            }\n            (Some(Err(error)), Ok(())) => {\n                return Err(error.context("OpenCode install failed"));\n            }\n            (_, Err(error)) => return Err(error),\n            _ => {}\n        }\n    }\n\n    if !dry_run {\n',
    '/// Main entry point for `rtk init` (anthology rtk-trim: Claude Code only)\npub fn run(\n    global: bool,\n    claude_md: bool,\n    hook_only: bool,\n    patch_mode: PatchMode,\n    ctx: InitContext,\n) -> Result<()> {\n    let InitContext { dry_run, .. } = ctx;\n    if claude_md {\n        run_claude_md_mode(global, ctx)?;\n    } else if hook_only {\n        run_hook_only_mode(global, patch_mode, None, ctx)?;\n    } else {\n        run_default_mode(global, patch_mode, None, ctx)?;\n    }\n\n    if !dry_run {\n')

    # ---- 3c. init uninstall/show_config -> Claude only
    t.sub('src/hooks/init/mod.rs', '    if codex {\n        let outcome = uninstall_codex(global, ctx)?;\n        if dry_run {\n            print_dry_run_footer();\n        }\n        let code = match outcome {\n            UninstallCodexOutcome::Clean => 0,\n            UninstallCodexOutcome::HookRefused => 2,\n        };\n        return Ok(code);\n    }\n\n    if cursor {\n        if !global {\n            anyhow::bail!("Cursor uninstall only works with --global flag");\n        }\n        let cursor_removed = remove_cursor_hooks(ctx).context("Failed to remove Cursor hooks")?;\n        if !cursor_removed.is_empty() {\n            let header = if dry_run {\n                "[dry-run] would uninstall RTK (Cursor):"\n            } else {\n                "RTK uninstalled (Cursor):"\n            };\n            println!("{}", header);\n            for item in &cursor_removed {\n                println!("  - {}", item);\n            }\n            if !dry_run {\n                println!("\\nRestart Cursor to apply changes.");\n            }\n        } else {\n            println!("RTK Cursor support was not installed (nothing to remove)");\n        }\n        if dry_run {\n            print_dry_run_footer();\n        }\n        return Ok(0);\n    }\n\n    if pi {\n        uninstall_pi_with_patch_mode(global, patch_mode, ctx)?;\n        return Ok(0);\n    }\n\n    if omp {\n        uninstall_omp_with_patch_mode(global, patch_mode, ctx)?;\n        return Ok(0);\n    }\n\n',
    '    // anthology rtk-trim: Claude Code is the only uninstall target.\n    if gemini || codex || cursor || pi || omp {\n        anyhow::bail!("only Claude Code is supported by this rtk build");\n    }\n    let _ = patch_mode;\n\n')
    t.cut('src/hooks/init/mod.rs', '    // Also uninstall Gemini artifacts if --gemini or always (clean everything)\n    if gemini {\n        let gemini_removed = uninstall_gemini(ctx)?;\n        removed.extend(gemini_removed);\n        if !removed.is_empty() {\n            let header = if dry_run {\n                "[dry-run] would uninstall RTK (Gemini):"\n            } else {\n                "RTK uninstalled (Gemini):"\n            };\n            println!("{}", header);\n            for item in &removed {\n                println!("  - {}", item);\n            }\n            if !dry_run {\n                println!("\\nRestart Gemini CLI to apply changes.");\n            }\n        } else {\n            println!("RTK Gemini support was not installed (nothing to remove)");\n        }\n        if dry_run {\n            print_dry_run_footer();\n        }\n        return Ok(0);\n    }\n\n', 'uninstall_gemini(')
    t.cut('src/hooks/init/mod.rs', '    // 5. Remove OpenCode plugin\n    let opencode_removed = remove_opencode_plugin(ctx)?;\n    for path in opencode_removed {\n        removed.push(format!("OpenCode plugin: {}", path.display()));\n    }\n\n    // 6. Remove Cursor hooks\n    let cursor_removed = remove_cursor_hooks(ctx)?;\n    removed.extend(cursor_removed);\n\n', 'remove_opencode_plugin(')
    t.sub('src/hooks/init/mod.rs', 'println!("\\nRestart Claude Code, OpenCode, and Cursor (if used) to apply changes.");',
    'println!("\\nRestart Claude Code to apply changes.");')
    t.sub('src/hooks/init/mod.rs', '/// Show current rtk configuration\npub fn show_config(codex: bool, omp: bool) -> Result<()> {\n    if omp {\n        return show_omp_config();\n    }\n    if codex {\n        return show_codex_config();\n    }\n\n    show_claude_config()\n}\n',
    '/// Show current rtk configuration\npub fn show_config() -> Result<()> {\n    show_claude_config()\n}\n')

    # ---- 3d. init show_claude_config -> Claude only
    t.cut('src/hooks/init/mod.rs', '    // Check OpenCode plugin\n    match resolve_opencode_dir() {\n        Ok(opencode_dir) => {\n            let plugin = opencode_plugin_path(&opencode_dir);\n            if plugin.exists() {\n                println!("[ok] OpenCode: plugin installed ({})", plugin.display());\n            } else {\n                println!("[--] OpenCode: plugin not found");\n            }\n        }\n        _ => println!("[--] OpenCode: config dir not found"),\n    }\n\n    // Check Cursor hooks\n    match resolve_cursor_dir() {\n        Ok(cursor_dir) => {\n            let cursor_hook = cursor_dir.join(HOOKS_SUBDIR).join(REWRITE_HOOK_FILE);\n            let cursor_hooks_json = cursor_dir.join(HOOKS_JSON);\n\n            // Check for binary command in hooks.json first\n            let cursor_binary_registered = if cursor_hooks_json.exists() {\n                let content = fs::read_to_string(&cursor_hooks_json).unwrap_or_default();\n                if let Ok(root) = from_json_str::<serde_json::Value>(&content) {\n                    cursor_hook_already_present(&root)\n                } else {\n                    false\n                }\n            } else {\n                false\n            };\n\n            if cursor_binary_registered {\n                println!("[ok] Cursor hook: registered in hooks.json");\n            } else if cursor_hook.exists() {\n                #[cfg(unix)]\n                {\n                    use std::os::unix::fs::PermissionsExt;\n                    let meta = fs::metadata(&cursor_hook)?;\n                    let is_executable = meta.permissions().mode() & 0o111 != 0;\n\n                    if !is_executable {\n                        println!(\n                            "[warn] Cursor hook: {} (legacy script, NOT executable)",\n                            cursor_hook.display()\n                        );\n                    } else {\n                        println!(\n                            "[warn] Cursor hook: {} (legacy script — run `rtk init -g --agent cursor` to upgrade)",\n                            cursor_hook.display()\n                        );\n                    }\n                }\n\n                #[cfg(not(unix))]\n                {\n                    println!(\n                        "[warn] Cursor hook: {} (legacy script — run `rtk init -g --agent cursor` to upgrade)",\n                        cursor_hook.display()\n                    );\n                }\n            } else {\n                println!("[--] Cursor hook: not found");\n            }\n        }\n        _ => println!("[--] Cursor: home dir not found"),\n    }\n\n', 'resolve_opencode_dir()')
    t.cut('src/hooks/init/mod.rs', '    println!("  rtk init --codex            # Configure local AGENTS.md + RTK.md + hooks.json");\n    println!("  rtk init -g --codex         # Configure global AGENTS.md + RTK.md + hooks.json");\n    println!("  rtk init -g --opencode      # Claude setup + OpenCode plugin");\n    println!("  rtk init -g --agent cursor  # Install Cursor Agent hooks");\n', 'rtk init -g --agent cursor  #')

    # ---- 4. lib.rs: `rtk init` CLI -> Claude only
    t.drop('src/lib.rs', 'pub enum AgentTarget {')
    t.cut('src/lib.rs', '        /// Install OpenCode plugin (in addition to Claude Code)\n        #[arg(long)]\n        opencode: bool,\n\n        /// Initialize for Gemini CLI instead of Claude Code\n        #[arg(long)]\n        gemini: bool,\n\n        /// Target agent to install hooks for (default: claude)\n        #[arg(long, value_enum)]\n        agent: Option<AgentTarget>,\n\n', 'agent: Option<AgentTarget>')
    t.cut('src/lib.rs', '        /// Target Codex CLI (uses PreToolUse hook + AGENTS.md + RTK.md)\n        #[arg(long)]\n        codex: bool,\n\n        /// Install GitHub Copilot integration (VS Code + CLI)\n        #[arg(long)]\n        copilot: bool,\n', 'copilot: bool,')
    t.drop('src/lib.rs', 'fn uninstall_init_dispatch<UninstallHermes, UninstallStandard>(')
    t.sub('src/lib.rs', '        Commands::Init {\n            global,\n            opencode,\n            gemini,\n            agent,\n            show,\n',
    '        Commands::Init {\n            global,\n            show,\n')
    t.sub('src/lib.rs', '            uninstall,\n            codex,\n            copilot,\n            dry_run,\n        } => {',
    '            uninstall,\n            dry_run,\n        } => {')
    t.sub('src/lib.rs', '            if show {\n                hooks::init::show_config(codex, agent == Some(AgentTarget::Omp))?;\n            } else if uninstall && copilot {\n                if global {\n                    hooks::init::uninstall_copilot_global(ctx)?;\n                } else {\n                    hooks::init::uninstall_copilot(ctx)?;\n                }\n            } else if uninstall {\n                return uninstall_init_dispatch(\n                    agent,\n                    global,\n                    gemini,\n                    codex,\n                    patch_mode,\n                    ctx,\n                    hooks::init::uninstall_hermes,\n                    hooks::init::uninstall_with_patch_mode,\n                );\n            } else if gemini {\n                hooks::init::run_gemini(global, hook_only, patch_mode, ctx)?;\n            } else if copilot {\n                if global {\n                    hooks::init::run_copilot_global(ctx)?;\n                } else {\n                    hooks::init::run_copilot(ctx)?;\n                }\n            } else if agent == Some(AgentTarget::Pi) {\n                hooks::init::run_pi_mode_with_patch_mode(global, patch_mode, ctx)?\n            } else if agent == Some(AgentTarget::Trae) {\n                hooks::init::run_trae_mode(global, ctx)?\n            } else if agent == Some(AgentTarget::Omp) {\n                hooks::init::run_omp_mode_with_patch_mode(global, patch_mode, ctx)?\n            } else if agent == Some(AgentTarget::Kilocode) {\n                if global {\n                    anyhow::bail!("Kilo Code is project-scoped. Use: rtk init --agent kilocode");\n                }\n                hooks::init::run_kilocode_mode(ctx)?;\n            } else if agent == Some(AgentTarget::Antigravity) {\n                hooks::init::run_antigravity_mode(global, ctx)?;\n            } else if agent == Some(AgentTarget::Kimi) {\n                if global {\n                    anyhow::bail!("Kimi AI is project-scoped. Use: rtk init --agent kimi");\n                }\n                hooks::init::run_kimi_mode(ctx)?;\n            } else if agent == Some(AgentTarget::Hermes) {\n                hooks::init::run_hermes_mode(ctx)?;\n            } else if agent == Some(AgentTarget::Droid) {\n                hooks::init::run_droid_mode(global, ctx)?;\n            } else if agent == Some(AgentTarget::Vibe) {\n                hooks::init::run_vibe_mode(global, hook_only, patch_mode, ctx)?;\n            } else {\n                let install_opencode = opencode;\n                let install_cursor = agent == Some(AgentTarget::Cursor);\n                let install_windsurf = agent == Some(AgentTarget::Windsurf);\n                let install_cline = agent == Some(AgentTarget::Cline);\n                // Sibling agents (Cursor, Windsurf, Cline) fall through to this\n                // shared init path; only the default (no --agent) or an explicit\n                // Claude target should install Claude Code files. Without this\n                // guard, `rtk init -g --agent cursor` writes into ~/.claude (#2097).\n                let install_claude = !install_cursor && !install_windsurf && !install_cline;\n\n                hooks::init::run(\n',
    '            if show {\n                hooks::init::show_config()?;\n            } else if uninstall {\n                return hooks::init::uninstall_with_patch_mode(\n                    global, false, false, false, false, false, patch_mode, ctx,\n                );\n            } else {\n                hooks::init::run(\n', done='                return hooks::init::uninstall_with_patch_mode(\n')
    t.sub('src/lib.rs', '                hooks::init::run(\n                    global,\n                    install_claude,\n                    install_opencode,\n                    install_cursor,\n                    install_windsurf,\n                    install_cline,\n                    claude_md,\n                    hook_only,\n                    codex,\n                    patch_mode,\n                    ctx,\n                )?;',
    '                hooks::init::run(global, claude_md, hook_only, patch_mode, ctx)?;')
    t.drop('src/lib.rs', '    fn test_try_parse_init_agent_hermes() {')
    t.drop('src/lib.rs', '    fn test_try_parse_init_agent_trae_and_uninstall() {')
    t.drop('src/lib.rs', '    fn test_try_parse_init_agent_hermes_uninstall() {')
    t.drop('src/lib.rs', '    fn test_init_uninstall_dispatch_routes_hermes_to_hermes_cleanup() {')
    t.drop('src/lib.rs', '    fn test_try_parse_init_agent_omp() {')
    t.drop('src/lib.rs', '    fn test_try_parse_init_agent_omp_uninstall() {')
    t.drop('src/lib.rs', '    fn test_init_uninstall_dispatch_routes_omp_to_standard_cleanup() {')
    t.drop('src/lib.rs', '    fn test_init_agent_pi_parses() {')
    t.drop('src/lib.rs', '    fn test_init_uninstall_agent_pi_parses() {')

    # ---- 4b. init/lib/decision tests and imports
    t.sub('src/hooks/init/mod.rs', 'use crate::hooks::constants::{\n    CLAUDE_DIR, CLAUDE_HOOK_COMMAND, HOOKS_JSON, HOOKS_SUBDIR, PRE_TOOL_USE_KEY, REWRITE_HOOK_FILE,\n    SETTINGS_JSON,\n};',
    'use crate::hooks::constants::{\n    CLAUDE_DIR, CLAUDE_HOOK_COMMAND, HOOKS_SUBDIR, PRE_TOOL_USE_KEY, REWRITE_HOOK_FILE,\n    SETTINGS_JSON,\n};')
    t.sub('src/hooks/init/mod.rs', '    use super::claude::{remove_hook_from_json, remove_legacy_hook_entries_from_json};\n    use super::codex::{codex_hook_already_present, remove_codex_hook_from_json};\n    use super::cursor::remove_legacy_cursor_hook_entries_from_json;\n    use super::trae::{\n        insert_trae_hook_entry, remove_trae_hook_from_json, trae_hook_already_present,\n    };\n    use super::*;\n    use crate::hooks::constants::{\n        CLAUDE_PRE_MATCHER, CODEX_HOOK_COMMAND, CURSOR_HOOK_COMMAND, TRAE_HOOK_COMMAND,\n        TRAE_RUN_COMMAND_MATCHER,\n    };\n',
    '    use super::claude::remove_legacy_hook_entries_from_json;\n    use super::*;\n    use crate::hooks::constants::CLAUDE_PRE_MATCHER;\n')
    t.cut('src/hooks/init/mod.rs', '        let mut cursor = serde_json::json!({"hooks": {"preToolUse": [\n            {"command": legacy},\n            {"type": "prompt", "command": legacy},\n            {"command": CURSOR_HOOK_COMMAND}\n        ]}});\n        assert!(remove_legacy_cursor_hook_entries_from_json(&mut cursor));\n        assert!(!remove_legacy_cursor_hook_entries_from_json(&mut cursor));\n        assert_eq!(\n            cursor["hooks"]["preToolUse"],\n            serde_json::json!([\n                {"type": "prompt", "command": legacy},\n                {"command": CURSOR_HOOK_COMMAND}\n            ])\n        );\n', 'remove_legacy_cursor_hook_entries_from_json(&mut cursor)')
    t.sub('src/hooks/init/mod.rs', '            let mut root = serde_json::json!({"hooks": {"PreToolUse": [\n                {"hooks": [{"command": CODEX_HOOK_COMMAND}]}\n            ]}});\n            if let Some(matcher) = matcher {\n                root["hooks"]["PreToolUse"][0]["matcher"] = serde_json::json!(matcher);\n            }\n            assert!(codex_hook_already_present(&root), "{matcher:?}");\n            root["hooks"]["PreToolUse"][0]["hooks"][0]["command"] =\n                serde_json::json!(CLAUDE_HOOK_COMMAND);\n',
    '            let mut root = serde_json::json!({"hooks": {"PreToolUse": [\n                {"hooks": [{"command": CLAUDE_HOOK_COMMAND}]}\n            ]}});\n            if let Some(matcher) = matcher {\n                root["hooks"]["PreToolUse"][0]["matcher"] = serde_json::json!(matcher);\n            }\n')
    t.cut('src/hooks/init/mod.rs', '        for matcher in [\n            None,\n            Some(""),\n            Some("*"),\n            Some("Shell"),\n            Some("Read|Shell"),\n            Some("^Sh"),\n        ] {\n            let mut root =\n                serde_json::json!({"hooks": {"preToolUse": [{"command": CURSOR_HOOK_COMMAND}]}});\n            if let Some(matcher) = matcher {\n                root["hooks"]["preToolUse"][0]["matcher"] = serde_json::json!(matcher);\n            }\n            assert!(cursor_hook_already_present(&root), "{matcher:?}");\n        }\n', 'cursor_hook_already_present(&root)')
    t.drop('src/hooks/init/mod.rs', '    fn test_grouped_registration_preserves_user_hooks_and_checks_matcher() {')
    t.drop('src/hooks/decision.rs', '    fn agent_path_covers_every_install_target() {')
    t.sub('src/lib.rs', 'use clap::{Parser, Subcommand, ValueEnum};',
    'use clap::{Parser, Subcommand};')
    t.cut('src/lib.rs', '    use std::cell::Cell;\n', 'use std::cell::Cell;')

    # ---- 5. hook_check: no non-Claude probes
    t.sub('src/hooks/hook_check.rs', 'const AGENT_PROBES: &[fn() -> bool] = &[\n    super::init::antigravity::is_configured,\n    super::init::cursor::is_configured,\n    super::init::gemini::is_configured,\n    super::init::opencode::is_configured,\n    super::init::codex::is_configured,\n    super::init::droid::is_configured,\n    super::init::trae::is_configured,\n    super::init::hermes::is_configured,\n    super::init::pi::is_configured,\n    super::init::vibe::is_configured,\n];',
    '/// anthology rtk-trim: Claude Code is the only agent, so there is no other probe.\nconst AGENT_PROBES: &[fn() -> bool] = &[];')
    t.drop('src/hooks/hook_check.rs', '    fn test_probes_ignore_foreign_hooks_in_agent_files() {')
    t.drop('src/hooks/hook_check.rs', '    fn test_check_and_warn_suppressed_when_agent_configured() {')
    t.drop('src/hooks/hook_check.rs', '    fn test_all_agent_probes_and_arms_in_process() {')

    # ---- 6. hook subcommands and hook_cmd handlers: Claude only
    t.sub('src/lib.rs', "enum HookCommands {\n    /// Process Claude Code PreToolUse hook (reads JSON from stdin)\n    Claude,\n    /// Process Trae PreToolUse hook (reads JSON from stdin)\n    Trae,\n    /// Process Codex CLI PreToolUse hook (reads JSON from stdin)\n    Codex,\n    /// Process Cursor Agent hook (reads JSON from stdin)\n    Cursor,\n    /// Process Gemini CLI BeforeTool hook (reads JSON from stdin)\n    Gemini,\n    /// Process Copilot preToolUse hook (VS Code + Copilot CLI, reads JSON from stdin)\n    Copilot,\n    /// Process Factory Droid PreToolUse hook (reads JSON from stdin)\n    Droid,\n    /// Process Google Antigravity PreToolUse hook (reads JSON from stdin)\n    Antigravity,\n    /// Process Mistral Vibe CLI pre_tool hook (reads JSON from stdin)\n    Vibe,\n    /// Answer for OpenCode's plugin: the rewrite as JSON, or `{}` when\n    /// rewriting would change what OpenCode's own permission rules decide\n    Opencode {\n        /// Active OpenCode agent, when its rules scope permissions by one\n        #[arg(long)]\n        agent: Option<String>,\n        /// Raw command to judge and rewrite\n        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]\n        args: Vec<String>,\n    },\n    /// Check how a command would be rewritten by the hook engine (dry-run)\n",
    'enum HookCommands {\n    /// Process Claude Code PreToolUse hook (reads JSON from stdin)\n    Claude,\n    /// Check how a command would be rewritten by the hook engine (dry-run)\n')
    t.cut('src/lib.rs', '            HookCommands::Trae => {\n                hooks::hook_cmd::run_trae()?;\n                0\n            }\n            HookCommands::Codex => {\n                hooks::hook_cmd::run_codex()?;\n                0\n            }\n            HookCommands::Cursor => {\n                hooks::hook_cmd::run_cursor()?;\n                0\n            }\n            HookCommands::Gemini => {\n                hooks::hook_cmd::run_gemini()?;\n                0\n            }\n            HookCommands::Copilot => {\n                hooks::hook_cmd::run_copilot()?;\n                0\n            }\n            HookCommands::Droid => {\n                hooks::hook_cmd::run_droid()?;\n                0\n            }\n            HookCommands::Antigravity => {\n                hooks::hook_cmd::run_antigravity()?;\n                0\n            }\n            HookCommands::Vibe => {\n                hooks::hook_cmd::run_vibe()?;\n                0\n            }\n            HookCommands::Opencode { agent, args } => {\n                hooks::hook_cmd::run_opencode(&args.join(" "), agent.as_deref())?;\n                0\n            }\n', 'hooks::hook_cmd::run_trae()')
    t.sub('src/lib.rs', '    /// Hook processors for LLM CLI tools (Gemini CLI, Copilot, etc.)\n',
    '    /// Hook processor for Claude Code, and `hook check`\n')
    t.drop('src/lib.rs', '    fn test_hook_trae_parses() {')
    t.drop('src/lib.rs', '    fn test_hook_codex_parses() {')
    t.drop('src/hooks/hook_cmd.rs', 'enum HookFormat {')
    t.drop('src/hooks/hook_cmd.rs', 'pub fn run_copilot() -> Result<()> {')
    t.drop('src/hooks/hook_cmd.rs', 'fn detect_format(v: &Value) -> HookFormat {')
    t.drop('src/hooks/hook_cmd.rs', 'fn heal_legacy_copilot_configs() -> Vec<std::path::PathBuf> {')
    t.drop('src/hooks/hook_cmd.rs', 'fn legacy_camelcase_entry() -> Value {')
    t.drop('src/hooks/hook_cmd.rs', 'fn heal_legacy_hook_file(path: &std::path::Path) -> bool {')
    t.drop('src/hooks/hook_cmd.rs', 'fn handle_vscode(cmd: &str, input: &Value) -> Result<()> {')
    t.drop('src/hooks/hook_cmd.rs', 'fn vscode_response(cmd: &str, input: &Value) -> Option<Value> {')
    t.drop('src/hooks/hook_cmd.rs', 'fn vscode_response_from_decision(')
    t.drop('src/hooks/hook_cmd.rs', 'fn handle_copilot_cli(cmd: &str, args: &Value) -> Result<()> {')
    t.drop('src/hooks/hook_cmd.rs', 'fn handle_copilot_ide(cmd: &str) -> Result<()> {')
    t.drop('src/hooks/hook_cmd.rs', 'fn copilot_ide_response_from_decision(decision: HookDecision, cmd: &str) -> Option<Value> {')
    t.drop('src/hooks/hook_cmd.rs', 'fn copilot_cli_response(cmd: &str, args: &Value) -> Option<Value> {')
    t.drop('src/hooks/hook_cmd.rs', 'fn copilot_cli_response_from_decision(')
    t.drop('src/hooks/hook_cmd.rs', 'pub fn run_gemini() -> Result<()> {')
    t.drop('src/hooks/hook_cmd.rs', 'fn run_gemini_inner(input: &str) -> serde_json::Result<String> {')
    t.drop('src/hooks/hook_cmd.rs', 'fn run_gemini_inner_with_rules(')
    t.drop('src/hooks/hook_cmd.rs', 'fn run_gemini_inner_impl(')
    t.drop('src/hooks/hook_cmd.rs', 'pub fn run_antigravity() -> Result<()> {')
    t.drop('src/hooks/hook_cmd.rs', 'fn run_antigravity_inner(input: &str) -> Value {')
    t.drop('src/hooks/hook_cmd.rs', 'pub fn run_vibe() -> Result<()> {')
    t.drop('src/hooks/hook_cmd.rs', 'fn run_vibe_inner(input: &str) -> Option<String> {')
    t.drop('src/hooks/hook_cmd.rs', 'fn vibe_rewrite_json(rewritten: &str) -> String {')
    t.drop('src/hooks/hook_cmd.rs', 'fn gemini_json(decision: &str, rewrite: Option<&str>) -> String {')
    t.drop('src/hooks/hook_cmd.rs', 'struct TraeRewrite {')
    t.drop('src/hooks/hook_cmd.rs', 'fn process_trae_payload(v: &Value) -> Option<TraeRewrite> {')
    t.drop('src/hooks/hook_cmd.rs', 'pub fn run_trae() -> Result<()> {')
    t.drop('src/hooks/hook_cmd.rs', 'fn run_trae_inner(input: &str) -> Option<String> {')
    t.drop('src/hooks/hook_cmd.rs', 'fn is_supported_codex_permission_mode(v: &Value) -> bool {')
    t.drop('src/hooks/hook_cmd.rs', 'fn process_codex_payload(v: &Value) -> PayloadAction {')
    t.drop('src/hooks/hook_cmd.rs', 'fn process_codex_payload_from_decision(')
    t.drop('src/hooks/hook_cmd.rs', 'pub fn run_codex() -> Result<()> {')
    t.drop('src/hooks/hook_cmd.rs', 'fn run_codex_inner(input: &str) -> Option<String> {')
    t.drop('src/hooks/hook_cmd.rs', 'pub fn run_cursor() -> Result<()> {')
    t.drop('src/hooks/hook_cmd.rs', 'fn cursor_allow(rewritten: &str) -> String {')
    t.drop('src/hooks/hook_cmd.rs', 'fn cursor_ask(rewritten: &str) -> String {')
    t.drop('src/hooks/hook_cmd.rs', 'fn run_cursor_inner(input: &str) -> String {')
    t.drop('src/hooks/hook_cmd.rs', 'fn run_cursor_inner_with_rules(')
    t.drop('src/hooks/hook_cmd.rs', 'fn process_droid_payload(v: &Value) -> Option<Value> {')
    t.drop('src/hooks/hook_cmd.rs', 'fn droid_execute_command(v: &Value) -> Option<&str> {')
    t.drop('src/hooks/hook_cmd.rs', 'fn droid_response_from_decision(v: &Value, cmd: &str, decision: HookDecision) -> Option<Value> {')
    t.drop('src/hooks/hook_cmd.rs', 'pub fn run_opencode(cmd: &str, agent: Option<&str>) -> Result<()> {')
    t.drop('src/hooks/hook_cmd.rs', 'fn opencode_answer_for(cmd: &str, agent: Option<&str>) -> Value {')
    t.drop('src/hooks/hook_cmd.rs', 'fn opencode_answer(cmd: &str, rules: &[permissions_opencode::Rule]) -> Value {')
    t.drop('src/hooks/hook_cmd.rs', 'pub fn run_droid() -> Result<()> {')
    t.drop('src/hooks/hook_cmd.rs', 'fn droid_payload(input: &str) -> serde_json::Result<Option<Value>> {')
    t.drop('src/hooks/hook_cmd.rs', 'fn run_droid_inner(input: &str) -> Option<String> {')
    t.drop('src/hooks/hook_cmd.rs', 'fn run_droid_inner_with_rules(')

    # ---- 6b. hook_cmd tests of removed handlers (pass 1)
    t.cut('src/hooks/hook_cmd.rs', 'use super::permissions_opencode;\n', 'use super::permissions_opencode;')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_detect_vscode_bash() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_detect_vscode_run_terminal_command() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_detect_vscode_run_in_terminal() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_detect_copilot_cli_bash() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_detect_copilot_ide_run_in_terminal() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_detect_copilot_cli_powershell() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_detect_non_bash_is_passthrough() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_copilot_bom_prefixed_payload_is_recognized() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_detect_unknown_is_passthrough() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_vscode_rewrite_preserves_tool_input_fields() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_vscode_allow_rewrite_sets_permission_allow() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_vscode_ask_rewrite_omits_permission_decision() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_vscode_deny_returns_none() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_vscode_defer_returns_none() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_copilot_cli_ask_rewrite_omits_permission_decision() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_copilot_cli_allow_rewrite_returns_allow() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_copilot_cli_deny_returns_none() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_copilot_cli_defer_returns_none() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_copilot_ide_rewrite_returns_deny_with_suggestion() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_copilot_ide_allow_rewrite_returns_deny_with_suggestion() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_copilot_ide_permission_deny_is_enforced() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_copilot_ide_defer_is_silent() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_copilot_cli_passthrough_unsupported() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_copilot_cli_passthrough_already_rtk() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_copilot_cli_passthrough_heredoc() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_copilot_cli_preserves_env_prefix() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_copilot_cli_preserves_extra_args_fields() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn end_to_end(cmd: &str) -> Option<Value> {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_trae_rewrite_preserves_tool_input_without_permission_override() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_trae_skips_non_run_command_and_unsafe_commands() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_codex_rewrite_uses_required_allow_shape() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_codex_shared_decision_preserves_host_approval_and_deny() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_codex_skip_reasons_remain_distinct() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_codex_rewrites_all_documented_permission_modes() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_codex_unknown_or_missing_permission_mode_passes_through() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_codex_rewrites_commands_that_still_need_native_approval() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_codex_rewrite_preserves_tool_input_fields() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_codex_ignores_other_events_and_tools() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_codex_passthrough_for_unsupported_or_unattestable_commands() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_codex_rewrites_compound_command_directly() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_codex_malformed_json_and_empty_command_pass_through() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn run_cursor_allowed(input: &str) -> String {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_cursor_default_verdict_rewrites() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_cursor_unallowed_segment_asks() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_cursor_passthrough_empty_json() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_cursor_empty_input_empty_json() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_cursor_heredoc_passthrough() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_cursor_already_rtk_passthrough() {')
    t.drop('src/hooks/hook_cmd.rs', '    mod opencode_answer {')
    t.drop('src/hooks/hook_cmd.rs', '    fn gemini_render(cmd: &str, deny: &[String], ask: &[String], allow: &[String]) -> String {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_gemini_strips_utf8_bom() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_gemini_inner_preserves_serde_diagnostic() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_droid_rewrites_execute_tool() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_droid_strips_utf8_bom() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_droid_unlisted_command_omits_decision() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_droid_denylisted_command_steps_aside() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_droid_blocklisted_command_steps_aside() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_droid_allowlist_never_auto_allows() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_droid_project_scope_deny_steps_aside() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_droid_ignores_non_execute_tool() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_droid_bash_tool_name_accepted_defensively() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_droid_deny_steps_aside() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_droid_allow_decision_emits_no_permission_decision() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_droid_substitution_defers() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_droid_file_redirect_defers() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_droid_empty_command_passthrough() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_droid_no_rewrite_passthrough() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_vibe_rewrites_bash_command() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_vibe_rewrites_namespaced_bash_command() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_vibe_ignores_namespaced_non_bash_tool() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_vibe_ignores_other_namespace_bash_tool() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_vibe_strips_utf8_bom() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_vibe_ignores_non_bash_tool() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_vibe_empty_command_passthrough() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_vibe_malformed_json_returns_none() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_vibe_unknown_binary_passthrough() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_vibe_substitution_defers() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_antigravity_rewrite_git_status() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_antigravity_passthrough_non_rewritable() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_antigravity_non_command_tool_passthrough() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_antigravity_empty_and_corrupt_input_passthrough() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_antigravity_leading_bom_and_whitespace_trimmed() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_antigravity_complex_quoted_command_preserved() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_copilot_cli_shell_wrapper_rewrites_without_auto_allow() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_antigravity_fallback_command_args_keys() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_antigravity_pre_prefixed_command_defers() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_antigravity_shell_redirection_and_subshells_defer() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_antigravity_unknown_binary_passthrough() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_droid_shell_wrapper_rewrites_without_permission_decision() {')

    # ---- 6c. hook_cmd tests of removed handlers (pass 2)
    t.drop('src/hooks/hook_cmd.rs', '    fn test_copilot_cli_cve_safe_forms_still_rewrite() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_copilot_cli_cve_newline_bypass_never_auto_allows() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_copilot_cli_cve_background_bypass_never_auto_allows() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_copilot_cli_cve_command_substitution_returns_none() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_copilot_cli_cve_backtick_substitution_returns_none() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_copilot_cli_cve_file_redirect_amp_returns_none() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_copilot_cli_cve_file_redirect_returns_none() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_cursor_rewrite_flat_format() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_cursor_substitution_defers_even_when_allowed() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_cursor_no_hook_specific_output() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_cursor_compound_rewrite_includes_continue() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_cursor_strips_single_utf8_bom() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_cursor_strips_double_utf8_bom() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_gemini_allow_emits_rewrite() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_gemini_default_asks_user() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_gemini_substitution_asks_user_without_rewrite() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_gemini_deny_decision() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_cursor_shell_wrapper_rewrites_without_auto_allow() {')
    t.drop('src/hooks/hook_cmd.rs', '    fn test_gemini_shell_wrapper_asks_user_with_rewrite() {')

    # ---- 7. paths: docs, packaging, other hosts, dev tooling, non-Claude tests
    t.rm("README_es.md", "README_fr.md", "README_ja.md", "README_ko.md", "README_pt.md", "README_zh.md",
   "Formula", "install.sh", "INSTALL.md", ".release-please-manifest.json", "release-please-config.json",
   ".claude", ".github", ".semgrep.yml", "openclaw", "scripts")
    t.rm(*["hooks/" + h for h in ("antigravity", "cline", "codex", "copilot", "cursor", "hermes", "kilocode",
     "opencode", "pi", "vibe", "windsurf")])
    t.rm("docs/guide/pipelines", "docs/guide/proxy", "docs/guide/resources/agent-coverage.md",
   "docs/guide/resources/editions.md", "docs/guide/getting-started/supported-agents.md")
    t.rm(*["tests/%s.rs" % t for t in ("agent_coverage_doc_test", "antigravity_hook_test", "copilot_repo_hook_test",
     "copilot_selfheal_test", "fork_8_upstream_headers", "hook_detection_multi_agent_test",
     "init_agent_cursor_test", "init_codex_uninstall_test", "init_opencode_test", "omp_init_test",
     "trae_hook_test")])

    # ---- 1b. Cargo.toml: comments, unused getrandom, engine/LICENSE assets
    t.sub('Cargo.toml', '# Two crates, one binary. `rtk` (Apache-2.0) is the CLI; `engine` (BUSL-1.1)\n# is `rtk-engine`, the pure compression core the CLI LINKS -- so the shipped\n# `rtk` binary contains BUSL-1.1 code, and engine/LICENSE carries an\n# Additional Use Grant covering production use as part of that binary.\n#\n# `proxy/` is DELIBERATELY NOT a member, but note what that does and does\n# not buy: cargo scans every manifest in the repository regardless of\n# membership, so a bare `cargo install --git <url>` still aborts with\n# "multiple packages with binaries found" as long as proxy/ declares a\n# [[bin]]. Non-membership only keeps tokio/axum out of the CLI\'s dependency\n# graph and lockfile. The documented install command therefore names the\n# package (`cargo install --git <url> rtk`), and the daemon is built\n# explicitly from its own directory:\n#   cargo build --release --manifest-path proxy/Cargo.toml\n#\n# The cost of the split is that `libsqlite3-sys` (which declares\n# `links = "sqlite3"`) is pinned by hand in both lockfiles: rusqlite 0.34 here\n# and in proxy/Cargo.toml, matching what icm-store links. Do not bump one\n# alone. CI lints and tests the daemon in its own job.\n',
    '# anthology rtk-trim: the Apache-2.0 edition only. The BUSL-1.1 crates\n# (engine/, plugin/, proxy/) and the `engine` feature are removed; the\n# `#[cfg(feature = "engine")]` code left in src/ can never be enabled (see\n# `unexpected_cfgs` below). Workspace: the CLI and its Apache-2.0 text crate.\n')
    t.sub('Cargo.toml', '# The full edition. `--no-default-features` builds the Apache-2.0 hook core\n# alone: no rtk-engine in the dependency graph, no BUSL code in the binary.\n',
    '# anthology rtk-trim: no features; the BUSL `engine` feature is removed.\n')
    t.cut('Cargo.toml', 'getrandom = "0.4"\n', 'getrandom = ')
    t.cut('Cargo.toml', '# The binary links the BUSL-1.1 rtk-engine; its text ships alongside.\n', 'BUSL-1.1 rtk-engine; its text')
    t.cut('Cargo.toml', '    ["engine/LICENSE", "usr/share/doc/rtk/LICENSE-ENGINE", "644"],\n', '["engine/LICENSE"')
    t.cut('Cargo.toml', '    { source = "engine/LICENSE", dest = "/usr/share/doc/rtk/LICENSE-ENGINE", mode = "644", doc = true },\n', 'source = "engine/LICENSE"')

    # ---- 8. tests that reference removed files or hosts
    t.sub('src/core/test_isolation/mod.rs', 'const DIRECT_READS: [(&str, &str); 10] = [',
    'const DIRECT_READS: [(&str, &str); 5] = [')
    t.cut('src/core/test_isolation/mod.rs', '    // The same, for the Windows test that runs itself again with a `.cmd`\n    // wrapper\'s directory in front of its `PATH`.\n    ("src/core/task_cmd.rs", r#"var_os("PATH")"#),\n    // Tells that re-run apart from the test that spawned it. The parent sets\n    // it on the child\'s command, and the child is the test binary itself, so\n    // the process environment is the only place it arrives: `user_env` holds\n    // what the calling thread set, and nothing more, in a test build.\n    ("src/core/task_cmd.rs", "var_os(WRAPPER_DIR)"),\n', '("src/core/task_cmd.rs", "var_os(WRAPPER_DIR)")')
    t.cut('src/core/test_isolation/mod.rs', '    // Credential variables a pipeline names at run time, so no accessor\n    // could resolve or redirect them for a child.\n    ("src/core/classify.rs", "var(env)"),\n    ("src/core/pipeline_cmd.rs", "var(&env)"),\n    ("src/core/task_cmd.rs", "var(var)"),\n', '("src/core/classify.rs", "var(env)")')
    t.cut('tests/hook_decision_protocol_test.rs', '            (\n                ".claude/hooks/rtk-rewrite.sh",\n                "/hookSpecificOutput/updatedInput/command",\n                "/hookSpecificOutput/permissionDecision",\n            ),\n            (\n                "hooks/cursor/rtk-rewrite.sh",\n                "/updated_input/command",\n                "/permission",\n            ),\n', '"hooks/cursor/rtk-rewrite.sh"')
    t.rm("tests/hook_config_io_test.rs", "tests/init_uninstall_exit_code_test.rs")


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description="Trim rtk/ to the Apache-2.0, Claude-only edition.")
    parser.add_argument("--root", default=str(DEFAULT_ROOT), help="the rtk/ directory (default: %(default)s)")
    group = parser.add_mutually_exclusive_group()
    group.add_argument("--lock-guard", nargs=2, metavar=("BEFORE", "AFTER"))
    group.add_argument("--trimmed-conflicts", action="store_true")
    group.add_argument("--removed-paths", action="store_true")
    args = parser.parse_args(argv)
    try:
        if args.removed_paths:
            print("\n".join(removed_paths()))
        elif args.trimmed_conflicts:
            paths = trimmed_conflicts(sys.stdin.read())
            if paths:
                print("\n".join(paths))
        elif args.lock_guard:
            removed = lock_guard(*args.lock_guard)
            if removed:
                print(f"rtk-trim: Cargo.lock: {len(removed)} packages pruned, none added or changed: "
                      + ", ".join(f"{n} {v}" for n, v in removed))
        else:
            trim = Trim(args.root)
            if not trim.root.is_dir():
                raise TrimError(f"{args.root}: not a directory: ''")
            operations(trim)
            print("\n".join(trim.changed) if trim.changed else "no changes")
    except TrimError as error:
        sys.stderr.write(f"rtk-trim: {error}\n")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
