//! `check-repo`: a scan of a repo tree for malicious code before anyone opens it (plan
//! `.superpowers/sdd/2026-11-28-check-repo/plan.md`). The crate reads files as data, never follows a
//! symlink and never runs anything of the repo. The scan itself reads no environment; `fetch` and
//! the optional tools read only `PATH`, `HOME`, `SSH_AUTH_SOCK` and
//! `OSV_SCANNER_LOCAL_DB_CACHE_DIRECTORY`.
//!
//! The interface here is frozen by Task 1 of the plan: every module below holds the signatures the
//! later tasks fill in. A stub returns an empty value until its task lands.
#![forbid(unsafe_code)]

pub mod escape;
pub mod fetch;
pub mod files;
pub mod jsonc;
pub mod report;
pub mod shape;
pub mod tomlish;
pub mod tools;
pub mod tree;
pub mod yara;

// The rule modules are `pub(crate)` stubs until `scan` calls them (Tasks 2, 4, 9, 10).
#[allow(dead_code)]
mod rules;
mod scan;

/// How serious a finding is. The order is the report order, lowest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

impl Severity {
    /// The upper-case name the report and the JSON print: `INFO` to `CRITICAL`.
    pub fn name(self) -> &'static str {
        match self {
            Severity::Info => "INFO",
            Severity::Low => "LOW",
            Severity::Medium => "MEDIUM",
            Severity::High => "HIGH",
            Severity::Critical => "CRITICAL",
        }
    }

    /// One level higher; `Critical` stays `Critical`.
    pub fn up(self) -> Severity {
        match self {
            Severity::Info => Severity::Low,
            Severity::Low => Severity::Medium,
            Severity::Medium => Severity::High,
            Severity::High | Severity::Critical => Severity::Critical,
        }
    }
}

/// The rule group of a finding. The order is the report order inside one severity level.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    Agent,
    Autorun,
    Hidden,
    Payload,
    Exfil,
    Deps,
    Yara,
    Tool,
    Scanner,
}

/// What makes an auto-run surface run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    Open,
    Install,
    Build,
    OptIn,
}

/// One thing the scan found. A rule reports one finding per path, with `count` hits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub severity: Severity,
    pub group: Group,
    pub rule: String,
    /// Root-relative, `/` separated.
    pub path: String,
    /// 1-based; 0 means the file as a whole.
    pub line: usize,
    pub why: String,
    /// The raw text; the report escapes and masks it.
    pub seen: String,
    pub action: String,
    pub count: usize,
}

/// A command that runs by itself: an install script, an editor task, a hook.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Surface {
    pub path: String,
    pub line: usize,
    pub trigger: Trigger,
    pub command: String,
    pub rule: &'static str,
    pub floor: Severity,
    /// The folder of the file, for `npm run` resolution.
    pub dir: String,
}

/// The language of a code file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lang {
    Js,
    Py,
    Sh,
    Ps,
    Bat,
    Rb,
    Php,
    Pl,
    Go,
    Rs,
    Jvm,
    Cs,
    C,
    Lua,
    Swift,
}

/// What a file is, as far as the rules care.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FileKind {
    Code(Lang),
    Agent,
    Doc,
    Config,
    Data,
    Binary,
}

/// A file as the rules see it.
#[derive(Clone, Copy, Debug)]
pub struct FileCtx<'a> {
    pub path: &'a str,
    pub bytes: &'a [u8],
    /// The decoded text, or `None` for a file that is not text.
    pub text: Option<&'a str>,
    pub kind: FileKind,
    /// The file was longer than the text cap, so `bytes` and `text` hold only its start.
    pub truncated: bool,
}

/// The result of a scan.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scan {
    pub findings: Vec<Finding>,
    /// The paths of the agent instruction files, for the lead's triage.
    pub instructions: Vec<String>,
    /// What the repo says it is: the README's first paragraph and the manifest description.
    pub about: String,
    pub files: usize,
    pub skipped: Vec<String>,
    pub partial: bool,
    pub errors: Vec<String>,
    pub scan_ms: u64,
}

/// Scans `tree`: every file through the rules, then each auto-run surface chained to the files it
/// runs (Task 2).
pub fn scan(tree: &tree::DiskTree) -> Scan {
    scan::run(tree)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_orders_and_names() {
        let all = [
            Severity::Info,
            Severity::Low,
            Severity::Medium,
            Severity::High,
            Severity::Critical,
        ];
        for pair in all.windows(2) {
            assert!(pair[0] < pair[1], "{pair:?}");
        }
        let names: Vec<&str> = all.iter().map(|s| s.name()).collect();
        assert_eq!(names, ["INFO", "LOW", "MEDIUM", "HIGH", "CRITICAL"]);
    }

    #[test]
    fn up_caps_at_critical() {
        assert_eq!(Severity::Info.up(), Severity::Low);
        assert_eq!(Severity::Low.up(), Severity::Medium);
        assert_eq!(Severity::Medium.up(), Severity::High);
        assert_eq!(Severity::High.up(), Severity::Critical);
        assert_eq!(Severity::Critical.up(), Severity::Critical);
    }
}
