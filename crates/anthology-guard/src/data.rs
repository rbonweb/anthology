//! Guard data compiled into the binary (GC-6, design §6.3).
//!
//! Equal to `plugins/anthology-guardrails/scripts/guard_rules.py`; `port/difftest/oracle/data.py`
//! compares them. Settings (deny rules), doctor and the session-start self-test use these
//! before the guard lane exists (I-2).

use anthology_pycompat::json::{Map, Value};

/// Home-relative Chrome and Chromium profile roots (`guard_rules.PROFILE_ROOTS`).
pub const PROFILE_ROOTS: [&str; 4] = [
    "Library/Application Support/Google",
    "Library/Application Support/Chromium",
    ".config/google-chrome",
    ".config/chromium",
];

/// The channel suffixes tried after a profile-root prefix (`guard_rules._PREFIX_VARIANTS`).
pub const PREFIX_VARIANTS: [&str; 6] = ["", "-beta", "-unstable", "-dev", "-stable", "-canary"];

/// `json.dumps(guard_rules.SELF_TEST_SAMPLE)`: the hook input the session-start self-test sends
/// to the guard (GC-4 `PY_DEFAULT`). It must block.
pub const SELF_TEST_SAMPLE_JSON: &str = r#"{"hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {"command": "npx playwright install chromium"}}"#;

/// `guard_rules.SELF_TEST_SAMPLE`, in its key order.
pub fn self_test_sample() -> Value {
    let mut input = Map::new();
    input.insert(
        "command".into(),
        Value::Str("npx playwright install chromium".into()),
    );
    let mut sample = Map::new();
    sample.insert("hook_event_name".into(), Value::Str("PreToolUse".into()));
    sample.insert("tool_name".into(), Value::Str("Bash".into()));
    sample.insert("tool_input".into(), Value::Object(input));
    Value::Object(sample)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Python's default `json.dumps` for the string-and-object values the sample holds.
    fn dumps(v: &Value) -> String {
        match v {
            Value::Str(s) => {
                assert!(s.chars().all(|c| c.is_ascii_graphic() || c == ' '));
                assert!(!s.contains(['"', '\\']));
                format!("\"{s}\"")
            }
            Value::Object(m) => {
                let items: Vec<String> = m
                    .iter()
                    .map(|(k, v)| format!("{}: {}", dumps(&Value::Str(k.into())), dumps(v)))
                    .collect();
                format!("{{{}}}", items.join(", "))
            }
            other => panic!("not in the sample: {other:?}"),
        }
    }

    #[test]
    fn the_sample_matches_its_json_text() {
        assert_eq!(dumps(&self_test_sample()), SELF_TEST_SAMPLE_JSON);
    }

    #[test]
    fn the_profile_roots_are_relative_and_the_first_variant_is_empty() {
        for root in PROFILE_ROOTS {
            assert!(!root.starts_with('/') && !root.ends_with('/'), "{root}");
        }
        assert_eq!(PREFIX_VARIANTS[0], "");
    }
}
