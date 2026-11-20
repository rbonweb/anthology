//! The G6 secret patterns (GC-6): `guard_rules._SECRET_PATTERNS`, in order.
//!
//! Each entry is the Python `re` pattern source, verbatim, and whether it is compiled with
//! `re.IGNORECASE`. Callers compile them through `pycompat::re` (GC-16). Every entry must also
//! be in `anthology_memory::privacy::SECRET_PATTERNS` (a test enforces it, GD1/MP1).
//! `port/difftest/oracle/data.py` compares this list with the Python one.

/// `(pattern, ignorecase)` for each G6 pattern.
pub const GUARD_SECRET_PATTERNS: &[(&str, bool)] = &[
    (r"(?:AKIA|ASIA)[0-9A-Z]{16}", false),
    (r"\bgh[pousr]_[A-Za-z0-9]+", false),
    (r"github_pat_", false),
    (r"\bsk-[A-Za-z0-9_-]{16,}", false),
    (r"\bxox[baprs]-", false),
    (r"-----BEGIN [A-Z ]*PRIVATE KEY-----", false),
    (r"password\s*[:=]", true),
    (r"api[_-]?key\s*[:=]", true),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn there_are_eight_patterns_and_each_compiles() {
        assert_eq!(GUARD_SECRET_PATTERNS.len(), 8);
        for (pattern, ignorecase) in GUARD_SECRET_PATTERNS {
            regex::RegexBuilder::new(pattern)
                .case_insensitive(*ignorecase)
                .build()
                .unwrap_or_else(|e| panic!("{pattern}: {e}"));
        }
    }

    #[test]
    fn only_the_credential_assignments_ignore_case() {
        let ignoring: Vec<&str> = GUARD_SECRET_PATTERNS
            .iter()
            .filter(|(_, ignorecase)| *ignorecase)
            .map(|(pattern, _)| *pattern)
            .collect();
        assert_eq!(ignoring, [r"password\s*[:=]", r"api[_-]?key\s*[:=]"]);
    }
}
