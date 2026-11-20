//! `pycompat::json`: Python's `json` module, byte for byte (plan §3.1, GC-4, design §9.5, §12).
//!
//! - [`loads`] is CPython 3.12's `json.loads` (the C scanner): the same accepted texts, values,
//!   `JSONDecodeError` messages and code-point positions. NaN and Infinity are accepted; a
//!   repeated key keeps its first position and takes the last value. Two deliberate differences
//!   (GC-16): a lone surrogate escape decodes to U+FFFD (D-6, F-19), and there is **no depth
//!   limit** where Python raised `RecursionError` (D-1, F-10).
//! - [`dumps`] is `json.dumps` under a [`DumpOpts`] preset (GC-4): separators, `indent`,
//!   `sort_keys` by code point, `ensure_ascii` escapes with surrogate pairs, and float `repr`.
//! - [`js_stringify`] is node's `JSON.stringify(JSON.parse(text))` for the same value (RP1).
//!
//! # Depth
//!
//! Parsing, printing, [`Clone`], [`PartialEq`] and dropping a [`Value`] are all iterative, so any
//! depth works on a small thread stack (the guard parses hostile input, and a stack overflow there
//! would abort the binary). `Debug` stays derived (recursive): do not `{:?}` hostile values.
//!
//! # No by-value destructuring of `Value`
//!
//! `Value` implements [`Drop`] (an iterative drain), so a `match` cannot move a field out of an
//! owned `Value` (`E0509`). Match on a reference (`match &v { Value::Str(s) => … }`), or take
//! ownership with [`Value::into_string`], [`Value::into_array`], [`Value::into_object`] or
//! [`Value::take`] (which leaves `Null` behind).
//!
//! # Errors
//!
//! [`JsonError`] carries Python's message and position: [`JsonError::type_name`] is the Python
//! exception class and [`JsonError::py_str`] is `str(exc)`.

mod dump;
mod float;
mod js;
mod parse;

use std::collections::HashMap;
use std::fmt;

pub use dump::{
    COMPACT_SORTED, DumpOpts, INDENT2, INDENT2_SORTED, PRESETS, PY_DEFAULT, PY_DEFAULT_SORTED,
    PY_DEFAULT_UNICODE, STATE, dumps,
};
pub use js::js_stringify;
pub use parse::{loads, loads_bytes_lossy, loads_utf8_sig};

/// A JSON value, as Python's `json.loads` returns it. See the module docs on destructuring.
#[derive(Debug)]
pub enum Value {
    Null,
    Bool(bool),
    Int(Int),
    Float(f64),
    Str(String),
    Array(Vec<Value>),
    Object(Map),
}

/// A JSON integer: `Small` when it fits an `i64`, else `Big` with its canonical decimal text
/// (an optional `-`, no leading zeros).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Int {
    Small(i64),
    Big(String),
}

/// A JSON object in insertion order. A repeated key keeps its first position and takes the
/// last value, as a Python `dict` does. Equality ignores the order, as `dict ==` does.
#[derive(Clone, Debug, Default)]
pub struct Map(Vec<(String, Value)>);

impl Value {
    /// Takes the value out, leaving `Null` in its place.
    pub fn take(&mut self) -> Value {
        std::mem::replace(self, Value::Null)
    }

    /// The string, if this is a `Str`.
    pub fn into_string(mut self) -> Option<String> {
        match &mut self {
            Value::Str(s) => Some(std::mem::take(s)),
            _ => None,
        }
    }

    /// The items, if this is an `Array`.
    pub fn into_array(mut self) -> Option<Vec<Value>> {
        match &mut self {
            Value::Array(a) => Some(std::mem::take(a)),
            _ => None,
        }
    }

    /// The object, if this is an `Object`.
    pub fn into_object(mut self) -> Option<Map> {
        match &mut self {
            Value::Object(m) => Some(std::mem::take(m)),
            _ => None,
        }
    }

    /// The string, if this is a `Str`.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    /// The flag, if this is a `Bool`.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// The integer, if this is an `Int` that fits an `i64`.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(Int::Small(i)) => Some(*i),
            _ => None,
        }
    }

    /// The items, if this is an `Array`.
    pub fn as_array(&self) -> Option<&Vec<Value>> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }

    /// The items, mutably, if this is an `Array`.
    pub fn as_array_mut(&mut self) -> Option<&mut Vec<Value>> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }

    /// The object, if this is an `Object`.
    pub fn as_object(&self) -> Option<&Map> {
        match self {
            Value::Object(m) => Some(m),
            _ => None,
        }
    }

    /// The object, mutably, if this is an `Object`.
    pub fn as_object_mut(&mut self) -> Option<&mut Map> {
        match self {
            Value::Object(m) => Some(m),
            _ => None,
        }
    }

    /// Whether this is `Null`.
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    fn is_container(&self) -> bool {
        matches!(self, Value::Array(_) | Value::Object(_))
    }
}

impl Drop for Value {
    /// Iterative: nested containers are drained onto a heap stack, so depth never reaches the
    /// call stack.
    fn drop(&mut self) {
        let mut stack: Vec<Value> = match self {
            Value::Array(a) if a.iter().any(Value::is_container) => std::mem::take(a),
            Value::Object(m) if m.0.iter().any(|(_, v)| v.is_container()) => {
                m.0.drain(..).map(|(_, v)| v).collect()
            }
            _ => return,
        };
        while let Some(mut v) = stack.pop() {
            match &mut v {
                Value::Array(a) => stack.append(a),
                Value::Object(m) => stack.extend(m.0.drain(..).map(|(_, v)| v)),
                _ => {}
            }
            // `v` is now a scalar or an empty container: its own drop returns at once.
        }
    }
}

impl Clone for Value {
    /// Iterative, post-order: children are cloned onto an output stack, then gathered.
    fn clone(&self) -> Value {
        enum Task<'a> {
            Visit(&'a Value),
            Array(usize),
            Object(&'a Map),
        }
        let mut tasks = vec![Task::Visit(self)];
        let mut out: Vec<Value> = Vec::new();
        while let Some(task) = tasks.pop() {
            match task {
                Task::Visit(v) => match v {
                    Value::Null => out.push(Value::Null),
                    Value::Bool(b) => out.push(Value::Bool(*b)),
                    Value::Int(i) => out.push(Value::Int(i.clone())),
                    Value::Float(f) => out.push(Value::Float(*f)),
                    Value::Str(s) => out.push(Value::Str(s.clone())),
                    Value::Array(a) => {
                        tasks.push(Task::Array(a.len()));
                        tasks.extend(a.iter().rev().map(Task::Visit));
                    }
                    Value::Object(m) => {
                        tasks.push(Task::Object(m));
                        tasks.extend(m.0.iter().rev().map(|(_, v)| Task::Visit(v)));
                    }
                },
                Task::Array(n) => {
                    let items = out.split_off(out.len() - n);
                    out.push(Value::Array(items));
                }
                Task::Object(m) => {
                    let values = out.split_off(out.len() - m.0.len());
                    let entries = m.0.iter().map(|(k, _)| k.clone()).zip(values).collect();
                    out.push(Value::Object(Map(entries)));
                }
            }
        }
        out.pop().unwrap_or(Value::Null)
    }
}

/// Maps with more entries than this are compared through a hash index.
const LINEAR_LOOKUP_MAX: usize = 16;

/// Pushes the value pairs of two equal-length maps, or returns false when a key is missing.
fn push_map_pairs<'a>(a: &'a Map, b: &'a Map, stack: &mut Vec<(&'a Value, &'a Value)>) -> bool {
    if b.0.len() <= LINEAR_LOOKUP_MAX {
        for (k, v) in &a.0 {
            match b.get(k) {
                Some(w) => stack.push((v, w)),
                None => return false,
            }
        }
    } else {
        let index: HashMap<&str, &Value> = b.0.iter().map(|(k, v)| (k.as_str(), v)).collect();
        for (k, v) in &a.0 {
            match index.get(k.as_str()) {
                Some(w) => stack.push((v, w)),
                None => return false,
            }
        }
    }
    true
}

fn eq_pairs<'a>(mut stack: Vec<(&'a Value, &'a Value)>) -> bool {
    while let Some(pair) = stack.pop() {
        match pair {
            (Value::Null, Value::Null) => {}
            (Value::Bool(x), Value::Bool(y)) if x == y => {}
            (Value::Int(x), Value::Int(y)) if x == y => {}
            (Value::Float(x), Value::Float(y)) if x == y => {}
            (Value::Str(x), Value::Str(y)) if x == y => {}
            (Value::Array(x), Value::Array(y)) if x.len() == y.len() => {
                stack.extend(x.iter().zip(y));
            }
            (Value::Object(x), Value::Object(y)) if x.len() == y.len() => {
                if !push_map_pairs(x, y, &mut stack) {
                    return false;
                }
            }
            _ => return false,
        }
    }
    true
}

impl PartialEq for Value {
    /// Iterative. Floats compare as `f64` (NaN is unequal to itself); objects ignore order.
    fn eq(&self, other: &Value) -> bool {
        eq_pairs(vec![(self, other)])
    }
}

impl PartialEq for Map {
    fn eq(&self, other: &Map) -> bool {
        let mut stack = Vec::new();
        self.len() == other.len() && push_map_pairs(self, other, &mut stack) && eq_pairs(stack)
    }
}

impl Map {
    /// An empty object.
    pub fn new() -> Map {
        Map(Vec::new())
    }

    /// The value under `k`, if any.
    pub fn get(&self, k: &str) -> Option<&Value> {
        self.0.iter().find(|(key, _)| key == k).map(|(_, v)| v)
    }

    /// The value under `k`, mutably, if any.
    pub fn get_mut(&mut self, k: &str) -> Option<&mut Value> {
        self.0.iter_mut().find(|(key, _)| key == k).map(|(_, v)| v)
    }

    /// Whether `k` is a key.
    pub fn contains_key(&self, k: &str) -> bool {
        self.0.iter().any(|(key, _)| key == k)
    }

    /// Sets `k` to `v`: in place when `k` exists, else appended.
    pub fn insert(&mut self, k: String, v: Value) {
        match self.0.iter_mut().find(|(key, _)| *key == k) {
            Some(slot) => slot.1 = v,
            None => self.0.push((k, v)),
        }
    }

    /// The entries in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Removes `k` and returns its value, if any.
    pub fn remove(&mut self, k: &str) -> Option<Value> {
        let i = self.0.iter().position(|(key, _)| key == k)?;
        Some(self.0.remove(i).1)
    }

    /// The number of entries.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the object has no entries.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl IntoIterator for Map {
    type Item = (String, Value);
    type IntoIter = std::vec::IntoIter<(String, Value)>;

    /// The owned entries in insertion order.
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

/// A decode error: Python's exception message and position.
///
/// - `JSONDecodeError`: `msg` is `exc.msg` and `pos` is `exc.pos`, a code-point index into the
///   text that was parsed.
/// - `UnicodeDecodeError` ([`loads_utf8_sig`] only): `msg` is the whole `str(exc)` and `pos` is
///   `exc.start`, a byte index.
/// - `ValueError` (an int of more than 4300 digits): `msg` is the whole `str(exc)`; `pos` is the
///   code-point index of the number (Python has none).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsonError {
    pub msg: String,
    pub pos: usize,
}

/// The start of the message for an integer over Python's 4300-digit limit.
pub(crate) const INT_LIMIT_PREFIX: &str = "Exceeds the limit (4300 digits)";
/// The start of a `UnicodeDecodeError` message.
pub(crate) const UTF8_ERROR_PREFIX: &str = "'utf-8' codec can't decode";

impl JsonError {
    /// The Python exception class: `JSONDecodeError`, `UnicodeDecodeError` or `ValueError`.
    pub fn type_name(&self) -> &'static str {
        if self.msg.starts_with(INT_LIMIT_PREFIX) {
            "ValueError"
        } else if self.msg.starts_with(UTF8_ERROR_PREFIX) {
            "UnicodeDecodeError"
        } else {
            "JSONDecodeError"
        }
    }

    /// Python's `str(exc)`. For a `JSONDecodeError` that is `"%s: line %d column %d (char %d)"`,
    /// where `doc` is the text given to `loads` (after the BOM strip or lossy decode, for the
    /// byte entry points); for the other classes it is `msg`.
    pub fn py_str(&self, doc: &str) -> String {
        if self.type_name() != "JSONDecodeError" {
            return self.msg.clone();
        }
        let mut line = 1;
        let mut last_newline = None;
        for (i, c) in doc.chars().take(self.pos).enumerate() {
            if c == '\n' {
                line += 1;
                last_newline = Some(i);
            }
        }
        let column = match last_newline {
            Some(i) => self.pos - i,
            None => self.pos + 1,
        };
        format!(
            "{}: line {line} column {column} (char {})",
            self.msg, self.pos
        )
    }
}

impl fmt::Display for JsonError {
    /// `<msg> (char <pos>)`; use [`JsonError::py_str`] for Python's exact text.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (char {})", self.msg, self.pos)
    }
}

impl std::error::Error for JsonError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(text: &str) -> Value {
        Value::Str(text.to_string())
    }

    #[test]
    fn a_repeated_key_keeps_its_first_position_and_takes_the_last_value() {
        let mut m = Map::new();
        m.insert("a".into(), s("1"));
        m.insert("b".into(), s("2"));
        m.insert("a".into(), s("3"));
        let entries: Vec<_> = m.iter().collect();
        assert_eq!(entries, vec![("a", &s("3")), ("b", &s("2"))]);
    }

    #[test]
    fn equality_ignores_the_order_like_a_dict() {
        let mut a = Map::new();
        a.insert("x".into(), s("1"));
        a.insert("y".into(), s("2"));
        let mut b = Map::new();
        b.insert("y".into(), s("2"));
        b.insert("x".into(), s("1"));
        assert_eq!(a, b);
        b.insert("x".into(), s("3"));
        assert_ne!(a, b);
        b.remove("x");
        assert_ne!(a, b);
    }

    #[test]
    fn equality_of_large_maps_uses_the_index_and_still_ignores_the_order() {
        let mut a = Map::new();
        let mut b = Map::new();
        for i in 0..100 {
            a.insert(format!("k{i}"), Value::Int(Int::Small(i)));
            b.insert(format!("k{}", 99 - i), Value::Int(Int::Small(99 - i)));
        }
        assert_eq!(Value::Object(a.clone()), Value::Object(b.clone()));
        b.insert("k5".into(), Value::Null);
        assert_ne!(a, b);
        b.remove("k5");
        b.insert("other".into(), Value::Null);
        assert_ne!(a, b);
    }

    #[test]
    fn remove_returns_the_value_and_keeps_the_order() {
        let mut m = Map::new();
        for k in ["a", "b", "c"] {
            m.insert(k.into(), s(k));
        }
        assert_eq!(m.remove("b"), Some(s("b")));
        assert_eq!(m.remove("b"), None);
        assert_eq!(m.iter().map(|(k, _)| k).collect::<Vec<_>>(), vec!["a", "c"]);
        assert_eq!(m.len(), 2);
        assert!(!m.is_empty());
        assert_eq!(m.get("c"), Some(&s("c")));
        assert!(m.contains_key("a") && !m.contains_key("b"));
        *m.get_mut("a").unwrap() = s("z");
        let owned: Vec<(String, Value)> = m.into_iter().collect();
        assert_eq!(
            owned,
            vec![("a".to_string(), s("z")), ("c".to_string(), s("c"))]
        );
    }

    #[test]
    fn the_owning_helpers_move_the_payload_out() {
        let mut v = Value::Array(vec![s("a"), Value::Bool(true)]);
        let mut items = v.take().into_array().unwrap();
        assert!(v.is_null());
        assert_eq!(items[1].as_bool(), Some(true));
        assert_eq!(items[0].take().into_string(), Some("a".to_string()));
        assert_eq!(s("x").into_array(), None);
        let mut m = Map::new();
        m.insert("k".into(), Value::Int(Int::Small(7)));
        let obj = Value::Object(m);
        assert_eq!(obj.as_object().unwrap().get("k").unwrap().as_i64(), Some(7));
        assert_eq!(obj.into_object().unwrap().len(), 1);
        assert_eq!(s("q").as_str(), Some("q"));
        assert_eq!(Value::Null.into_string(), None);
    }

    #[test]
    fn floats_compare_as_f64_and_ints_by_form() {
        assert_ne!(Value::Float(f64::NAN), Value::Float(f64::NAN));
        assert_eq!(Value::Float(-0.0), Value::Float(0.0));
        assert_ne!(Value::Int(Int::Small(1)), Value::Float(1.0));
        assert_ne!(
            Value::Array(vec![Value::Null]),
            Value::Array(vec![Value::Null, Value::Null])
        );
    }

    #[test]
    fn clone_keeps_the_key_order_and_every_value() {
        let mut m = Map::new();
        m.insert("z".into(), Value::Array(vec![Value::Null, s("x")]));
        m.insert("a".into(), Value::Float(1.5));
        let v = Value::Object(m);
        let c = v.clone();
        assert_eq!(c, v);
        let keys: Vec<_> = c.as_object().unwrap().iter().map(|(k, _)| k).collect();
        assert_eq!(keys, vec!["z", "a"]);
    }

    #[test]
    fn py_str_counts_lines_and_columns_in_code_points() {
        let e = JsonError {
            msg: "Expecting value".into(),
            pos: 4,
        };
        assert_eq!(
            e.py_str("é\n\n é"),
            "Expecting value: line 3 column 2 (char 4)"
        );
        assert_eq!(
            e.py_str("éé  x"),
            "Expecting value: line 1 column 5 (char 4)"
        );
        assert_eq!(e.type_name(), "JSONDecodeError");
        let v = JsonError {
            msg: format!("{INT_LIMIT_PREFIX} for …"),
            pos: 0,
        };
        assert_eq!((v.type_name(), v.py_str("")), ("ValueError", v.msg.clone()));
        assert_eq!(e.to_string(), "Expecting value (char 4)");
    }
}
