//! Python semantics on decoded JSON values, for the duck-typed stored ops (`revert`,
//! `reconcile`) and the `==` / `in` tests of `settings_patch.py`.
//!
//! - [`py_eq`] is Python's `==` between two values `json.loads` returned: `1 == 1.0 == True`,
//!   `0 == False`, ints and floats compared exactly, NaN equal to NaN (see below), dicts compared
//!   without order. It is iterative, so any depth works.
//! - Errors Python raises that are not `ValueError` are returned as `"<PyType>: <message>"`
//!   (ruling R2: the text is informational; that it is an error is what counts).

use anthology_pycompat::json::{Int, Value};

/// `type(v).__name__`.
pub(crate) fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Int(_) => "int",
        Value::Float(_) => "float",
        Value::Str(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// A `TypeError`.
pub(crate) fn type_error(msg: impl std::fmt::Display) -> String {
    format!("TypeError: {msg}")
}

/// `KeyError(key)`: `str()` is `repr(key)`.
pub(crate) fn key_error(key: &str) -> String {
    format!("KeyError: '{key}'")
}

/// `AttributeError: '<type>' object has no attribute '<attr>'`.
pub(crate) fn attr_error(v: &Value, attr: &str) -> String {
    format!(
        "AttributeError: '{}' object has no attribute '{attr}'",
        type_name(v)
    )
}

/// `bool(v)`.
pub(crate) fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Int(Int::Small(i)) => *i != 0,
        Value::Int(Int::Big(_)) => true,
        Value::Float(f) => *f != 0.0,
        Value::Str(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(m) => !m.is_empty(),
    }
}

/// `v.get(key)` (`AttributeError` unless `v` is a dict).
pub(crate) fn get<'a>(v: &'a Value, key: &str) -> Result<Option<&'a Value>, String> {
    match v {
        Value::Object(m) => Ok(m.get(key)),
        other => Err(attr_error(other, "get")),
    }
}

/// `v[key]` for a str key.
pub(crate) fn getitem<'a>(v: &'a Value, key: &str) -> Result<&'a Value, String> {
    match v {
        Value::Object(m) => m.get(key).ok_or_else(|| key_error(key)),
        Value::Str(_) => Err(type_error("string indices must be integers, not 'str'")),
        Value::Array(_) => Err(type_error(
            "list indices must be integers or slices, not str",
        )),
        other => Err(type_error(format!(
            "'{}' object is not subscriptable",
            type_name(other)
        ))),
    }
}

/// `list(v)` / `tuple(v)`: a str's characters, a list's items, a dict's keys.
pub(crate) fn py_list(v: &Value) -> Result<Vec<Value>, String> {
    match v {
        Value::Str(s) => Ok(s.chars().map(|c| Value::Str(c.to_string())).collect()),
        Value::Array(a) => Ok(a.clone()),
        Value::Object(m) => Ok(m.iter().map(|(k, _)| Value::Str(k.to_string())).collect()),
        other => Err(type_error(format!(
            "'{}' object is not iterable",
            type_name(other)
        ))),
    }
}

/// `reversed(v)`, as a list.
pub(crate) fn py_reversed(v: &Value) -> Result<Vec<Value>, String> {
    match v {
        Value::Str(_) | Value::Array(_) | Value::Object(_) => {
            let mut items = py_list(v)?;
            items.reverse();
            Ok(items)
        }
        other => Err(type_error(format!(
            "'{}' object is not reversible",
            type_name(other)
        ))),
    }
}

/// `len(v)` for a sized value (str in code points).
pub(crate) fn py_len(v: &Value) -> Option<usize> {
    match v {
        Value::Str(s) => Some(s.chars().count()),
        Value::Array(a) => Some(a.len()),
        Value::Object(m) => Some(m.len()),
        _ => None,
    }
}

/// `hash(v)` succeeds (lists and dicts are unhashable).
pub(crate) fn check_hashable(v: &Value) -> Result<(), String> {
    match v {
        Value::Array(_) | Value::Object(_) => {
            Err(type_error(format!("unhashable type: '{}'", type_name(v))))
        }
        _ => Ok(()),
    }
}

/// A number's exact value for Python's mixed int/float `==`.
enum Num<'a> {
    Int(&'a Int),
    Float(f64),
}

fn num(v: &Value) -> Option<Num<'_>> {
    match v {
        Value::Bool(false) => Some(Num::Int(&Int::Small(0))),
        Value::Bool(true) => Some(Num::Int(&Int::Small(1))),
        Value::Int(i) => Some(Num::Int(i)),
        Value::Float(f) => Some(Num::Float(*f)),
        _ => None,
    }
}

/// The exact decimal text of an integral, finite `f`.
fn float_int_text(f: f64) -> String {
    if f.abs() < 9_223_372_036_854_775_808_f64 {
        return (f as i64).to_string();
    }
    // f = mantissa * 2^exp with exp > 0 here; build the decimal digits by doubling.
    let bits = f.to_bits();
    let exp = ((bits >> 52) & 0x7ff) as i32 - 1075;
    let mantissa = (bits & ((1u64 << 52) - 1)) | (1u64 << 52);
    let mut digits: Vec<u8> = mantissa
        .to_string()
        .bytes()
        .rev()
        .map(|b| b - b'0')
        .collect();
    for _ in 0..exp {
        let mut carry = 0;
        for d in digits.iter_mut() {
            let x = *d * 2 + carry;
            *d = x % 10;
            carry = x / 10;
        }
        if carry > 0 {
            digits.push(carry);
        }
    }
    let mut s: String = digits.iter().rev().map(|d| char::from(b'0' + d)).collect();
    if f < 0.0 {
        s.insert(0, '-');
    }
    s
}

fn int_text(i: &Int) -> String {
    match i {
        Int::Small(n) => n.to_string(),
        Int::Big(s) => s.clone(),
    }
}

fn num_eq(a: &Num<'_>, b: &Num<'_>) -> bool {
    match (a, b) {
        (Num::Int(x), Num::Int(y)) => match (x, y) {
            (Int::Small(x), Int::Small(y)) => x == y,
            _ => int_text(x) == int_text(y),
        },
        // Every NaN `json.loads` returns is one object (`json.decoder.NaN`), and the
        // comparisons here are identity-first (`PyObject_RichCompareBool`: container items,
        // `in`, set lookups), so NaN meets NaN as equal. The one plain top-level `==`
        // (`json.loads(raw) == value`) never sees NaN on the document side: the scanner refuses
        // the token.
        (Num::Float(x), Num::Float(y)) => x == y || (x.is_nan() && y.is_nan()),
        (Num::Int(i), Num::Float(f)) | (Num::Float(f), Num::Int(i)) => {
            f.is_finite() && f.fract() == 0.0 && float_int_text(*f) == int_text(i)
        }
    }
}

/// Python's `a == b` for decoded JSON values (iterative).
pub(crate) fn py_eq(a: &Value, b: &Value) -> bool {
    let mut stack = vec![(a, b)];
    while let Some((a, b)) = stack.pop() {
        if let (Some(x), Some(y)) = (num(a), num(b)) {
            if !num_eq(&x, &y) {
                return false;
            }
            continue;
        }
        match (a, b) {
            (Value::Null, Value::Null) => {}
            (Value::Str(x), Value::Str(y)) => {
                if x != y {
                    return false;
                }
            }
            (Value::Array(x), Value::Array(y)) => {
                if x.len() != y.len() {
                    return false;
                }
                stack.extend(x.iter().zip(y.iter()));
            }
            (Value::Object(x), Value::Object(y)) => {
                if x.len() != y.len() {
                    return false;
                }
                for (k, xv) in x.iter() {
                    match y.get(k) {
                        Some(yv) => stack.push((xv, yv)),
                        None => return false,
                    }
                }
            }
            _ => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use anthology_pycompat::json::loads;

    fn eq(a: &str, b: &str) -> bool {
        py_eq(&loads(a).unwrap(), &loads(b).unwrap())
    }

    #[test]
    fn numbers_compare_as_python_does() {
        assert!(eq("1", "1.0"));
        assert!(eq("true", "1"));
        assert!(eq("false", "0.0"));
        assert!(eq("-0.0", "0"));
        assert!(eq("NaN", "NaN"));
        assert!(!eq("NaN", "1"));
        assert!(!eq("1", "\"1\""));
        assert!(!eq("null", "false"));
        assert!(eq("9007199254740992", "9007199254740992.0"));
        assert!(!eq("9007199254740993", "9007199254740992.0"));
        assert!(eq("1e20", "100000000000000000000"));
        assert!(!eq("1e20", "100000000000000000001"));
        assert!(eq(
            "-1e300",
            &format!("-{}", float_int_text(1e300).trim_start_matches('-'))
        ));
        assert!(!eq("1.5", "1"));
        assert!(!eq("Infinity", "1"));
    }

    #[test]
    fn containers_compare_deeply_without_key_order() {
        assert!(eq(
            r#"{"a": [1, {"b": true}], "c": null}"#,
            r#"{"c": null, "a": [1.0, {"b": 1}]}"#
        ));
        assert!(!eq(r#"{"a": 1}"#, r#"{"a": 1, "b": 2}"#));
        assert!(!eq("[1, 2]", "[2, 1]"));
        assert!(eq("[NaN]", "[NaN]"));
        let deep = format!("{}1{}", "[".repeat(200_000), "]".repeat(200_000));
        assert!(eq(&deep, &deep));
    }

    #[test]
    fn float_text_is_exact() {
        assert_eq!(float_int_text(1e20), "100000000000000000000");
        assert_eq!(float_int_text(2f64.powi(64)), "18446744073709551616");
        assert_eq!(float_int_text(-2f64.powi(63)), "-9223372036854775808");
    }
}
