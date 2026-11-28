//! The once-per-repo hint at session start that names the files of a repo which run code when
//! Claude Code or an editor opens it, and points at `/anthology:check-repo`.
//!
//! [`surfaces`] looks at five files at the repo top and nothing else: it never runs anything the
//! repo holds, and reads each file up to [`READ_LIMIT`] bytes through the same non-blocking open
//! the settings reads use (a FIFO or device is skipped, a link to a regular file is read as the
//! file). A file over the limit, not valid JSON where JSON is expected, or nested past the repo
//! JSON depth, is not a surface. `.envrc` is only looked for, never opened. This is a hint, not
//! a scan: the check itself is `anthology check-repo`.

use std::path::Path;

use anthology_pycompat::json::Value;

use crate::compat::{self, Opened};

/// The hint; `{list}` is the labels [`surfaces`] gives, joined with `; `.
pub const CHECK_REPO_HINT: &str = "anthology: this repo has files that run code when Claude Code or an editor opens it ({list}). If you did not write it, check it with /anthology:check-repo <path> from a session in another folder; its hooks may already have run in this one.";

/// Most bytes read of each file.
pub const READ_LIMIT: u64 = 256 * 1024;

/// The label of `.claude/settings.json`.
const SETTINGS: &str = "hooks, env or helper commands in .claude/settings.json";
/// The label of `.claude/settings.local.json`.
const SETTINGS_LOCAL: &str = "the same in .claude/settings.local.json";
/// The label of `.mcp.json`.
const MCP: &str = ".mcp.json servers";
/// The label of `.vscode/tasks.json`.
const TASK: &str = "a VS Code task that runs on folder open";
/// The label of `.envrc`.
const ENVRC: &str = ".envrc";

/// Settings keys that name a command Claude Code runs on its own, besides `hooks` and `env`.
const HELPER_KEYS: [&str; 5] = [
    "statusLine",
    "apiKeyHelper",
    "awsAuthRefresh",
    "awsCredentialExport",
    "otelHeadersHelper",
];

/// The labels of the files at the repo top `top` that run code on open, in a fixed order; empty
/// for none.
pub fn surfaces(top: &Path) -> Vec<&'static str> {
    let mut out = Vec::new();
    if settings_runs_code(top, ".claude/settings.json") {
        out.push(SETTINGS);
    }
    if settings_runs_code(top, ".claude/settings.local.json") {
        out.push(SETTINGS_LOCAL);
    }
    let servers = json_at(top, ".mcp.json")
        .and_then(|v| v.as_object().and_then(|o| o.get("mcpServers")).map(filled));
    if servers == Some(true) {
        out.push(MCP);
    }
    // `tasks.json` is JSONC (comments are allowed), so a strict parse would miss real files.
    if text_at(top, ".vscode/tasks.json").is_some_and(|t| t.contains("\"folderOpen\"")) {
        out.push(TASK);
    }
    // Only its presence counts, so it is not opened.
    if compat::is_file(&top.join(".envrc")) {
        out.push(ENVRC);
    }
    out
}

/// The text of `top/rel` when it is a regular file within [`READ_LIMIT`].
fn text_at(top: &Path, rel: &str) -> Option<String> {
    match compat::open_read(&top.join(rel), Some(READ_LIMIT)) {
        Opened::Bytes(raw) => Some(compat::decode_text(&raw)),
        Opened::Absent | Opened::Refused => None,
    }
}

/// `top/rel` as strict JSON.
fn json_at(top: &Path, rel: &str) -> Option<Value> {
    compat::loads_bounded(&text_at(top, rel)?)
}

/// Whether a Claude Code settings file at `top/rel` sets hooks, env, or a helper command.
fn settings_runs_code(top: &Path, rel: &str) -> bool {
    let Some(value) = json_at(top, rel) else {
        return false;
    };
    let Some(settings) = value.as_object() else {
        return false;
    };
    ["hooks", "env"]
        .iter()
        .chain(HELPER_KEYS.iter())
        .any(|key| settings.get(key).is_some_and(filled))
}

/// A string, array or object that holds something.
fn filled(v: &Value) -> bool {
    match v {
        Value::Str(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
        _ => false,
    }
}

/// [`CHECK_REPO_HINT`] with the labels filled in.
pub fn hint(labels: &[&str]) -> String {
    CHECK_REPO_HINT.replace("{list}", &labels.join("; "))
}
