//! The bits of Python semantics vendor.py relies on that `anthology_pycompat` does not publish,
//! plus re-exports of the `pycompat` items the vendor tool shares.
//!
//! Not in pycompat, so kept here: `rstrip`/`lstrip`, `bytes.strip`, `str.splitlines`, a bytes-pattern
//! `re` ([`compile_bytes_ignorecase`]: pycompat's `re` takes str subjects, and vendor.py matches
//! the raw bytes of a file), the `re.sub` template parser, and the `repr` of bytes and JSON values.

use std::fs;
use std::path::Path;

use anthology_pycompat::json::{self, Int, Value};
use anthology_pycompat::pyerr::PyExc;

pub use anthology_pycompat::difflib::unified_diff_lineterm;
pub use anthology_pycompat::quote::quote;
pub use anthology_pycompat::re::{D, S, W};
pub use anthology_pycompat::str::{is_space, lower, repr as repr_str, strip_ws as strip};
/// The inside of [`W`], for building classes such as `[\w-]`.
pub use anthology_pycompat::tables::WORD_ITEMS as W_INNER;

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
    // An error without an OS code gets the errno its kind maps to.
    let errno = match PyExc::from(std::io::Error::from(e.kind())) {
        PyExc::OSError { errno, .. } => errno,
        _ => 0,
    };
    PyExc::OSError {
        errno: e.raw_os_error().unwrap_or(errno),
        filename: Some(path.to_string_lossy().into_owned()),
    }
    .py_str()
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

/// A Python bytes pattern (the UTF-8 of the str) in fancy-regex syntax, to compile in
/// fancy-regex's ASCII bytes mode, whose classes are already Python's. pycompat's `re` matches
/// str subjects only, so it cannot run a pattern over the raw bytes of a file; str patterns go
/// through `pycompat::re::compile`.
///
/// Translated: `$` (no MULTILINE) also matches before a final `\n`; `\Z` is `\z`; `\B` does not
/// match an empty subject; `{,n}` is `{0,n}`; a `{` or `}` that is not a repeat is a literal;
/// escaped punctuation and `\0` octal escapes become `\x{…}` literals; `\b` in a class is a
/// backspace; `&&`/`~~` in a class stay literal; in a bytes pattern every non-ASCII character is
/// its UTF-8 bytes, so a quantifier repeats the last byte as Python's does.
///
/// Refused rather than reinterpreted (Python's text where it raises, else "not supported"):
/// - an escape Python rejects (`bad escape \X at position N`), `\N{…}`, backreferences and
///   other octal escapes, `--` in a class;
/// - a quantifier with nothing to repeat, a quantifier after a quantifier (a lazy `?` or
///   possessive `+` suffix is allowed), `min repeat greater than max repeat`;
/// - in a look-behind, any quantifier but `{n}`, and alternation (Python needs a fixed width);
/// - inline flags (`(?m)` would change `$`), `(?P=…)`, `(?<name>…)`, conditionals, comments.
///
/// `\b`/`\B` keep fancy-regex's word definition, which differs from Python's `\w` only around
/// non-ASCII marks and numbers. Regex error texts are fancy-regex's, not Python's.
fn translate_bytes(py: &str) -> Result<String, String> {
    let chars: Vec<char> = py.chars().collect();
    let mut out = String::with_capacity(py.len() + 8);
    let mut in_class = false;
    // Open groups: whether each is a look-behind.
    let mut groups: Vec<bool> = Vec::new();
    // Whether a quantifier may follow (an atom precedes), one just did, or a lazy/possessive
    // suffix just did.
    let (mut can_repeat, mut after_quant, mut after_suffix) = (false, false, false);
    let mut i = 0;
    let hex = |from: usize, n: usize| -> Option<u32> {
        let digits: String = chars.get(from..from + n)?.iter().collect();
        if digits.chars().all(|c| c.is_ascii_hexdigit()) {
            u32::from_str_radix(&digits, 16).ok()
        } else {
            None
        }
    };
    // A literal character: a `\x{…}` escape, one per UTF-8 byte.
    let literal = |out: &mut String, code: u32| {
        let c = char::from_u32(code).unwrap_or('\u{fffd}');
        if code < 0x80 {
            out.push_str(&format!("\\x{{{code:x}}}"));
        } else {
            let mut buf = [0u8; 4];
            for b in c.encode_utf8(&mut buf).bytes() {
                out.push_str(&format!("\\x{{{b:x}}}"));
            }
        }
    };
    while i < chars.len() {
        let c = chars[i];
        let at = i;
        // What this item is, for the quantifier rules: an atom, an anchor (or a group opening,
        // or `|`), or a quantifier.
        let mut atom = true;
        match c {
            '\\' => {
                let Some(&e) = chars.get(i + 1) else {
                    return Err(format!("bad escape (end of pattern) at position {at}"));
                };
                i += 1;
                match (e, in_class) {
                    ('b', true) => out.push_str(r"\x08"),
                    ('Z', false) => {
                        out.push_str(r"\z");
                        atom = false;
                    }
                    // 3.12's `\B` does not match an empty subject.
                    ('B', false) => {
                        out.push_str(r"(?!\A\z)\B");
                        atom = false;
                    }
                    ('A' | 'b', false) => {
                        out.push('\\');
                        out.push(e);
                        atom = false;
                    }
                    ('d' | 'D', ..) | ('w' | 'W' | 's' | 'S', ..) => {
                        out.push('\\');
                        out.push(e);
                    }
                    ('a', ..) => out.push_str(r"\x07"),
                    ('f', ..) => out.push_str(r"\x0c"),
                    ('n', ..) => out.push_str(r"\n"),
                    ('r', ..) => out.push_str(r"\r"),
                    ('t', ..) => out.push_str(r"\t"),
                    ('v', ..) => out.push_str(r"\x0b"),
                    ('x', ..) => {
                        let Some(code) = hex(i + 1, 2) else {
                            return Err(format!("incomplete escape \\x at position {at}"));
                        };
                        // `\xNN` is one code point (str) or one byte (bytes).
                        out.push_str(&format!("\\x{{{code:x}}}"));
                        i += 2;
                    }
                    ('0', ..) => {
                        // `\0`, `\0o`, `\0oo`: an octal escape of up to three digits.
                        let mut code = 0;
                        let mut n = 0;
                        while n < 2 && chars.get(i + 1).is_some_and(|d| ('0'..='7').contains(d)) {
                            code = code * 8 + chars[i + 1].to_digit(8).unwrap_or(0);
                            i += 1;
                            n += 1;
                        }
                        out.push_str(&format!("\\x{{{code:x}}}"));
                    }
                    ('1'..='9', ..) => {
                        return Err(format!(
                            "backreferences and octal escapes (\\{e} at position {at}) are not \
                             supported"
                        ));
                    }
                    (e, ..) if e.is_ascii_alphabetic() => {
                        return Err(format!("bad escape \\{e} at position {at}"));
                    }
                    (e, ..) => literal(&mut out, u32::from(e)),
                }
            }
            _ if in_class => match c {
                ']' => {
                    in_class = false;
                    out.push(c);
                }
                '[' => out.push_str(r"\["),
                // Set operators in fancy-regex, literals (with a FutureWarning) in Python.
                '&' | '~' if chars.get(i + 1) == Some(&c) => {
                    out.push(c);
                    out.push('\\');
                    out.push(c);
                    i += 1;
                }
                // Whether `--` makes a range depends on what precedes it.
                '-' if chars.get(i + 1) == Some(&'-') => {
                    return Err(format!(
                        "`--` in a class (at position {at}) is not supported"
                    ));
                }
                c if !c.is_ascii() => literal(&mut out, u32::from(c)),
                c => out.push(c),
            },
            '[' => {
                in_class = true;
                out.push(c);
                // A `]` right after `[` or `[^` is a literal.
                if chars.get(i + 1) == Some(&'^') {
                    out.push('^');
                    i += 1;
                }
                if chars.get(i + 1) == Some(&']') {
                    out.push_str(r"\]");
                    i += 1;
                }
            }
            '*' | '+' | '?' | '{' => {
                // `{` is a repeat only as `{m}`, `{m,}`, `{,n}`, `{m,n}` or `{,}`.
                let mut repeat = String::new();
                let mut exact = false;
                if c == '{' {
                    let mut j = i + 1;
                    let digits = |j: &mut usize| {
                        let start = *j;
                        while chars.get(*j).is_some_and(char::is_ascii_digit) {
                            *j += 1;
                        }
                        chars[start..*j].iter().collect::<String>()
                    };
                    let lo = digits(&mut j);
                    let hi = if chars.get(j) == Some(&',') {
                        j += 1;
                        Some(digits(&mut j))
                    } else {
                        None
                    };
                    if chars.get(j) != Some(&'}') || (lo.is_empty() && hi.is_none()) {
                        // Not a repeat: a literal `{`.
                        out.push_str(r"\{");
                        can_repeat = true;
                        after_quant = false;
                        after_suffix = false;
                        i += 1;
                        continue;
                    }
                    let min = if lo.is_empty() {
                        "0".to_string()
                    } else {
                        lo.clone()
                    };
                    if let (Some(hi), Ok(a)) = (&hi, min.parse::<u64>())
                        && !hi.is_empty()
                        && hi.parse::<u64>().is_ok_and(|b| b < a)
                    {
                        return Err(format!(
                            "min repeat greater than max repeat at position {}",
                            at + 1
                        ));
                    }
                    exact = hi.is_none() || hi.as_deref() == Some(lo.as_str());
                    repeat = match &hi {
                        None => format!("{{{min}}}"),
                        Some(hi) => format!("{{{min},{hi}}}"),
                    };
                    i = j;
                }
                let token = if c == '{' { repeat } else { c.to_string() };
                if after_quant && matches!(c, '?' | '+') {
                    // A lazy or possessive suffix.
                    if groups.contains(&true) {
                        return Err(format!(
                            "a quantifier in a look-behind (at position {at}) is not supported"
                        ));
                    }
                    out.push_str(&token);
                    after_quant = false;
                    after_suffix = true;
                    i += 1;
                    continue;
                }
                if after_quant || after_suffix {
                    return Err(format!("multiple repeat at position {at}"));
                }
                if !can_repeat {
                    return Err(format!("nothing to repeat at position {at}"));
                }
                if groups.contains(&true) && !exact {
                    return Err(format!(
                        "a variable-width look-behind (at position {at}) is not supported"
                    ));
                }
                out.push_str(&token);
                can_repeat = false;
                after_quant = true;
                i += 1;
                continue;
            }
            '}' => out.push_str(r"\}"),
            '$' => {
                out.push_str(r"(?=\n?\z)");
                atom = false;
            }
            '^' => {
                out.push(c);
                atom = false;
            }
            '|' => {
                if groups.contains(&true) {
                    return Err(format!(
                        "alternation in a look-behind (at position {at}) is not supported"
                    ));
                }
                out.push(c);
                atom = false;
            }
            '(' => {
                let behind = chars.get(i + 1) == Some(&'?')
                    && chars.get(i + 2) == Some(&'<')
                    && matches!(chars.get(i + 3), Some('=' | '!'));
                // The group's prefix (`(`, `(?:`, `(?=`, `(?<!`, `(?P<name>`, …), copied whole.
                let end = if chars.get(i + 1) == Some(&'?') {
                    match chars.get(i + 2) {
                        Some(':' | '=' | '!' | '>') => Some(i + 2),
                        Some('<') if matches!(chars.get(i + 3), Some('=' | '!')) => Some(i + 3),
                        Some('P') if chars.get(i + 3) == Some(&'<') => chars[i + 4..]
                            .iter()
                            .position(|&ch| ch == '>')
                            .map(|p| i + 4 + p),
                        _ => None,
                    }
                } else {
                    Some(i)
                };
                let Some(end) = end else {
                    return Err(format!(
                        "this group syntax (at position {at}) is not supported"
                    ));
                };
                out.extend(&chars[i..=end]);
                i = end;
                groups.push(behind);
                atom = false;
            }
            ')' => {
                groups.pop();
                out.push(c);
            }
            c if !c.is_ascii() => literal(&mut out, u32::from(c)),
            c => out.push(c),
        }
        if !in_class {
            can_repeat = atom;
            after_quant = false;
            after_suffix = false;
        }
        i += 1;
    }
    Ok(out)
}

/// `re.compile(pattern.encode(), re.IGNORECASE)` of a bytes pattern: ASCII classes, ASCII case
/// folding, any byte matched by `.`.
pub fn compile_bytes_ignorecase(py: &str) -> Result<fancy_regex::Regex, String> {
    let pattern = translate_bytes(py)?;
    fancy_regex::RegexBuilder::new(&pattern)
        .bytes_mode(fancy_regex::BytesMode::Ascii)
        .case_insensitive(true)
        .build()
        .map_err(|e| e.to_string())
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

    /// (mode, pattern, subject, Python 3.12's `re.search` span or `none`/`error`), from
    /// `python:3.12-slim` (`python3 -I`; bytes patterns compiled with IGNORECASE).
    const RE_CASES: &[(&str, &str, &str, &str)] = &[
        ("str", "\\p{L}", "a", "error"),
        ("str", "a\\z", "a", "error"),
        ("str", "[a--b]", "-", "error"),
        ("str", "[!--]", "-", "0,1"),
        ("str", "[--z]", "q", "0,1"),
        ("str", "[a&&b]", "&", "0,1"),
        ("str", "[a~~b]", "~", "0,1"),
        ("str", "\\<", "<", "0,1"),
        ("str", "\\>", ">", "0,1"),
        ("str", "\\N{DIGIT ONE}", "1", "0,1"),
        ("str", "\\0", "a\u{0}", "1,2"),
        ("str", "\\012", "a\n", "1,2"),
        ("str", "\\1", "a", "error"),
        ("str", "(a)\\1", "aa", "0,2"),
        ("str", "(?m)a$", "a\nb", "0,1"),
        ("str", "(?i)A", "a", "0,1"),
        ("str", "(?#c)a", "a", "0,1"),
        ("str", "(?P<n>a)(?P=n)", "aa", "0,2"),
        ("str", "(?<n>a)", "a", "error"),
        ("str", "(?>a+)b", "aab", "0,3"),
        ("str", "(?:a|b)+", "xab", "1,3"),
        ("str", "(?<=x)a", "xa", "1,2"),
        ("str", "(?<!x)a", "xa ya", "4,5"),
        ("str", "a(?=b)", "ab", "0,1"),
        ("str", "a(?!b)", "ab ac", "3,4"),
        ("str", "\\x41", "A", "0,1"),
        ("str", "\\x4", "A", "error"),
        ("str", "\\x{41}", "A", "error"),
        ("str", "\\u00e9", "\u{e9}", "0,1"),
        ("str", "\\U0001F600", "\u{1f600}", "0,1"),
        ("str", "\\.", "a.b", "1,2"),
        ("str", "\\-", "a-b", "1,2"),
        ("str", "[\\]]", "]", "0,1"),
        ("str", "[]a]", "]", "0,1"),
        ("str", "[^]a]", "b", "0,1"),
        ("str", "[\\b]", "\u{8}", "0,1"),
        ("str", "\\bfoo\\b", "a foo b", "2,5"),
        ("str", "\\Bfoo", "afoo", "1,4"),
        ("str", "a$", "a\n", "0,1"),
        ("str", "a$", "a\nb", "none"),
        ("str", "a\\Z", "a\n", "none"),
        ("str", "\\Aa", "ba", "none"),
        ("str", "x{,2}y", "xxy", "0,3"),
        ("str", "x{,}y", "xxxy", "0,4"),
        ("str", "\\w+", "\u{e9}_1", "0,3"),
        ("str", "\\s+", "a\u{1c}\u{1f}b", "1,3"),
        ("str", "\\d", "\u{661}", "0,1"),
        ("str", "\\e", "e", "error"),
        ("str", "\\G", "a", "error"),
        ("str", "\\\u{e9}", "\u{e9}", "0,1"),
        ("str", "\\ ", "a b", "1,2"),
        ("str", "[[:alpha:]]", ":", "none"),
        ("str", "\\a\\f\\v\\t", "\u{7}\u{c}\u{b}\t", "0,4"),
        (
            "str",
            "\\brequire\\s*\\(|\\bfrom\\s*['\"](?:node:|(?:fs|path|os|http|https|net|child_process|url|zlib|crypto|util|stream|worker_threads)['\"/])|\\bimport\\s*\\(\\s*['\"]node:|\\bchild_process\\b|\\bprocess\\.(?:argv|exit|env|cwd)\\b",
            "x = require ('fs')",
            "4,13",
        ),
        (
            "str",
            "\\brequire\\s*\\(|\\bfrom\\s*['\"](?:node:|(?:fs|path|os|http|https|net|child_process|url|zlib|crypto|util|stream|worker_threads)['\"/])|\\bimport\\s*\\(\\s*['\"]node:|\\bchild_process\\b|\\bprocess\\.(?:argv|exit|env|cwd)\\b",
            "const required = 1; // from 'react'",
            "none",
        ),
        (
            "bytes",
            "remote-debugging|user-data-dir|CHROME_PATH|Google Chrome|chrome\\.exe|/usr/bin/(google-chrome|chromium)|playwright|puppeteer",
            "launch --Remote-Debugging-Port",
            "9,25",
        ),
        (
            "bytes",
            "remote-debugging|user-data-dir|CHROME_PATH|Google Chrome|chrome\\.exe|/usr/bin/(google-chrome|chromium)|playwright|puppeteer",
            "HeadlessChrome/ UA",
            "none",
        ),
        ("bytes", "\\w+", "\u{e9}a", "2,3"),
        ("bytes", "\\u0041", "A", "error"),
        ("bytes", "\\N{DIGIT ONE}", "1", "error"),
        ("bytes", "a$", "A\n", "0,1"),
        ("bytes", "\\x41", "a", "0,1"),
        ("bytes", "\\bab", "xab ab", "4,6"),
        ("str", "(?=a)*a", "a", "0,1"),
        ("str", "(?<=a)?b", "ab", "1,2"),
        ("str", "^*a", "a", "error"),
        ("str", "a**", "aa", "error"),
        ("str", "a*?b", "aab", "0,3"),
        ("str", "a*+b", "aab", "0,3"),
        ("str", "a{2}{3}", "aaaaaa", "error"),
        ("str", "a{,}b", "aab", "0,3"),
        ("str", "{1,2}", "{1,2}", "error"),
        ("str", "x{}", "x{}", "0,3"),
        ("str", "x{a}", "x{a}", "0,4"),
        ("str", "a}", "a}", "0,2"),
        ("str", "(?<=a?)b", "ab", "error"),
        ("str", "(?<=a|bc)d", "bcd", "error"),
        ("str", "(?<=ab|cd)e", "cde", "2,3"),
        ("str", "(?<=\\d*)x", "1x", "error"),
        ("str", "(?<=a{2})b", "aab", "2,3"),
        ("str", "|*", "a", "error"),
        ("str", "(*)", "a", "error"),
        ("str", "a|*b", "b", "error"),
        ("str", "\\Z?", "a", "error"),
        ("str", "\\b?a", "a", "error"),
        ("str", "a+{1,2}", "aa", "error"),
        ("str", "a?+b", "ab", "0,2"),
        ("str", "a{1,2}?b", "aab", "0,3"),
        ("str", "a{1,2}+b", "aab", "0,3"),
        ("str", "a{2,1}", "aa", "error"),
        ("str", "\\B", "", "none"),
        ("str", "\\B", "ab", "1,1"),
        ("str", "a{1", "a{1", "0,3"),
        ("str", "a{1,", "a{1,", "0,4"),
        ("str", "(a)*", "aa", "0,2"),
        ("str", "(?:)*a", "a", "0,1"),
        ("str", "a(?#x)*", "aa", "0,2"),
        ("bytes", "\\\u{e9}", "\u{e9}", "0,2"),
        ("bytes", "\u{e9}*", "\u{e9}\u{a9}", "0,2"),
        ("bytes", "[\u{e9}]", "\u{a9}", "1,2"),
        ("bytes", "\\B", "", "none"),
        ("bytes", "\u{e9}", "\u{c9}", "none"),
        ("bytes", "\\xe9", "\u{e9}", "none"),
        ("str", "\u{e9}*", "\u{e9}\u{e9}", "0,2"),
    ];

    /// Patterns Python accepts that the bytes translation or pycompat's `re` refuses instead of
    /// reinterpreting; a pattern that compiles must still give Python's answer.
    const REFUSED: &[&str] = &[
        "[!--]",
        "[--z]",
        "\\N{DIGIT ONE}",
        "(a)\\1",
        "(?m)a$",
        "(?i)A",
        "(?#c)a",
        "(?P<n>a)(?P=n)",
        "a(?#x)*",
        "(?<=ab|cd)e",
        // fancy-regex refuses a quantified look-around or empty group.
        "(?=a)*a",
        "(?<=a)?b",
        "(?:)*a",
    ];

    /// Patterns Python rejects (a look-behind without a fixed width) that pycompat's `re` runs
    /// anyway (its documented difference: no Python source can hold one).
    const ACCEPTED_WIDER: &[&str] = &["(?<=a?)b", "(?<=a|bc)d", "(?<=\\d*)x"];

    /// Python's span of the first match: code points for a str pattern, bytes for a bytes one.
    fn span(mode: &str, pattern: &str, subject: &str) -> Result<String, String> {
        let found = if mode == "str" {
            let re = anthology_pycompat::re::compile(pattern, false)?;
            re.find_at(subject, 0).map_err(|e| e.to_string())?
        } else {
            let re = compile_bytes_ignorecase(pattern)?;
            re.find(subject)
                .map_err(|e| e.to_string())?
                .map(|m| (m.start(), m.end()))
        };
        Ok(match found {
            None => "none".to_string(),
            Some((a, b)) if mode == "str" => format!(
                "{},{}",
                subject[..a].chars().count(),
                subject[..b].chars().count()
            ),
            Some((a, b)) => format!("{a},{b}"),
        })
    }

    #[test]
    fn regex_matches_python_or_refuses() {
        let mut failures = Vec::new();
        for (mode, pattern, subject, want) in RE_CASES {
            match span(mode, pattern, subject) {
                Err(e) => {
                    if *want != "error" && !REFUSED.contains(pattern) {
                        failures.push(format!("{pattern:?} refused ({e}) but Python gives {want}"));
                    }
                }
                Ok(_) if *want == "error" && ACCEPTED_WIDER.contains(pattern) => {}
                Ok(_) if *want == "error" => failures.push(format!(
                    "{mode} {pattern:?} compiles but Python gives {want}"
                )),
                Ok(got) if got != *want => failures.push(format!(
                    "{mode} {pattern:?} on {subject:?}: {got}, Python {want}"
                )),
                Ok(_) => {}
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

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
    fn truthiness() {
        assert!(!truthy(None));
        assert!(!truthy(Some(&Value::Str(String::new()))));
        assert!(truthy(Some(&Value::Str("x".into()))));
        assert!(!truthy(Some(&Value::Int(Int::Small(0)))));
        assert!(!truthy(Some(&Value::Float(0.0))));
    }
}
