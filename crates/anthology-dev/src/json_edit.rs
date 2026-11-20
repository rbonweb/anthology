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

// ---- minimal in-place edits (release_edit's `_tokens`, `_member_value`, `_replace_string_value`) --

/// A token of [`tokens`]: `kind` is `'s'` for a string, or the structural character itself
/// (`{ } [ ] : ,`); `start..end` are byte offsets into the text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: char,
    pub start: usize,
    pub end: usize,
}

/// A failed edit: `release_edit` turns it into a `ReleaseError` with this message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditError(pub String);

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for EditError {}

/// `_tokens`: the structural characters and the strings of `text`, with their offsets. It does
/// not validate the JSON; the callers parse it first. Only ASCII characters matter to the scan,
/// so it walks bytes (a multi-byte character holds no ASCII byte).
pub fn tokens(text: &str) -> Result<Vec<Token>, EditError> {
    let b = text.as_bytes();
    let n = b.len();
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        match b[i] {
            b'"' => {
                let mut j = i + 1;
                while j < n && b[j] != b'"' {
                    j += if b[j] == b'\\' { 2 } else { 1 };
                }
                if j >= n {
                    return Err(EditError("unterminated string in JSON".into()));
                }
                out.push(Token {
                    kind: 's',
                    start: i,
                    end: j + 1,
                });
                i = j + 1;
                continue;
            }
            c @ (b'{' | b'}' | b'[' | b']' | b':' | b',') => out.push(Token {
                kind: char::from(c),
                start: i,
                end: i + 1,
            }),
            _ => {}
        }
        i += 1;
    }
    Ok(out)
}

/// `_member_value`: the token index of the value of `key` directly inside the object (or
/// array) opened at token `open_idx`; the first member with that key.
pub fn member_value(text: &str, toks: &[Token], open_idx: usize, key: &str) -> Option<usize> {
    let mut depth = 0i64;
    for (k, &t) in toks.iter().enumerate().skip(open_idx) {
        match t.kind {
            '{' | '[' => depth += 1,
            '}' | ']' => {
                depth -= 1;
                if depth == 0 {
                    return None;
                }
            }
            's' if depth == 1 && k + 2 < toks.len() && toks[k + 1].kind == ':' => {
                // A string token of a document that parsed always decodes; one that does
                // not is no match.
                let is_key = matches!(
                    json::loads(&text[t.start..t.end]),
                    Ok(Value::Str(ref s)) if s == key
                );
                if is_key {
                    return Some(k + 2);
                }
            }
            _ => {}
        }
    }
    None
}

/// `_replace_string_value`: `text` with the string token `idx` replaced by `json.dumps(new)`
/// (ASCII only, as `ensure_ascii` makes it). `what` names the value in the error.
pub fn replace_string_value(
    text: &str,
    toks: &[Token],
    idx: usize,
    new: &str,
    what: &str,
) -> Result<String, EditError> {
    let t = toks[idx];
    if t.kind != 's' {
        return Err(EditError(format!("{what} is not a string")));
    }
    let dumped = json::dumps(&Value::Str(new.to_string()), &PY_DEFAULT);
    Ok(format!("{}{}{}", &text[..t.start], dumped, &text[t.end..]))
}

// ---- uncaught Python exceptions (ruling R10) ---------------------------------------------------

/// A Python exception the script does not catch: Python prints a traceback and exits 1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Uncaught {
    pub ty: String,
    pub msg: String,
}

impl Uncaught {
    pub fn new(ty: impl Into<String>, msg: impl Into<String>) -> Uncaught {
        Uncaught {
            ty: ty.into(),
            msg: msg.into(),
        }
    }

    /// An `OSError` (or one of its subclasses) as `PyExc` describes it.
    pub fn from_exc(e: &anthology_pycompat::pyerr::PyExc) -> Uncaught {
        Uncaught::new(e.type_name(), e.py_str())
    }

    /// Exit 1 with `anthology-dev <tool>: <PyType>: <message>` on stderr; `stdout` is what the
    /// script had printed before it raised.
    pub fn into_output(self, tool: &str, stdout: Vec<u8>) -> crate::Output {
        crate::Output {
            stdout,
            stderr: format!("anthology-dev {tool}: {}: {}\n", self.ty, self.msg).into_bytes(),
            code: 1,
        }
    }
}

/// `UnicodeDecodeError`'s `str(exc)` for the first invalid byte of `bytes` decoded as UTF-8
/// (`None` when `bytes` is valid UTF-8).
pub fn utf8_decode_error(bytes: &[u8]) -> Option<String> {
    let e = std::str::from_utf8(bytes).err()?;
    let start = e.valid_up_to();
    let first = bytes[start];
    let (end, reason) = match e.error_len() {
        _ if (0x80..=0xc1).contains(&first) || first >= 0xf5 => (start + 1, "invalid start byte"),
        Some(len) => (start + len, "invalid continuation byte"),
        None => (bytes.len(), "unexpected end of data"),
    };
    Some(if end - start == 1 {
        format!("'utf-8' codec can't decode byte 0x{first:02x} in position {start}: {reason}")
    } else {
        format!(
            "'utf-8' codec can't decode bytes in position {start}-{}: {reason}",
            end - 1
        )
    })
}

/// Text mode's universal newlines: `\r\n` and a lone `\r` become `\n`.
pub fn universal_newlines(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
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
