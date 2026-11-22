//! The G6 secret patterns (GC-6): `guard_rules._SECRET_PATTERNS`, in order.
//!
//! Each entry is the Python `re` pattern source, verbatim, and whether it is compiled with
//! `re.IGNORECASE`. Callers compile them through `pycompat::re` (GC-16). Every entry must also
//! be in `anthology_memory::privacy::SECRET_PATTERNS` (a test enforces it, GD1/MP1).
//! `port/difftest/oracle/data.py` compares this list with the Python one.

/// `(pattern, ignorecase)` for each G6 pattern.
pub const GUARD_SECRET_PATTERNS: &[(&str, bool)] = &[
    (r#"(?:AKIA|ASIA)[0-9A-Z]{16}"#, false),
    (r#"\bgh[pousr]_[A-Za-z0-9]+"#, false),
    (r#"github_pat_"#, false),
    (r#"\bsk-[A-Za-z0-9_-]{16,}"#, false),
    (r#"\bxox[baprs]-"#, false),
    (r#"-----BEGIN [A-Z ]*PRIVATE KEY-----"#, false),
    (
        r#"password(?:\s*[:=](?![ \t]*(?:\\?[\"'](?:null|none|true|false|<[^<>\r\n]*>|\$\{[^{}\r\n]*\}|[*.xX]+)\\?[\"']|(?:null|none|true|false|<[^<>\r\n]*>|\$\{[^{}\r\n]*\}|[*.xX]+))[ \t]*(?:[,}\]\r\n]|$))|\\?[\"']\s*[:=]\s*(?!(?:\\?[\"'](?:null|none|true|false|<[^<>\r\n]*>|\$\{[^{}\r\n]*\}|[*.xX]+)\\?[\"']|(?:null|none|true|false|<[^<>\r\n]*>|\$\{[^{}\r\n]*\}|[*.xX]+))[ \t]*(?:[,}\]\r\n]|$))(?:\\?[\"'])?(?![\\\"'{\[\]},\s])\S)"#,
        true,
    ),
    (
        r#"api[_-]?key(?:\s*[:=](?![ \t]*(?:\\?[\"'](?:null|none|true|false|<[^<>\r\n]*>|\$\{[^{}\r\n]*\}|[*.xX]+)\\?[\"']|(?:null|none|true|false|<[^<>\r\n]*>|\$\{[^{}\r\n]*\}|[*.xX]+))[ \t]*(?:[,}\]\r\n]|$))|\\?[\"']\s*[:=]\s*(?!(?:\\?[\"'](?:null|none|true|false|<[^<>\r\n]*>|\$\{[^{}\r\n]*\}|[*.xX]+)\\?[\"']|(?:null|none|true|false|<[^<>\r\n]*>|\$\{[^{}\r\n]*\}|[*.xX]+))[ \t]*(?:[,}\]\r\n]|$))(?:\\?[\"'])?(?![\\\"'{\[\]},\s])\S)"#,
        true,
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    // fancy-regex, so a look-around added on the Python side still compiles here. GD1 switches
    // this to `pycompat::re::compile` once P1 lands (GC-6).
    #[test]
    fn there_are_eight_patterns_and_each_compiles() {
        assert_eq!(GUARD_SECRET_PATTERNS.len(), 8);
        for (pattern, ignorecase) in GUARD_SECRET_PATTERNS {
            fancy_regex::RegexBuilder::new(pattern)
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
        assert_eq!(ignoring.len(), 2);
        assert!(ignoring[0].starts_with("password(?:"), "{}", ignoring[0]);
        assert!(ignoring[1].starts_with("api[_-]?key(?:"), "{}", ignoring[1]);
    }

    /// FW-Q's credential patterns skip placeholder values; the look-aheads need fancy-regex
    /// (GC-16). Fixture values are assembled at run time.
    #[test]
    fn the_credential_assignments_skip_placeholders() {
        let rx = |i: usize| {
            fancy_regex::RegexBuilder::new(GUARD_SECRET_PATTERNS[i].0)
                .case_insensitive(true)
                .build()
                .unwrap()
        };
        let password = rx(6);
        let value = ["s3", "cr3t", "V4lue"].concat();
        assert!(password.is_match(&format!("PASSWORD = {value}")).unwrap());
        assert!(
            password
                .is_match(&format!("\"password\": \"{value}\""))
                .unwrap()
        );
        assert!(!password.is_match("password = null").unwrap());
        assert!(
            !password
                .is_match("\"password\": \"<your-password>\"")
                .unwrap()
        );
        let api_key = rx(7);
        assert!(api_key.is_match(&format!("api_key: {value}")).unwrap());
        assert!(!api_key.is_match("api-key = ${API_KEY}").unwrap());
    }
}
