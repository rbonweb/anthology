//! `JSON.stringify` form (F2): what node prints for `JSON.stringify(JSON.parse(text))` when
//! `loads(text)` is the value. Used for the replay envelope (RP1, design §13.7).
//!
//! - Objects list array-index keys (`"0"` … `"4294967294"`, canonical) first in ascending order,
//!   then the other keys in insertion order (ECMA-262 `OrdinaryOwnPropertyKeys`).
//! - Numbers are JavaScript `Number`s: an integer beyond 2^53 is rounded to the nearest double,
//!   `-0` prints `0`, NaN and ±Infinity print `null`.
//! - Strings escape `"`, `\`, `\b \f \n \r \t` and other controls as `\u00xx`; everything else is
//!   raw. (A lone surrogate, which JS escapes, cannot occur: `loads` made it U+FFFD, D-6.)

use super::float::js_number;
use super::{Int, Value};

/// `JSON.stringify` of the JavaScript form of `v`. Iterative; any depth.
pub fn js_stringify(v: &Value) -> String {
    let mut out = String::new();
    let mut stack: Vec<JsFrame<'_>> = Vec::new();
    open(&mut out, &mut stack, v);
    while let Some((items, _, first)) = stack.last_mut() {
        match items.next() {
            Some((key, value)) => {
                if *first {
                    *first = false;
                } else {
                    out.push(',');
                }
                if let Some(k) = key {
                    write_js_str(&mut out, k);
                    out.push(':');
                }
                open(&mut out, &mut stack, value);
            }
            None => {
                if let Some((_, close, _)) = stack.pop() {
                    out.push(close);
                }
            }
        }
    }
    out
}

type JsFrame<'a> = (std::vec::IntoIter<(Option<&'a str>, &'a Value)>, char, bool);

fn open<'a>(out: &mut String, stack: &mut Vec<JsFrame<'a>>, v: &'a Value) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Int(i) => out.push_str(&js_int(i)),
        Value::Float(f) if f.is_finite() => out.push_str(&js_number(*f)),
        Value::Float(_) => out.push_str("null"),
        Value::Str(s) => write_js_str(out, s),
        Value::Array(a) => {
            out.push('[');
            let items: Vec<_> = a.iter().map(|v| (None, v)).collect();
            stack.push((items.into_iter(), ']', true));
        }
        Value::Object(m) => {
            out.push('{');
            let mut indexed: Vec<(u32, &str, &Value)> = Vec::new();
            let mut named: Vec<(Option<&str>, &Value)> = Vec::new();
            for (k, v) in m.iter() {
                match array_index(k) {
                    Some(n) => indexed.push((n, k, v)),
                    None => named.push((Some(k), v)),
                }
            }
            indexed.sort_unstable_by_key(|e| e.0);
            let mut items: Vec<_> = indexed.into_iter().map(|(_, k, v)| (Some(k), v)).collect();
            items.extend(named);
            stack.push((items.into_iter(), '}', true));
        }
    }
}

/// The value of an array-index key: the canonical decimal form of 0 ..= 2^32 - 2.
fn array_index(k: &str) -> Option<u32> {
    let b = k.as_bytes();
    if b.is_empty() || b.len() > 10 || !b.iter().all(u8::is_ascii_digit) {
        return None;
    }
    if b.len() > 1 && b[0] == b'0' {
        return None;
    }
    let n: u64 = k.parse().ok()?;
    u32::try_from(n).ok().filter(|&n| n != u32::MAX)
}

fn js_int(i: &Int) -> String {
    const EXACT: u64 = 1 << 53;
    let f = match i {
        Int::Small(n) if n.unsigned_abs() <= EXACT => return n.to_string(),
        // `as` rounds to nearest, ties to even, as JavaScript's number parsing does.
        Int::Small(n) => *n as f64,
        Int::Big(s) => s.parse::<f64>().unwrap_or(f64::NAN),
    };
    if f.is_finite() {
        js_number(f)
    } else {
        "null".into()
    }
}

/// `QuoteJSONString` (ECMA-262 §25.5.2.3).
fn write_js_str(out: &mut String, s: &str) {
    out.push('"');
    let mut run = 0;
    for (i, c) in s.char_indices() {
        let esc = match c {
            '"' => "\\\"",
            '\\' => "\\\\",
            '\u{8}' => "\\b",
            '\u{c}' => "\\f",
            '\n' => "\\n",
            '\r' => "\\r",
            '\t' => "\\t",
            c if c < ' ' => "",
            _ => continue,
        };
        out.push_str(&s[run..i]);
        run = i + 1;
        if esc.is_empty() {
            out.push_str(&format!("\\u{:04x}", u32::from(c)));
        } else {
            out.push_str(esc);
        }
    }
    out.push_str(&s[run..]);
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::super::loads;
    use super::*;

    fn js(text: &str) -> String {
        js_stringify(&loads(text).unwrap())
    }

    #[test]
    fn array_index_keys_come_first_in_numeric_order() {
        assert_eq!(
            js(r#"{"b": 1, "10": 2, "2": 3, "01": 4, "4294967295": 5, "4294967294": 6, "a": 7}"#),
            r#"{"2":3,"10":2,"4294967294":6,"b":1,"01":4,"4294967295":5,"a":7}"#
        );
    }

    #[test]
    fn numbers_and_strings_print_like_javascript() {
        assert_eq!(
            js(r#"[1.0, -0.0, 1e21, 9007199254740993, NaN, -Infinity, 1e400, 1e-7]"#),
            "[1,0,1e+21,9007199254740992,null,null,null,1e-7]"
        );
        assert_eq!(js(&"9".repeat(400)), "null");
        assert_eq!(
            js(r#""\u0001\b\u007f\u2028\u00e9\ud83d\ude00\"\\/""#),
            "\"\\u0001\\b\u{7f}\u{2028}é😀\\\"\\\\/\""
        );
    }
}
