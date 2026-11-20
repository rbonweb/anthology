//! In-place JSON edits that keep the formatting. Owner: DV2.
//!
//! Also [`py_repr`]: Python's `repr()` of a value `json.loads` returned, which the DV2 tools'
//! messages embed (`{rng!r}`, `{head_version!r}`, `str(value)`).

use anthology_pycompat::json::{self, Int, PY_DEFAULT, Value};

use crate::memory_diff::pystr;

/// The nesting depth past which `repr` raises `RecursionError` here (CPython's limit is about
/// 1,000 frames; the exact depth is D-1).
pub const REPR_MAX_DEPTH: usize = 900;

/// Python's `repr(v)` (`str(v)` too, for everything but a top-level string). `None` where
/// Python would raise `RecursionError`.
pub fn py_repr(v: &Value) -> Option<String> {
    let mut out = String::new();
    repr_into(v, 0, &mut out).then_some(out)
}

fn repr_into(v: &Value, depth: usize, out: &mut String) -> bool {
    if depth > REPR_MAX_DEPTH {
        return false;
    }
    match v {
        Value::Null => out.push_str("None"),
        Value::Bool(true) => out.push_str("True"),
        Value::Bool(false) => out.push_str("False"),
        Value::Int(Int::Small(n)) => out.push_str(&n.to_string()),
        Value::Int(Int::Big(s)) => out.push_str(s),
        Value::Float(f) => out.push_str(&float_repr(*f)),
        Value::Str(s) => out.push_str(&str_repr(s)),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                if !repr_into(item, depth + 1, out) {
                    return false;
                }
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            for (i, (k, item)) in map.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&str_repr(k));
                out.push_str(": ");
                if !repr_into(item, depth + 1, out) {
                    return false;
                }
            }
            out.push('}');
        }
    }
    true
}

/// `repr(float)`: `json.dumps` already prints `float.__repr__` for finite values.
pub fn float_repr(f: f64) -> String {
    if f.is_nan() {
        "nan".into()
    } else if f.is_infinite() {
        if f > 0.0 { "inf".into() } else { "-inf".into() }
    } else {
        json::dumps(&Value::Float(f), &PY_DEFAULT)
    }
}

/// `repr(str)`: single quotes unless the text has `'` and no `"`; `\\`, the quote, `\t`, `\n`,
/// `\r` escaped; other non-printable code points as `\xNN`, `\uNNNN` or `\UNNNNNNNN`.
pub fn str_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if pystr::is_printable(c) => out.push(c),
            c => {
                let n = u32::from(c);
                if n < 0x100 {
                    out.push_str(&format!("\\x{n:02x}"));
                } else if n < 0x10000 {
                    out.push_str(&format!("\\u{n:04x}"));
                } else {
                    out.push_str(&format!("\\U{n:08x}"));
                }
            }
        }
    }
    out.push(quote);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(text: &str) -> String {
        py_repr(&json::loads(text).expect("json")).expect("repr")
    }

    #[test]
    fn reprs_match_python() {
        assert_eq!(r("null"), "None");
        assert_eq!(
            r("[true, false, 1, -2.5, 1e16, 1e-05, 123456789012345678901234567890]"),
            "[True, False, 1, -2.5, 1e+16, 1e-05, 123456789012345678901234567890]"
        );
        assert_eq!(
            r("[NaN, Infinity, -Infinity, -0.0]"),
            "[nan, inf, -inf, -0.0]"
        );
        assert_eq!(
            r(r#"{"a": "it's", "b": "say \"hi\"", "c": "both ' \""}"#),
            r#"{'a': "it's", 'b': 'say "hi"', 'c': 'both \' "'}"#
        );
        assert_eq!(
            r(r#""\u0000\u0085\u00e9\u2028\ue000\\\t""#),
            "'\\x00\\x85\u{e9}\\u2028\\ue000\\\\\\t'"
        );
        assert_eq!(str_repr("\u{e0001}"), r"'\U000e0001'");
    }

    #[test]
    fn deep_nesting_is_a_recursion_error() {
        let deep = format!("{}{}", "[".repeat(2000), "]".repeat(2000));
        assert!(py_repr(&json::loads(&deep).expect("json")).is_none());
    }
}
