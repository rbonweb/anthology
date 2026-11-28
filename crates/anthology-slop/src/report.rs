//! The gate's messages (Task 6). Until then every message is empty.

use crate::{Finding, Waiver};

/// The most findings a block message lists.
pub const MAX_SHOWN: usize = 20;

/// The stderr message of a refused edit. `findings` holds only unwaived findings.
pub fn hook_message(_tool: &str, _shown: &str, _findings: &[Finding]) -> String {
    String::new()
}

/// `{shown}:{line}: {rule}: "{excerpt}"`.
pub fn check_line(_shown: &str, _f: &Finding) -> String {
    String::new()
}

/// `{shown}:{line}: slop-ok {rule}: {reason}` or `{shown}:{line}: slop-ignore region`.
pub fn waiver_line(_shown: &str, _w: &Waiver) -> String {
    String::new()
}

/// One `{id}\t{severity}\t{kinds}\t{summary}` line per rule.
pub fn rules_table() -> String {
    String::new()
}
