//! The bits of Python semantics vendor.py relies on that `anthology_pycompat` does not publish.
//!
//! Items marked `P1:` stand in for a `pycompat` item of plan §3.2 until P1 lands; DV1d swaps
//! them for re-exports.

use std::fs;
use std::path::Path;
use std::sync::OnceLock;

use anthology_pycompat::json::{self, Int, Value};
use anthology_pycompat::pyerr::PyExc;

/// Python's `\w` in a str pattern: `str.isalnum()` or `_` (P1: `pycompat::re::W`).
pub const W: &str = r"[\p{L}\p{N}_]";
/// The inside of [`W`], for building classes such as `[\w-]` (P1: from `pycompat::re::W`).
pub const W_INNER: &str = r"\p{L}\p{N}_";
/// Python's `\d` in a str pattern: Unicode decimal digits, `Nd` (P1: `pycompat::re::D`).
pub const D: &str = r"\d";
/// Python's `\s` in a str pattern: `str.isspace()` (P1: `pycompat::re::S`).
pub const S: &str = r"[\s\x1c-\x1f]";

/// SWAP(P1): Python's IGNORECASE matches U+0130 (`İ`, whose simple lowercase is `i`) and
/// U+0131 (`ı`, an sre case equivalence of `i`) to `i` and `I`; Rust's simple case folding does
/// not. This rewrites each literal `i`/`I` of a `(?i)` pattern into a class holding all four.
/// Replace it with pycompat's fold once P1 provides one.
pub fn py_casefold_pattern(pattern: &str) -> String {
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::with_capacity(pattern.len() + 16);
    let mut in_class = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' => {
                out.push(c);
                if let Some(&e) = chars.get(i + 1) {
                    out.push(e);
                    i += 1;
                    if matches!(e, 'p' | 'P' | 'x') && chars.get(i + 1) == Some(&'{') {
                        while let Some(&b) = chars.get(i + 1) {
                            out.push(b);
                            i += 1;
                            if b == '}' {
                                break;
                            }
                        }
                    }
                }
            }
            '(' if !in_class && chars.get(i + 1) == Some(&'?') => {
                // A group's flags or kind (`(?i)`, `(?:`, `(?!`) are not literals.
                out.push_str("(?");
                i += 1;
                while let Some(&f) = chars.get(i + 1) {
                    if !(f.is_ascii_alphabetic() || f == '-') {
                        break;
                    }
                    out.push(f);
                    i += 1;
                }
            }
            '[' if !in_class => {
                in_class = true;
                out.push(c);
            }
            ']' if in_class => {
                in_class = false;
                out.push(c);
            }
            'i' | 'I' if in_class => {
                out.push(c);
                out.push_str("\u{130}\u{131}");
            }
            'i' | 'I' => out.push_str("[iI\u{130}\u{131}]"),
            _ => out.push(c),
        }
        i += 1;
    }
    out
}

/// `str.isspace()` for one code point (P1: `pycompat::str::is_space`).
pub fn is_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n'
            | '\u{0b}'
            | '\u{0c}'
            | '\r'
            | '\u{1c}'..='\u{1f}'
            | ' '
            | '\u{85}'
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
    )
}

/// `s.strip()` (P1: `pycompat::str::strip_ws`).
pub fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// `s.rstrip()`.
pub fn rstrip(s: &str) -> &str {
    s.trim_end_matches(is_space)
}

/// `s.lstrip()`.
pub fn lstrip(s: &str) -> &str {
    s.trim_start_matches(is_space)
}

/// `bytes.strip()`: ASCII whitespace (space, `\t\n\r\x0b\x0c`).
pub fn bytes_strip(b: &[u8]) -> &[u8] {
    let ws = |c: &u8| matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c);
    let start = b.iter().position(|c| !ws(c)).unwrap_or(b.len());
    let end = b.iter().rposition(|c| !ws(c)).map_or(start, |i| i + 1);
    &b[start..end]
}

/// `str.splitlines()` (no `keepends`): the line boundaries of `str.splitlines`.
pub fn splitlines(s: &str) -> Vec<&str> {
    splitlines_keepends(s)
        .into_iter()
        .map(|line| &line[..line.len() - line_break_len(line)])
        .collect()
}

/// The byte length of the line boundary that ends `line` (0 when none); `\r\n` is the only
/// two-character one.
pub fn line_break_len(line: &str) -> usize {
    if line.ends_with("\r\n") {
        return 2;
    }
    match line.chars().next_back() {
        Some(c) if is_line_break(c) => c.len_utf8(),
        _ => 0,
    }
}

/// A `str.splitlines()` boundary character.
pub fn is_line_break(c: char) -> bool {
    matches!(
        c,
        '\n' | '\r'
            | '\u{0b}'
            | '\u{0c}'
            | '\u{1c}'
            | '\u{1d}'
            | '\u{1e}'
            | '\u{85}'
            | '\u{2028}'
            | '\u{2029}'
    )
}

/// `str.splitlines(keepends=True)`.
pub fn splitlines_keepends(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if !is_line_break(c) {
            continue;
        }
        let mut end = i + c.len_utf8();
        if c == '\r'
            && let Some(&(_, '\n')) = chars.peek()
        {
            chars.next();
            end += 1;
        }
        out.push(&s[start..end]);
        start = end;
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// `Path.read_text(encoding="utf-8")`: strict UTF-8 and universal newlines (`\r\n` and `\r`
/// become `\n`). The error is Python's exception text.
pub fn read_text(path: &Path) -> Result<String, String> {
    let raw = fs::read(path).map_err(|e| os_error_text(&e, path))?;
    let text = decode_utf8(&raw)?;
    Ok(universal_newlines(&text))
}

/// `\r\n` and lone `\r` → `\n` (text-mode reading).
pub fn universal_newlines(text: &str) -> String {
    if !text.contains('\r') {
        return text.to_string();
    }
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// `bytes.decode("utf-8")`, with Python's `UnicodeDecodeError` text on failure.
pub fn decode_utf8(raw: &[u8]) -> Result<String, String> {
    match std::str::from_utf8(raw) {
        Ok(s) => Ok(s.to_string()),
        Err(e) => {
            let at = e.valid_up_to();
            let byte = raw[at];
            let reason = match e.error_len() {
                None => "unexpected end of data",
                Some(_) if (0x80..0xc2).contains(&byte) || byte >= 0xf5 => "invalid start byte",
                Some(_) => "invalid continuation byte",
            };
            let len = e.error_len().unwrap_or(raw.len() - at).max(1);
            if len > 1 {
                Err(format!(
                    "'utf-8' codec can't decode bytes in position {at}-{}: {reason}",
                    at + len - 1
                ))
            } else {
                Err(format!(
                    "'utf-8' codec can't decode byte 0x{byte:02x} in position {at}: {reason}"
                ))
            }
        }
    }
}

/// `str(OSError)` for a failed file operation: `[Errno N] <text>: '<path>'`.
pub fn os_error_text(e: &std::io::Error, path: &Path) -> String {
    let exc = match PyExc::from(std::io::Error::new(e.kind(), e.to_string())) {
        PyExc::OSError { errno, .. } => PyExc::OSError {
            errno: e.raw_os_error().unwrap_or(errno),
            filename: Some(path.to_string_lossy().into_owned()),
        },
        other => other,
    };
    exc.py_str()
}

/// `s.lower()` (P1: `pycompat::str::lower`).
pub fn lower(s: &str) -> String {
    s.to_lowercase()
}

/// The first `n` code points of `s` (`s[:n]`).
pub fn prefix(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// `posixpath.basename(p)`.
pub fn basename(p: &str) -> &str {
    match p.rfind('/') {
        Some(i) => &p[i + 1..],
        None => p,
    }
}

/// Python truthiness of a JSON value.
pub fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Int(Int::Small(n))) => *n != 0,
        Some(Value::Int(Int::Big(_))) => true,
        Some(Value::Float(f)) => *f != 0.0,
        Some(Value::Str(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(m)) => !m.is_empty(),
    }
}

/// Python's `a == b` for two JSON values (a missing value is `None`): numbers compare by value
/// across int, float and bool (`True == 1 == 1.0`), dicts ignore key order.
pub fn py_eq(a: Option<&Value>, b: Option<&Value>) -> bool {
    let null = Value::Null;
    let mut stack = vec![(a.unwrap_or(&null), b.unwrap_or(&null))];
    while let Some((a, b)) = stack.pop() {
        let same = match (a, b) {
            (Value::Null, Value::Null) => true,
            (Value::Str(x), Value::Str(y)) => x == y,
            (Value::Array(x), Value::Array(y)) => {
                if x.len() != y.len() {
                    return false;
                }
                stack.extend(x.iter().zip(y.iter()));
                true
            }
            (Value::Object(x), Value::Object(y)) => {
                if x.len() != y.len() {
                    return false;
                }
                for (k, v) in x.iter() {
                    match y.get(k) {
                        Some(w) => stack.push((v, w)),
                        None => return false,
                    }
                }
                true
            }
            (Value::Int(Int::Big(x)), Value::Int(Int::Big(y))) => x == y,
            (x, y) => match (number(x), number(y)) {
                (Some(Num::Exact(p)), Some(Num::Exact(q))) => p == q,
                (Some(Num::Exact(p)), Some(Num::Float(q)))
                | (Some(Num::Float(q)), Some(Num::Exact(p))) => int_eq_float(p, q),
                (Some(Num::Float(p)), Some(Num::Float(q))) => p == q,
                (Some(Num::Big(s)), Some(Num::Float(f)))
                | (Some(Num::Float(f)), Some(Num::Big(s))) => {
                    f.is_finite() && f.fract() == 0.0 && format!("{f:.0}") == s
                }
                _ => false,
            },
        };
        if !same {
            return false;
        }
    }
    true
}

enum Num<'a> {
    Exact(i64),
    Big(&'a str),
    Float(f64),
}

fn number(v: &Value) -> Option<Num<'_>> {
    match v {
        Value::Bool(b) => Some(Num::Exact(i64::from(*b))),
        Value::Int(Int::Small(n)) => Some(Num::Exact(*n)),
        Value::Int(Int::Big(s)) => Some(Num::Big(s)),
        Value::Float(f) => Some(Num::Float(*f)),
        _ => None,
    }
}

/// `int == float`, exactly (Python compares the values, not a rounded float).
fn int_eq_float(i: i64, f: f64) -> bool {
    if !f.is_finite() || f.fract() != 0.0 || !(-9.3e18..9.3e18).contains(&f) {
        return false;
    }
    // In range: the float is an integer that fits in i128 exactly.
    i128::from(i) == f as i128
}

/// `f"{v}"` (`str(v)`): a str is itself, anything else its repr; a missing value is `None`.
pub fn str_of(v: Option<&Value>) -> String {
    match v {
        Some(Value::Str(s)) => s.clone(),
        Some(v) => repr(v),
        None => "None".to_string(),
    }
}

/// `repr(v)` of an optional value (`None` when missing, as `dict.get` gives).
pub fn repr_opt(v: Option<&Value>) -> String {
    v.map_or_else(|| "None".to_string(), repr)
}

/// `repr(v)` of a JSON value, as Python prints the object `json.loads` returned.
pub fn repr(v: &Value) -> String {
    let mut out = String::new();
    write_repr(v, &mut out);
    out
}

fn write_repr(v: &Value, out: &mut String) {
    // Iterative over nesting, so a deep value cannot overflow the stack.
    enum Step<'a> {
        Value(&'a Value),
        Text(&'static str),
        Key(&'a str),
    }
    let mut stack = vec![Step::Value(v)];
    while let Some(step) = stack.pop() {
        match step {
            Step::Text(t) => out.push_str(t),
            Step::Key(k) => {
                out.push_str(&repr_str(k));
                out.push_str(": ");
            }
            Step::Value(v) => match v {
                Value::Null => out.push_str("None"),
                Value::Bool(true) => out.push_str("True"),
                Value::Bool(false) => out.push_str("False"),
                Value::Int(Int::Small(n)) => out.push_str(&n.to_string()),
                Value::Int(Int::Big(s)) => out.push_str(s),
                Value::Float(f) => out.push_str(&repr_float(*f)),
                Value::Str(s) => out.push_str(&repr_str(s)),
                Value::Array(items) => {
                    out.push('[');
                    stack.push(Step::Text("]"));
                    for (i, item) in items.iter().enumerate().rev() {
                        stack.push(Step::Value(item));
                        if i > 0 {
                            stack.push(Step::Text(", "));
                        }
                    }
                }
                Value::Object(map) => {
                    out.push('{');
                    stack.push(Step::Text("}"));
                    let pairs: Vec<(&str, &Value)> = map.iter().collect();
                    for (i, (k, item)) in pairs.into_iter().enumerate().rev() {
                        stack.push(Step::Value(item));
                        stack.push(Step::Key(k));
                        if i > 0 {
                            stack.push(Step::Text(", "));
                        }
                    }
                }
            },
        }
    }
}

/// `repr(float)`.
pub fn repr_float(f: f64) -> String {
    if f.is_nan() {
        "nan".to_string()
    } else if f.is_infinite() {
        if f > 0.0 { "inf" } else { "-inf" }.to_string()
    } else {
        json::dumps(&Value::Float(f), &json::PY_DEFAULT)
    }
}

/// Python's `str.isprintable()` for one code point.
pub fn is_printable(c: char) -> bool {
    static NOT_PRINTABLE: OnceLock<regex::Regex> = OnceLock::new();
    if c == ' ' {
        return true;
    }
    let re = NOT_PRINTABLE.get_or_init(|| {
        regex::Regex::new(r"^[\p{Cc}\p{Cf}\p{Co}\p{Cn}\p{Zl}\p{Zp}\p{Zs}]$")
            .unwrap_or_else(|_| unreachable!("a fixed pattern"))
    });
    let mut buf = [0u8; 4];
    !re.is_match(c.encode_utf8(&mut buf))
}

/// `repr(str)`.
pub fn repr_str(s: &str) -> String {
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
            c if is_printable(c) => out.push(c),
            c if (c as u32) < 0x100 => out.push_str(&format!("\\x{:02x}", c as u32)),
            c if (c as u32) < 0x10000 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push_str(&format!("\\U{:08x}", c as u32)),
        }
    }
    out.push(quote);
    out
}

/// `repr(bytes)`.
pub fn repr_bytes(b: &[u8]) -> String {
    let quote = if b.contains(&b'\'') && !b.contains(&b'"') {
        b'"'
    } else {
        b'\''
    };
    let mut out = String::from("b");
    out.push(quote as char);
    for &c in b {
        match c {
            b'\\' => out.push_str("\\\\"),
            b'\t' => out.push_str("\\t"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            c if c == quote => {
                out.push('\\');
                out.push(c as char);
            }
            0x20..=0x7e => out.push(c as char),
            c => out.push_str(&format!("\\x{c:02x}")),
        }
    }
    out.push(quote as char);
    out
}

/// The `str` of a list of strings, as Python prints it (`['a', 'b']`).
pub fn repr_str_list(items: &[&str]) -> String {
    let parts: Vec<String> = items.iter().map(|s| repr_str(s)).collect();
    format!("[{}]", parts.join(", "))
}

/// One piece of a parsed `re.sub` replacement template.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece {
    Lit(String),
    /// `\g<0>`: the whole match.
    Whole,
}

/// `re._parser.parse_template` for a pattern with no groups: Python's replacement-template
/// escapes (`\n`, `\\`, octal, `\g<0>`). An error is the `re.error` Python raises (D-1).
pub fn parse_template(t: &str) -> Result<Vec<Piece>, String> {
    let chars: Vec<char> = t.chars().collect();
    let mut out = Vec::new();
    let mut lit = String::new();
    let octal = |c: char| ('0'..='7').contains(&c);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c != '\\' {
            lit.push(c);
            i += 1;
            continue;
        }
        let Some(&d) = chars.get(i + 1) else {
            return Err(format!("bad escape (end of pattern) at position {i}"));
        };
        i += 2;
        match d {
            'g' => {
                if chars.get(i) != Some(&'<') {
                    return Err(format!("missing < at position {i}"));
                }
                let start = i + 1;
                let Some(len) = chars[start..].iter().position(|&c| c == '>') else {
                    return Err("missing >, unterminated name".to_string());
                };
                let name: String = chars[start..start + len].iter().collect();
                i = start + len + 1;
                if name.is_empty() {
                    return Err("missing group name".to_string());
                }
                // 3.12 accepts only ASCII digits as a group number (gh-91760).
                let number = if name.bytes().all(|b| b.is_ascii_digit()) {
                    name.parse::<u64>().map_err(|_| ())
                } else {
                    Err(())
                };
                match number {
                    Ok(0) => {
                        if !lit.is_empty() {
                            out.push(Piece::Lit(std::mem::take(&mut lit)));
                        }
                        out.push(Piece::Whole);
                    }
                    Ok(n) => return Err(format!("invalid group reference {n}")),
                    Err(_) if is_identifier(&name) => {
                        return Err(format!("unknown group name {}", repr_str(&name)));
                    }
                    Err(_) => {
                        return Err(format!("bad character in group name {}", repr_str(&name)));
                    }
                }
            }
            '0' => {
                let mut value = 0u32;
                let mut taken = 0;
                while taken < 2 && chars.get(i).is_some_and(|&c| octal(c)) {
                    value = value * 8 + chars[i].to_digit(8).unwrap_or(0);
                    i += 1;
                    taken += 1;
                }
                lit.push(char::from_u32(value & 0xff).unwrap_or('\0'));
            }
            '1'..='9' => {
                let second = chars.get(i).copied().filter(char::is_ascii_digit);
                if let Some(e) = second {
                    i += 1;
                    if octal(d) && octal(e) && chars.get(i).is_some_and(|&c| octal(c)) {
                        let f = chars[i];
                        i += 1;
                        let value = [d, e, f]
                            .iter()
                            .fold(0u32, |v, c| v * 8 + c.to_digit(8).unwrap_or(0));
                        if value > 0o377 {
                            return Err(format!(
                                "octal escape value \\{d}{e}{f} outside of range 0-0o377"
                            ));
                        }
                        lit.push(char::from_u32(value).unwrap_or('\0'));
                        continue;
                    }
                    let n = d.to_digit(10).unwrap_or(0) * 10 + e.to_digit(10).unwrap_or(0);
                    return Err(format!("invalid group reference {n}"));
                }
                return Err(format!("invalid group reference {d}"));
            }
            'a' => lit.push('\u{07}'),
            'b' => lit.push('\u{08}'),
            'f' => lit.push('\u{0c}'),
            'n' => lit.push('\n'),
            'r' => lit.push('\r'),
            't' => lit.push('\t'),
            'v' => lit.push('\u{0b}'),
            '\\' => lit.push('\\'),
            d if d.is_ascii_alphabetic() => return Err(format!("bad escape \\{d}")),
            d => {
                lit.push('\\');
                lit.push(d);
            }
        }
    }
    if !lit.is_empty() {
        out.push(Piece::Lit(lit));
    }
    Ok(out)
}

/// `str.isidentifier()`, close enough for template error texts.
fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c == '_' || c.is_alphabetic())
        && chars.all(|c| c == '_' || c.is_alphanumeric())
}

/// Expands a parsed template for one match.
pub fn expand(pieces: &[Piece], whole: &str) -> String {
    let mut out = String::new();
    for p in pieces {
        match p {
            Piece::Lit(s) => out.push_str(s),
            Piece::Whole => out.push_str(whole),
        }
    }
    out
}

/// Latin-1 decoding: each byte is the code point of the same value.
pub fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| char::from(c)).collect()
}

/// Latin-1 encoding of a string whose code points are all below 256.
pub fn unlatin1(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?'))
        .collect()
}

/// A `json::Map` built from `(key, int)` pairs, for the stats dicts.
pub fn stats(pairs: &[(&str, i64)]) -> json::Map {
    let mut m = json::Map::new();
    for (k, v) in pairs {
        m.insert((*k).to_string(), Value::Int(Int::Small(*v)));
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitlines_matches_python() {
        // "a\r\nb\rc\x0bd\x1ce f\n\ng\n".splitlines()
        assert_eq!(
            splitlines("a\r\nb\rc\u{0b}d\u{1c}e\u{2028}f\n\ng\n"),
            vec!["a", "b", "c", "d", "e", "f", "", "g"]
        );
        assert_eq!(splitlines(""), Vec::<&str>::new());
        assert_eq!(splitlines("\n"), vec![""]);
        assert_eq!(splitlines("x"), vec!["x"]);
        assert_eq!(splitlines("x\r"), vec!["x"]);
        assert_eq!(splitlines_keepends("a\r\nb"), vec!["a\r\n", "b"]);
    }

    #[test]
    fn strip_uses_python_whitespace() {
        assert_eq!(strip("\u{1c}\u{a0} x \u{3000}\u{85}"), "x");
        assert_eq!(rstrip("x\u{1f}"), "x");
        // U+200B is not whitespace for Python.
        assert_eq!(strip("\u{200b}x"), "\u{200b}x");
        assert_eq!(bytes_strip(b" \x0b\x0cab \x1c"), b"ab \x1c");
        assert_eq!(bytes_strip(b" \t "), b"");
    }

    #[test]
    fn repr_matches_python() {
        assert_eq!(repr_str("abc"), "'abc'");
        assert_eq!(repr_str("a'b"), "\"a'b\"");
        assert_eq!(repr_str("a'b\""), "'a\\'b\"'");
        assert_eq!(
            repr_str("t\tn\n\\\u{7f}\u{a0}é\u{200b}\u{1f600}"),
            "'t\\tn\\n\\\\\\x7f\\xa0é\\u200b\u{1f600}'"
        );
        assert_eq!(repr_str("\u{e000}\u{f0000}"), "'\\ue000\\U000f0000'");
        let v = json::loads(r#"{"a": [1, 2.5, null, true, "x"], "b": {}, "c": 1e16}"#).unwrap();
        assert_eq!(
            repr(&v),
            "{'a': [1, 2.5, None, True, 'x'], 'b': {}, 'c': 1e+16}"
        );
        assert_eq!(repr_bytes(b"a'\x00\xff"), "b\"a'\\x00\\xff\"");
    }

    #[test]
    fn utf8_errors_read_like_python() {
        assert_eq!(
            decode_utf8(b"ab\xff").unwrap_err(),
            "'utf-8' codec can't decode byte 0xff in position 2: invalid start byte"
        );
        assert_eq!(
            decode_utf8(b"a\xc3").unwrap_err(),
            "'utf-8' codec can't decode byte 0xc3 in position 1: unexpected end of data"
        );
        assert_eq!(
            decode_utf8(b"a\xe2\x82").unwrap_err(),
            "'utf-8' codec can't decode bytes in position 1-2: unexpected end of data"
        );
        assert_eq!(
            decode_utf8(b"a\xc3x").unwrap_err(),
            "'utf-8' codec can't decode byte 0xc3 in position 1: invalid continuation byte"
        );
        assert_eq!(
            decode_utf8(b"\xf0\x9fx").unwrap_err(),
            "'utf-8' codec can't decode bytes in position 0-1: invalid continuation byte"
        );
        assert_eq!(
            decode_utf8(b"\xed\xa0\x80").unwrap_err(),
            "'utf-8' codec can't decode byte 0xed in position 0: invalid continuation byte"
        );
    }

    #[test]
    fn casefold_pattern_rewrites_literal_i_only() {
        assert_eq!(
            py_casefold_pattern(r"(?i)\Aif\b[\p{L}i](?:x)\x{69}"),
            "(?i)\\A[iI\u{130}\u{131}]f\\b[\\p{L}i\u{130}\u{131}](?:x)\\x{69}"
        );
    }

    #[test]
    fn truthiness() {
        assert!(!truthy(None));
        assert!(!truthy(Some(&Value::Str(String::new()))));
        assert!(truthy(Some(&Value::Str("x".into()))));
        assert!(!truthy(Some(&Value::Int(Int::Small(0)))));
        assert!(!truthy(Some(&Value::Float(0.0))));
    }
}
