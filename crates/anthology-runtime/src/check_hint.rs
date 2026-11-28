//! The once-per-repo hint at session start that names the files of a repo which run code when
//! Claude Code or an editor opens it, and points at `/anthology:check-repo`.
//!
//! [`surfaces`] looks at five files at the repo top and nothing else: it never runs anything the
//! repo holds, and reads each file up to [`READ_LIMIT`] bytes through the same non-blocking open
//! the settings reads use (a FIFO or device is skipped, a link to a regular file is read as the
//! file). A file over the limit counts as a surface without being read, so padding a file does
//! not hide it. A file that is not valid JSON where JSON is expected, or is nested past the repo
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
    let servers = match probe(top, ".mcp.json") {
        Probe::Text(t) => compat::loads_bounded(&t).is_some_and(|v| {
            v.as_object()
                .and_then(|o| o.get("mcpServers"))
                .is_some_and(filled)
        }),
        Probe::Oversize => true,
        Probe::Skip => false,
    };
    if servers {
        out.push(MCP);
    }
    // `tasks.json` is JSONC (comments are allowed), so a strict parse would miss real files.
    let task = match probe(top, ".vscode/tasks.json") {
        Probe::Text(t) => t.contains("\"folderOpen\""),
        Probe::Oversize => true,
        Probe::Skip => false,
    };
    if task {
        out.push(TASK);
    }
    // Only its presence counts, so it is not opened.
    if compat::is_file(&top.join(".envrc")) {
        out.push(ENVRC);
    }
    out
}

/// What reading `top/rel` gave.
enum Probe {
    /// A regular file within [`READ_LIMIT`]: its text.
    Text(String),
    /// A regular file over [`READ_LIMIT`]. It is never read or parsed, but a repo must not hide
    /// a surface by padding the file, so it counts as one.
    Oversize,
    /// Absent, not a regular file (a FIFO or device is never opened), or unreadable.
    Skip,
}

fn probe(top: &Path, rel: &str) -> Probe {
    let path = top.join(rel);
    match compat::open_read(&path, Some(READ_LIMIT)) {
        Opened::Bytes(raw) => Probe::Text(compat::decode_text(&raw)),
        Opened::Absent => Probe::Skip,
        Opened::Refused => {
            // The size comes from the metadata of the file the path names, not from reading it.
            match std::fs::metadata(&path) {
                Ok(m) if m.is_file() && m.len() > READ_LIMIT => Probe::Oversize,
                _ => Probe::Skip,
            }
        }
    }
}

/// Whether a Claude Code settings file at `top/rel` sets hooks, env, or a helper command. An
/// oversize file does.
fn settings_runs_code(top: &Path, rel: &str) -> bool {
    let text = match probe(top, rel) {
        Probe::Text(t) => t,
        Probe::Oversize => return true,
        Probe::Skip => return false,
    };
    let Some(value) = compat::loads_bounded(&text) else {
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
