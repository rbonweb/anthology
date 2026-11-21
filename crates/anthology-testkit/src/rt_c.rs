//! Test helpers of lane RT-C (plan §0.2): `SetupCase` (RT4), the layout of
//! `tests/test_setup.py`'s `SetupCase`.
//!
//! Python ran setup against the repo's own `plugins/anthology`, whose sibling
//! `anthology-guardrails` had `scripts/guard.py`. The Rust setup locates the guardrails by
//! `hooks/guard.sh` (F-6), which the repo's guardrails folder does not have before cutover, so
//! each case builds the small plugin of [`crate::session::small_plugin`] at
//! `<tmp>/plugins/anthology` with a sibling `<tmp>/plugins/anthology-guardrails/hooks/guard.sh`.
//! The environment is `MemCase`'s plus `CLAUDE_PLUGIN_DATA` =
//! `<config>/plugins/data/anthology-anthology`.

use std::path::{Path, PathBuf};

use anthology_core::Env;
use anthology_pycompat::json;

use crate::mem::{MemCase, write};
use crate::session::{BLOCKS, guard_plugin, small_plugin};

/// Python's `SETTINGS`.
pub const SETTINGS: &str = r#"{"model": "opus", "permissions": {"allow": ["Bash(ls)"]}}"#;

/// `json.dumps(json.loads(compact), indent=2) + "\n"`.
pub fn pretty(compact: &str) -> String {
    let v = json::loads(compact).expect("pretty: valid JSON");
    json::dumps(&v, &json::INDENT2) + "\n"
}

/// `SetupCase` (module docs).
pub struct SetupCase {
    pub mem: MemCase,
    /// `PLUGIN`: `<tmp>/plugins/anthology`.
    pub plugin: PathBuf,
    /// `self.data_dir`.
    pub data_dir: PathBuf,
    /// `self.settings`: `<config>/settings.json`.
    pub settings: PathBuf,
    /// The environment of everything setup runs.
    pub env: Env,
}

impl Default for SetupCase {
    fn default() -> SetupCase {
        SetupCase::new()
    }
}

impl SetupCase {
    /// `setUp`.
    pub fn new() -> SetupCase {
        let mem = MemCase::new();
        let base = mem.tmp.join("plugins");
        let plugin = small_plugin(&base.join("anthology"));
        guard_plugin(&base.join("anthology-guardrails"), BLOCKS);
        let data_dir = mem
            .config
            .join("plugins")
            .join("data")
            .join("anthology-anthology");
        let env = mem.env.clone().with_os("CLAUDE_PLUGIN_DATA", &data_dir);
        SetupCase {
            settings: mem.config.join("settings.json"),
            mem,
            plugin,
            data_dir,
            env,
        }
    }

    /// `write_settings(data)`: `json.dumps(data, indent=2) + "\n"` written to settings.json.
    pub fn write_settings(&self, compact: &str) -> String {
        let text = pretty(compact);
        write(&self.settings, &text, None);
        text
    }

    /// An anthology plugin at `root` (its manifest only) with a guardrails sibling that has
    /// `hooks/guard.sh` (Python copied the guardrails' `scripts/`).
    pub fn plugin_with_guard(&self, root: &Path) -> PathBuf {
        let manifest = std::fs::read(self.plugin.join(".claude-plugin/plugin.json"))
            .expect("plugin_with_guard: manifest");
        std::fs::create_dir_all(root.join(".claude-plugin")).expect("plugin_with_guard: mkdir");
        std::fs::write(root.join(".claude-plugin/plugin.json"), manifest)
            .expect("plugin_with_guard: write");
        let parent = root.parent().expect("plugin_with_guard: parent");
        guard_plugin(&parent.join("anthology-guardrails"), BLOCKS);
        root.to_path_buf()
    }
}
