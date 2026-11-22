//! Lane GD's test support for anthology-guard (included with `#[path]`).
#![allow(dead_code, reason = "each test binary uses a different part")]

use std::time::Duration;

use anthology_guard::{TOO_COMPLEX_ID, Verdict, check};
use anthology_pycompat::json::{Map, Value};

/// The Python tests' `HOME`.
pub const HOME: &str = "/home/tester";

/// `bash(command)`: `("Bash", {"command": command})`.
pub fn bash(command: &str) -> (Value, Value) {
    (
        Value::Str("Bash".into()),
        obj(&[("command", Value::Str(command.into()))]),
    )
}

/// `bash(command)` as a row's `case` value: `["Bash", {"command": command}]`.
pub fn bash_case(command: &str) -> Value {
    let (tool, input) = bash(command);
    Value::Array(vec![tool, input])
}

/// A JSON object with these entries, in order.
pub fn obj(entries: &[(&str, Value)]) -> Value {
    let mut map = Map::new();
    for (k, v) in entries {
        map.insert((*k).to_string(), v.clone());
    }
    Value::Object(map)
}

/// A GitHub token shape, assembled so the repository never holds one.
pub fn gh_token() -> String {
    ["gh", "p_", "abc123"].concat()
}

/// The id a verdict shows (`G1`..`G6`, `G0` for too complex), or `None` for an allow;
/// `internal` for the limit block.
pub fn verdict_id(v: Verdict) -> Option<String> {
    match v {
        Verdict::Allow => None,
        Verdict::Block(rule) => Some(rule.id().to_string()),
        Verdict::Incomplete => Some("internal".to_string()),
        Verdict::TooComplex => Some(TOO_COMPLEX_ID.to_string()),
    }
}

/// `rule_of(case)` for a row's `case` (`[tool, input]`), with the Python tests' `HOME`.
pub fn rule_of(case: &Value) -> Option<String> {
    let items = match case {
        Value::Array(items) if items.len() == 2 => items,
        other => panic!("a case is [tool, input], got {other:?}"),
    };
    verdict_id(check(&items[0], &items[1], HOME))
}

/// A Python timing bound, in milliseconds. Unoptimized test builds (`cargo test`'s default)
/// get eight times the bound: the bound is Python's, written for CPython, and the debug build
/// is several times slower than the shipped release build.
pub fn bound(ms: u64) -> Duration {
    let factor = if cfg!(debug_assertions) { 8 } else { 1 };
    Duration::from_millis(ms * factor)
}
