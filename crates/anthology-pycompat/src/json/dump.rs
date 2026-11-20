//! `json.dumps` (F2): CPython's encoder output under the GC-4 presets, iterative.

use super::float::py_repr;
use super::{Int, Value};

/// `json.dumps` keyword arguments (GC-4). `item_sep`/`key_sep` are `separators`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DumpOpts {
    pub indent: Option<usize>,
    pub sort_keys: bool,
    pub ensure_ascii: bool,
    pub item_sep: &'static str,
    pub key_sep: &'static str,
}

/// `json.dumps(v)`.
pub const PY_DEFAULT: DumpOpts = DumpOpts {
    indent: None,
    sort_keys: false,
    ensure_ascii: true,
    item_sep: ", ",
    key_sep: ": ",
};
/// `json.dumps(v, sort_keys=True)`.
pub const PY_DEFAULT_SORTED: DumpOpts = DumpOpts {
    sort_keys: true,
    ..PY_DEFAULT
};
/// `json.dumps(v, ensure_ascii=False)`.
pub const PY_DEFAULT_UNICODE: DumpOpts = DumpOpts {
    ensure_ascii: false,
    ..PY_DEFAULT
};
/// `json.dumps(v, indent=2)` (an indent makes the item separator `","`).
pub const INDENT2: DumpOpts = DumpOpts {
    indent: Some(2),
    item_sep: ",",
    ..PY_DEFAULT
};
/// `json.dumps(v, indent=2, sort_keys=True)`.
pub const INDENT2_SORTED: DumpOpts = DumpOpts {
    sort_keys: true,
    ..INDENT2
};
/// `json.dumps(v, indent=2, sort_keys=True, ensure_ascii=False)`: state.json (GC-3).
pub const STATE: DumpOpts = DumpOpts {
    ensure_ascii: false,
    ..INDENT2_SORTED
};
/// `json.dumps(v, sort_keys=True, separators=(",", ":"))`.
pub const COMPACT_SORTED: DumpOpts = DumpOpts {
    sort_keys: true,
    item_sep: ",",
    key_sep: ":",
    ..PY_DEFAULT
};

/// Every GC-4 preset by its name (the difftest and golden cases name presets).
pub const PRESETS: [(&str, DumpOpts); 7] = [
    ("PY_DEFAULT", PY_DEFAULT),
    ("PY_DEFAULT_SORTED", PY_DEFAULT_SORTED),
    ("PY_DEFAULT_UNICODE", PY_DEFAULT_UNICODE),
    ("INDENT2", INDENT2),
    ("INDENT2_SORTED", INDENT2_SORTED),
    ("STATE", STATE),
    ("COMPACT_SORTED", COMPACT_SORTED),
];

/// `json.dumps(v, **o)`. Iterative; any depth.
pub fn dumps(v: &Value, o: &DumpOpts) -> String {
    let mut w = Writer {
        out: String::new(),
        o,
        stack: Vec::new(),
    };
    w.open(v, 0);
    while let Some(frame) = w.stack.last_mut() {
        let level = frame.level;
        let next = match &mut frame.items {
            Items::Array(it) => it.next().map(|v| (None, v)),
            Items::Object(it) => it.next().map(|(k, v)| (Some(k), v)),
        };
        match next {
            Some((key, value)) => {
                if frame.first {
                    frame.first = false;
                } else {
                    w.out.push_str(o.item_sep);
                }
                w.newline(level);
                if let Some(k) = key {
                    write_str(&mut w.out, k, o.ensure_ascii);
                    w.out.push_str(o.key_sep);
                }
                w.open(value, level);
            }
            None => {
                let close = match frame.items {
                    Items::Array(_) => ']',
                    Items::Object(_) => '}',
                };
                w.stack.pop();
                w.newline(level - 1);
                w.out.push(close);
            }
        }
    }
    w.out
}

enum Items<'a> {
    Array(std::slice::Iter<'a, Value>),
    Object(std::vec::IntoIter<(&'a str, &'a Value)>),
}

struct Frame<'a> {
    items: Items<'a>,
    /// The indent level of the items.
    level: usize,
    first: bool,
}

struct Writer<'a, 'o> {
    out: String,
    o: &'o DumpOpts,
    stack: Vec<Frame<'a>>,
}

impl<'a> Writer<'a, '_> {
    fn newline(&mut self, level: usize) {
        if let Some(n) = self.o.indent {
            self.out.push('\n');
            self.out.extend(std::iter::repeat_n(' ', n * level));
        }
    }

    /// Writes a scalar or an empty container, or opens a container at `level`.
    fn open(&mut self, v: &'a Value, level: usize) {
        match v {
            Value::Null => self.out.push_str("null"),
            Value::Bool(true) => self.out.push_str("true"),
            Value::Bool(false) => self.out.push_str("false"),
            Value::Int(Int::Small(i)) => self.out.push_str(&i.to_string()),
            Value::Int(Int::Big(s)) => self.out.push_str(s),
            Value::Float(f) => self.out.push_str(&py_repr(*f)),
            Value::Str(s) => write_str(&mut self.out, s, self.o.ensure_ascii),
            Value::Array(a) if a.is_empty() => self.out.push_str("[]"),
            Value::Object(m) if m.is_empty() => self.out.push_str("{}"),
            Value::Array(a) => {
                self.out.push('[');
                self.stack.push(Frame {
                    items: Items::Array(a.iter()),
                    level: level + 1,
                    first: true,
                });
            }
            Value::Object(m) => {
                self.out.push('{');
                let mut entries: Vec<(&str, &Value)> = m.iter().collect();
                if self.o.sort_keys {
                    // UTF-8 byte order is code-point order, as Python's `sorted` uses.
                    entries.sort_unstable_by(|a, b| a.0.cmp(b.0));
                }
                self.stack.push(Frame {
                    items: Items::Object(entries.into_iter()),
                    level: level + 1,
                    first: true,
                });
            }
        }
    }
}

/// A JSON string: `encode_basestring_ascii` (escape all but printable ASCII; astral characters as
/// surrogate pairs, lowercase hex) or `encode_basestring` (escape `"`, `\` and controls only).
pub(crate) fn write_str(out: &mut String, s: &str, ensure_ascii: bool) {
    out.push('"');
    let mut run = 0;
    for (i, c) in s.char_indices() {
        let short = match c {
            '"' => Some("\\\""),
            '\\' => Some("\\\\"),
            '\n' => Some("\\n"),
            '\r' => Some("\\r"),
            '\t' => Some("\\t"),
            '\u{8}' => Some("\\b"),
            '\u{c}' => Some("\\f"),
            _ => None,
        };
        let plain = short.is_none()
            && if ensure_ascii {
                (' '..='~').contains(&c)
            } else {
                c >= ' '
            };
        if plain {
            continue;
        }
        out.push_str(&s[run..i]);
        run = i + c.len_utf8();
        match short {
            Some(e) => out.push_str(e),
            None => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
        }
    }
    out.push_str(&s[run..]);
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::super::{Map, loads};
    use super::*;

    fn d(text: &str, o: &DumpOpts) -> String {
        dumps(&loads(text).unwrap(), o)
    }

    #[test]
    fn presets_print_like_python() {
        let text = r#"{"b": [1, {}], "a": "é😀\u0001", "c": []}"#;
        assert_eq!(
            d(text, &PY_DEFAULT),
            r#"{"b": [1, {}], "a": "\u00e9\ud83d\ude00\u0001", "c": []}"#
        );
        assert_eq!(
            d(text, &PY_DEFAULT_UNICODE),
            "{\"b\": [1, {}], \"a\": \"é😀\\u0001\", \"c\": []}"
        );
        assert_eq!(
            d(text, &COMPACT_SORTED),
            r#"{"a":"\u00e9\ud83d\ude00\u0001","b":[1,{}],"c":[]}"#
        );
        assert_eq!(
            d(text, &STATE),
            "{\n  \"a\": \"é😀\\u0001\",\n  \"b\": [\n    1,\n    {}\n  ],\n  \"c\": []\n}"
        );
        assert_eq!(d("\"\u{7f}\"", &PY_DEFAULT), r#""\u007f""#);
        assert_eq!(d("\"\u{7f}\"", &PY_DEFAULT_UNICODE), "\"\u{7f}\"");
    }

    #[test]
    fn indent_zero_still_breaks_lines() {
        let o = DumpOpts {
            indent: Some(0),
            ..INDENT2
        };
        assert_eq!(d("[1, [2]]", &o), "[\n1,\n[\n2\n]\n]");
        let mut m = Map::new();
        m.insert("k".into(), Value::Null);
        assert_eq!(dumps(&Value::Object(m), &INDENT2), "{\n  \"k\": null\n}");
    }
}
