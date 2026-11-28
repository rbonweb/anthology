//! A YARA subset on `regex` and `aho-corasick`, and GuardDog's threat rules (Task 5). The stubs
//! compile no rule and match nothing.

mod atoms;
mod eval;
mod parse;

use std::sync::OnceLock;

use crate::FileCtx;

/// A set of compiled rules.
#[derive(Debug, Default)]
pub struct RuleSet;

/// One rule that matched a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct YaraHit {
    pub rule: String,
    /// The rule's own `severity` meta: `high`, `medium` or `low`.
    pub severity: &'static str,
    pub line: usize,
    pub seen: String,
}

/// Compiles `(name, source)` pairs. A rule that uses syntax outside the subset is left out and
/// returned as `(rule, reason)`.
pub fn compile(_src: &[(&str, &str)]) -> (RuleSet, Vec<(String, String)>) {
    (RuleSet, Vec::new())
}

impl RuleSet {
    /// The hits of every rule on `f`.
    pub fn eval(&self, _f: &FileCtx) -> Vec<YaraHit> {
        Vec::new()
    }
}

/// The enabled GuardDog threat rules, compiled once.
pub fn guarddog() -> &'static RuleSet {
    static RULES: OnceLock<RuleSet> = OnceLock::new();
    RULES.get_or_init(|| compile(&[]).0)
}

/// GuardDog rules that use syntax outside the subset: `(rule, reason)`.
pub const UNSUPPORTED: &[(&str, &str)] = &[];

/// GuardDog rules left off because of a false alarm in the corpus: `(rule, reason)`.
pub const DISABLED: &[(&str, &str)] = &[];
