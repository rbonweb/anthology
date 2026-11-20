//! `pycompat::json`: Python's `json` module, byte for byte (plan §3.1, GC-4).
//!
//! F1 declares the value types, so `anthology_guard::data` can build its sample. F2 adds
//! `loads`, `dumps`, the `DumpOpts` presets and `js_stringify`.

mod dump;
mod float;
mod js;
mod parse;

/// A JSON value, as Python's `json.loads` returns it.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(Int),
    Float(f64),
    Str(String),
    Array(Vec<Value>),
    Object(Map),
}

/// A JSON integer: `Small` when it fits an `i64`, else `Big` with its decimal text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Int {
    Small(i64),
    Big(String),
}

/// A JSON object in insertion order. A repeated key keeps its first position and takes the
/// last value, as a Python `dict` does.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Map(Vec<(String, Value)>);

impl Map {
    /// An empty object.
    pub fn new() -> Map {
        Map(Vec::new())
    }

    /// The value under `k`, if any.
    pub fn get(&self, k: &str) -> Option<&Value> {
        self.0.iter().find(|(key, _)| key == k).map(|(_, v)| v)
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

/// A decode error: Python's `JSONDecodeError` message and character position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsonError {
    pub msg: String,
    pub pos: usize,
}

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
    }
}
