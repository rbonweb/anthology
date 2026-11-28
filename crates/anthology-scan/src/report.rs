//! The report: verdict, exit code, text and JSON (Task 7). The render stubs return empty text.

use std::path::PathBuf;

use crate::Scan;
use crate::tools::ToolRun;

/// Where the scanned tree came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Path,
    Url {
        url: String,
        rev: String,
        dest: PathBuf,
    },
}

/// What the CLI knows about a scan and the report prints next to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Meta {
    pub target: String,
    pub source: Source,
    pub opened: bool,
    pub tools: Vec<ToolRun>,
}

/// Printed first when the scanned folder is already open in Claude Code.
pub const OPENED_WARNING: &str = "This folder is already open in Claude Code. Its hooks, env settings and MCP servers may have run when the session started, so the agent findings below may already have acted. If any agent finding is HIGH or CRITICAL: end this session, do not start Claude Code in this folder again, and rotate the API keys and tokens this machine holds. Install scripts, builds and editor tasks have not run unless you ran them.";

/// Printed in every report.
pub const NO_PROOF: &str = "No findings is not proof of safety.";

/// The licence notice `anthology check-repo --notice` prints.
pub const NOTICE: &str = include_str!("../NOTICE");

/// `Do not open` (any CRITICAL), `Review before opening` (any HIGH) or `No known-bad signals
/// found`.
pub fn verdict(_s: &Scan) -> &'static str {
    ""
}

/// 2 for any CRITICAL; else 3 for an error or a partial scan; else 1 for any HIGH; else 0.
pub fn exit_code(_s: &Scan) -> i32 {
    0
}

pub fn render_text(_s: &Scan, _m: &Meta) -> String {
    String::new()
}

/// The JSON report; `nonce` marks the untrusted excerpts.
pub fn render_json(_s: &Scan, _m: &Meta, _nonce: &str) -> String {
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notice_credits_guarddog_under_apache() {
        assert!(NOTICE.contains("Datadog guarddog"), "{NOTICE}");
        assert!(NOTICE.contains("Apache License, Version 2.0"), "{NOTICE}");
    }
}
