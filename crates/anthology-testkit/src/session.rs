//! `SessionCase` (RT3): the layout of `tests/test_session_start.py`'s `SessionCase`.
//!
//! Python ran session start against the repo's own `plugins/anthology` (its `scripts/` beside
//! it). The Rust session start finds the guard through `hooks/guard.sh` (F-6) and checks four
//! `bin/` wrappers (F-9), which only the staging overlay has, and the real plugin folder is
//! large, so each case builds a small plugin instead: `<tmp>/plugins/anthology` holds
//! `.claude-plugin/plugin.json`, `templates/routing-guide.md`, `atl.lock` and `bin/` (the
//! repo's wrappers with `port/staging/plugins/anthology/bin` over them), and its sibling
//! `<tmp>/plugins/anthology-guardrails/hooks/guard.sh` is [`BLOCKS`], standing in for the real
//! guard (which needs the installed binary). The environment is `MemCase`'s plus
//! `CLAUDE_PLUGIN_ROOT` and `CLAUDE_PLUGIN_DATA`.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anthology_core::Env;

use crate::mem::MemCase;
use crate::sandbox::repo_root;

/// A guard that reads the sample and blocks it as the real guard does (Python's `BLOCKS`).
pub const BLOCKS: &str =
    "#!/bin/sh\ncat >/dev/null\nprintf 'Blocked by anthology-guardrails (G1): x\\n' >&2\nexit 2\n";
/// A guard that allows everything (Python's `EXIT_0`).
pub const EXIT_0: &str = "#!/bin/sh\nexit 0\n";

/// The repo's `plugins/anthology`.
pub fn real_plugin() -> PathBuf {
    repo_root().join("plugins").join("anthology")
}

/// The repo's routing guide.
pub fn guide_path() -> PathBuf {
    real_plugin().join("templates").join("routing-guide.md")
}

/// Writes `text` at `path` (parents created) with `mode`.
pub fn write_mode(path: &Path, text: &str, mode: u32) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("write_mode: mkdir");
    }
    fs::write(path, text).expect("write_mode: write");
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("write_mode: chmod");
}

/// The plugin's `bin/` wrappers: the repo's, then the staged ones over them.
pub fn copy_bin(dest: &Path) {
    fs::create_dir_all(dest).expect("copy_bin: mkdir");
    let staged = repo_root().join("port/staging/plugins/anthology/bin");
    for dir in [real_plugin().join("bin"), staged] {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let to = dest.join(e.file_name());
            fs::copy(e.path(), &to).expect("copy_bin: copy");
        }
    }
}

/// A small anthology plugin at `root` (module docs): the manifest, the guide, `atl.lock` and
/// `bin/`.
pub fn small_plugin(root: &Path) -> PathBuf {
    let real = real_plugin();
    fs::create_dir_all(root.join(".claude-plugin")).expect("small_plugin: mkdir");
    fs::copy(
        real.join(".claude-plugin/plugin.json"),
        root.join(".claude-plugin/plugin.json"),
    )
    .expect("small_plugin: plugin.json");
    fs::create_dir_all(root.join("templates")).expect("small_plugin: templates");
    fs::copy(guide_path(), root.join("templates/routing-guide.md")).expect("small_plugin: guide");
    fs::copy(real.join("atl.lock"), root.join("atl.lock")).expect("small_plugin: atl.lock");
    copy_bin(&root.join("bin"));
    root.to_path_buf()
}

/// A guardrails plugin at `root` whose `hooks/guard.sh` is `body`.
pub fn guard_plugin(root: &Path, body: &str) -> PathBuf {
    write_mode(&root.join("hooks").join("guard.sh"), body, 0o755);
    root.to_path_buf()
}

/// `SessionCase` (module docs).
pub struct SessionCase {
    pub mem: MemCase,
    /// `PLUGIN`: `<tmp>/plugins/anthology`.
    pub plugin: PathBuf,
    /// Its sibling guardrails folder.
    pub guard: PathBuf,
    /// The hook's environment.
    pub env: Env,
}

impl Default for SessionCase {
    fn default() -> SessionCase {
        SessionCase::new()
    }
}

impl SessionCase {
    /// `setUp`.
    pub fn new() -> SessionCase {
        let mem = MemCase::new();
        let base = mem.tmp.join("plugins");
        let plugin = small_plugin(&base.join("anthology"));
        let guard = guard_plugin(&base.join("anthology-guardrails"), BLOCKS);
        let env = mem
            .env
            .clone()
            .with_os("CLAUDE_PLUGIN_ROOT", &plugin)
            .with_os("CLAUDE_PLUGIN_DATA", &mem.data)
            .without("CLAUDE_CODE_SUBAGENT_MODEL");
        SessionCase {
            mem,
            plugin,
            guard,
            env,
        }
    }

    /// `fake_plugins(guard_body)`: another plugins folder with a copy of the guide and `bin/`,
    /// and a guardrails sibling whose `hooks/guard.sh` is `guard_body`. Returns its anthology
    /// root.
    pub fn fake_plugins(&self, guard_body: &str) -> PathBuf {
        let base = self.mem.tmp.join("fake-plugins");
        let root = base.join("anthology");
        fs::create_dir_all(root.join("templates")).expect("fake_plugins: mkdir");
        fs::copy(guide_path(), root.join("templates/routing-guide.md"))
            .expect("fake_plugins: guide");
        copy_bin(&root.join("bin"));
        guard_plugin(&base.join("anthology-guardrails"), guard_body);
        root
    }
}
