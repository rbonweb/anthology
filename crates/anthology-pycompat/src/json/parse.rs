//! `json.loads` (F2): CPython 3.12's C scanner (`Modules/_json.c`), iterative.
//!
//! The scanner works on the UTF-8 bytes. Every token it looks at is ASCII, so byte indices fall
//! on character boundaries; an error converts its byte index to Python's code-point index. The
//! two places where the C code compares an index with the code-point length (`\u` escapes) count
//! code points explicitly.

use std::collections::HashMap;

use super::{INT_LIMIT_PREFIX, Int, JsonError, Map, UTF8_ERROR_PREFIX, Value};

/// Python's `sys.get_int_max_str_digits()` default.
const INT_MAX_STR_DIGITS: usize = 4300;
/// Objects with more entries than this find a repeated key through a hash index.
const INDEX_FROM: usize = 16;

/// `json.loads(text)`. Iterative; no depth limit; NaN and Infinity accepted; a lone surrogate
/// escape is U+FFFD (D-6).
pub fn loads(text: &str) -> Result<Value, JsonError> {
    if text.starts_with('\u{feff}') {
        return Err(JsonError {
            msg: "Unexpected UTF-8 BOM (decode using utf-8-sig)".into(),
            pos: 0,
        });
    }
    Parser {
        text,
        b: text.as_bytes(),
    }
    .document()
    .map_err(|e| e.into_json_error(text))
}

/// `json.loads(b.decode("utf-8", errors="replace"))`, as `guard.py` reads its stdin.
pub fn loads_bytes_lossy(b: &[u8]) -> Result<Value, JsonError> {
    loads(&String::from_utf8_lossy(b))
}

/// `json.loads(text[1:] if text.startswith("\ufeff") else text)` with
/// `text = b.decode("utf-8")`, as `settings_patch.read_text` and its callers read settings.json.
/// For valid input that is `b.decode("utf-8-sig")`; invalid UTF-8 is Python's
/// `UnicodeDecodeError` (see [`JsonError`]) with its position counted from the first byte,
/// BOM included, as the call site's strict decode reports it.
pub fn loads_utf8_sig(b: &[u8]) -> Result<Value, JsonError> {
    match std::str::from_utf8(b) {
        Ok(text) => loads(text.strip_prefix('\u{feff}').unwrap_or(text)),
        Err(e) => Err(utf8_error(b, &e)),
    }
}

/// CPython's `UnicodeDecodeError` for strict UTF-8, as `str(exc)`.
fn utf8_error(b: &[u8], e: &std::str::Utf8Error) -> JsonError {
    let start = e.valid_up_to();
    let (end, reason) = match e.error_len() {
        None => (b.len(), "unexpected end of data"),
        Some(1) if matches!(b[start], 0x80..=0xC1 | 0xF5..=0xFF) => {
            (start + 1, "invalid start byte")
        }
        Some(n) => (start + n, "invalid continuation byte"),
    };
    let msg = if end - start == 1 {
        format!(
            "{UTF8_ERROR_PREFIX} byte 0x{:02x} in position {start}: {reason}",
            b[start]
        )
    } else {
        format!(
            "{UTF8_ERROR_PREFIX} bytes in position {start}-{}: {reason}",
            end - 1
        )
    };
    JsonError { msg, pos: start }
}

/// An error at a byte index, before conversion to a code-point index.
struct Fail {
    msg: String,
    at: usize,
}

impl Fail {
    fn new(msg: &str, at: usize) -> Fail {
        Fail {
            msg: msg.to_string(),
            at,
        }
    }

    fn into_json_error(self, text: &str) -> JsonError {
        let pos = text.as_bytes()[..self.at]
            .iter()
            .filter(|&&b| b & 0xC0 != 0x80)
            .count();
        JsonError { msg: self.msg, pos }
    }
}

/// An object under construction: entries in first-seen order, with a key index once it grows.
#[derive(Default)]
struct ObjectBuilder {
    entries: Vec<(String, Value)>,
    index: Option<HashMap<String, usize>>,
}

impl ObjectBuilder {
    fn insert(&mut self, k: String, v: Value) {
        if self.index.is_none() && self.entries.len() >= INDEX_FROM {
            let index = self
                .entries
                .iter()
                .enumerate()
                .map(|(i, (key, _))| (key.clone(), i))
                .collect();
            self.index = Some(index);
        }
        match &mut self.index {
            None => match self.entries.iter_mut().find(|(key, _)| *key == k) {
                Some(slot) => slot.1 = v,
                None => self.entries.push((k, v)),
            },
            Some(index) => match index.get(&k) {
                Some(&i) => self.entries[i].1 = v,
                None => {
                    index.insert(k.clone(), self.entries.len());
                    self.entries.push((k, v));
                }
            },
        }
    }

    fn finish(self) -> Value {
        Value::Object(Map(self.entries))
    }
}

/// A container being parsed.
enum Frame {
    Array(Vec<Value>),
    /// The object so far and the key whose value is being parsed.
    Object(ObjectBuilder, String),
}

/// What scanning one value produced.
enum Scanned {
    Done(Value, usize),
    /// A non-empty container was opened; its first value starts at the index.
    Open(Frame, usize),
}

struct Parser<'a> {
    text: &'a str,
    b: &'a [u8],
}

fn is_ws(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r')
}

fn hex4(b: &[u8]) -> Option<u32> {
    let mut c = 0;
    for &d in b.get(..4)? {
        c = c << 4 | char::from(d).to_digit(16)?;
    }
    Some(c)
}

impl Parser<'_> {
    fn skip_ws(&self, mut i: usize) -> usize {
        while i < self.b.len() && is_ws(self.b[i]) {
            i += 1;
        }
        i
    }

    /// Whether at least `n` code points start at byte `i`.
    fn has_code_points(&self, i: usize, n: usize) -> bool {
        self.b
            .get(i..)
            .is_some_and(|rest| rest.iter().filter(|&&b| b & 0xC0 != 0x80).take(n).count() == n)
    }

    /// `JSONDecoder.decode`: whitespace, one value, whitespace, then the end.
    fn document(&self) -> Result<Value, Fail> {
        let (value, end) = self.value(self.skip_ws(0))?;
        let end = self.skip_ws(end);
        if end != self.b.len() {
            return Err(Fail::new("Extra data", end));
        }
        Ok(value)
    }

    /// `scan_once` at `i`, iteratively: an explicit stack holds the open containers.
    fn value(&self, mut i: usize) -> Result<(Value, usize), Fail> {
        let mut stack: Vec<Frame> = Vec::new();
        loop {
            let (mut value, mut end) = match self.scan(i)? {
                Scanned::Open(frame, next) => {
                    stack.push(frame);
                    i = next;
                    continue;
                }
                Scanned::Done(value, end) => (value, end),
            };
            // Hand the finished value to the enclosing containers, closing those that end here.
            loop {
                match stack.last_mut() {
                    None => return Ok((value, end)),
                    Some(Frame::Array(items)) => {
                        items.push(value);
                        let j = self.skip_ws(end);
                        match self.b.get(j) {
                            Some(b']') => {
                                let Some(Frame::Array(items)) = stack.pop() else {
                                    unreachable!("the top frame is an array")
                                };
                                value = Value::Array(items);
                                end = j + 1;
                            }
                            Some(b',') => {
                                i = self.skip_ws(j + 1);
                                break;
                            }
                            _ => return Err(Fail::new("Expecting ',' delimiter", j)),
                        }
                    }
                    Some(Frame::Object(builder, key)) => {
                        builder.insert(std::mem::take(key), value);
                        let j = self.skip_ws(end);
                        match self.b.get(j) {
                            Some(b'}') => {
                                let Some(Frame::Object(builder, _)) = stack.pop() else {
                                    unreachable!("the top frame is an object")
                                };
                                value = builder.finish();
                                end = j + 1;
                            }
                            Some(b',') => {
                                let (next_key, next) = self.key(self.skip_ws(j + 1))?;
                                *key = next_key;
                                i = next;
                                break;
                            }
                            _ => return Err(Fail::new("Expecting ',' delimiter", j)),
                        }
                    }
                }
            }
        }
    }

    /// An object key at `i`, its `:` and the whitespace after it: `(key, start of the value)`.
    fn key(&self, i: usize) -> Result<(String, usize), Fail> {
        if self.b.get(i) != Some(&b'"') {
            return Err(Fail::new(
                "Expecting property name enclosed in double quotes",
                i,
            ));
        }
        let (key, end) = self.string(i + 1)?;
        let j = self.skip_ws(end);
        if self.b.get(j) != Some(&b':') {
            return Err(Fail::new("Expecting ':' delimiter", j));
        }
        Ok((key, self.skip_ws(j + 1)))
    }

    fn scan(&self, i: usize) -> Result<Scanned, Fail> {
        let expecting = || Fail::new("Expecting value", i);
        let rest = self.b.get(i..).unwrap_or_default();
        let Some(&c) = rest.first() else {
            return Err(expecting());
        };
        let done = |v: Value, len: usize| Ok(Scanned::Done(v, i + len));
        match c {
            b'"' => {
                let (s, end) = self.string(i + 1)?;
                Ok(Scanned::Done(Value::Str(s), end))
            }
            b'{' => {
                let j = self.skip_ws(i + 1);
                if self.b.get(j) == Some(&b'}') {
                    return Ok(Scanned::Done(Value::Object(Map::new()), j + 1));
                }
                let (key, next) = self.key(j)?;
                Ok(Scanned::Open(
                    Frame::Object(ObjectBuilder::default(), key),
                    next,
                ))
            }
            b'[' => {
                let j = self.skip_ws(i + 1);
                if self.b.get(j) == Some(&b']') {
                    return Ok(Scanned::Done(Value::Array(Vec::new()), j + 1));
                }
                Ok(Scanned::Open(Frame::Array(Vec::new()), j))
            }
            b'n' if rest.starts_with(b"null") => done(Value::Null, 4),
            b't' if rest.starts_with(b"true") => done(Value::Bool(true), 4),
            b'f' if rest.starts_with(b"false") => done(Value::Bool(false), 5),
            b'N' if rest.starts_with(b"NaN") => done(Value::Float(f64::NAN), 3),
            b'I' if rest.starts_with(b"Infinity") => done(Value::Float(f64::INFINITY), 8),
            b'-' if rest.starts_with(b"-Infinity") => done(Value::Float(f64::NEG_INFINITY), 9),
            _ => match self.number(i)? {
                Some((v, end)) => Ok(Scanned::Done(v, end)),
                None => Err(expecting()),
            },
        }
    }

    /// `_match_number_unicode`: `None` where Python raises `StopIteration` ("Expecting value").
    fn number(&self, start: usize) -> Result<Option<(Value, usize)>, Fail> {
        let b = self.b;
        let digit = |j: usize| b.get(j).is_some_and(u8::is_ascii_digit);
        let mut i = start;
        if b[i] == b'-' {
            i += 1;
            if i >= b.len() {
                return Ok(None);
            }
        }
        match b[i] {
            b'1'..=b'9' => {
                i += 1;
                while digit(i) {
                    i += 1;
                }
            }
            b'0' => i += 1,
            _ => return Ok(None),
        }
        let mut is_float = false;
        if b.get(i) == Some(&b'.') && digit(i + 1) {
            is_float = true;
            i += 2;
            while digit(i) {
                i += 1;
            }
        }
        if i + 1 < b.len() && matches!(b[i], b'e' | b'E') {
            let e_start = i;
            i += 1;
            if i + 1 < b.len() && matches!(b[i], b'-' | b'+') {
                i += 1;
            }
            while digit(i) {
                i += 1;
            }
            if b[i - 1].is_ascii_digit() {
                is_float = true;
            } else {
                i = e_start;
            }
        }
        let numstr = &self.text[start..i];
        let value = if is_float {
            // Rust's parse is correctly rounded like `float()`; out of range is ±inf.
            Value::Float(numstr.parse().unwrap_or(f64::NAN))
        } else {
            let digits = numstr.len() - usize::from(numstr.starts_with('-'));
            if digits > INT_MAX_STR_DIGITS {
                return Err(Fail {
                    msg: format!(
                        "{INT_LIMIT_PREFIX} for integer string conversion: value has {digits} \
                         digits; use sys.set_int_max_str_digits() to increase the limit"
                    ),
                    at: start,
                });
            }
            match numstr.parse::<i64>() {
                Ok(n) => Value::Int(Int::Small(n)),
                Err(_) => Value::Int(Int::Big(numstr.to_string())),
            }
        };
        Ok(Some((value, i)))
    }

    /// `scanstring_unicode`: `i` is just past the opening quote. Returns the string and the index
    /// past the closing quote.
    fn string(&self, i: usize) -> Result<(String, usize), Fail> {
        let b = self.b;
        let begin = i - 1;
        let unterminated = || Fail::new("Unterminated string starting at", begin);
        let mut out = String::new();
        let mut end = i;
        loop {
            // The next quote, backslash or control character.
            let mut next = end;
            while next < b.len() && !matches!(b[next], b'"' | b'\\' | 0..=0x1f) {
                next += 1;
            }
            let Some(&c) = b.get(next) else {
                return Err(unterminated());
            };
            if c <= 0x1f {
                return Err(Fail::new("Invalid control character at", next));
            }
            out.push_str(&self.text[end..next]);
            if c == b'"' {
                return Ok((out, next + 1));
            }
            // A backslash at `next`.
            let Some(&esc) = b.get(next + 1) else {
                return Err(unterminated());
            };
            if esc != b'u' {
                out.push(match esc {
                    b'"' => '"',
                    b'\\' => '\\',
                    b'/' => '/',
                    b'b' => '\u{8}',
                    b'f' => '\u{c}',
                    b'n' => '\n',
                    b'r' => '\r',
                    b't' => '\t',
                    _ => return Err(Fail::new("Invalid \\escape", next)),
                });
                end = next + 2;
                continue;
            }
            // `\uXXXX`: the C scanner wants a code point after the four hex digits.
            let u = next + 1;
            let invalid = |at: usize| Fail::new("Invalid \\uXXXX escape", at);
            if !self.has_code_points(u + 1, 5) {
                return Err(invalid(u));
            }
            let mut c = hex4(&b[u + 1..]).ok_or_else(|| invalid(u))?;
            end = u + 5;
            // A high surrogate, then `\u` and a low surrogate, is one character.
            if (0xD800..0xDC00).contains(&c)
                && self.has_code_points(end, 7)
                && b[end] == b'\\'
                && b[end + 1] == b'u'
            {
                let c2 = hex4(&b[end + 2..]).ok_or_else(|| invalid(end + 1))?;
                if (0xDC00..0xE000).contains(&c2) {
                    c = 0x10000 + ((c - 0xD800) << 10) + (c2 - 0xDC00);
                    end += 6;
                }
            }
            out.push(char::from_u32(c).unwrap_or('\u{fffd}'));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(text: &str) -> (String, usize) {
        let e = loads(text).unwrap_err();
        (e.msg, e.pos)
    }

    #[test]
    fn positions_are_code_points() {
        assert_eq!(err("[\"é\", x]"), ("Expecting value".into(), 6));
        assert_eq!(
            err("\"😀\u{1}\""),
            ("Invalid control character at".into(), 2)
        );
    }

    #[test]
    fn surrogate_escapes() {
        assert_eq!(loads(r#""😀""#).unwrap(), Value::Str("😀".into()));
        assert_eq!(
            loads(r#""\udc00\ud800""#).unwrap(),
            Value::Str("\u{fffd}\u{fffd}".into())
        );
        assert_eq!(
            loads(r#""\ud800A""#).unwrap(),
            Value::Str("\u{fffd}A".into())
        );
    }

    #[test]
    fn utf8_errors_read_like_python() {
        let e = loads_utf8_sig(b"ab\xe2\x82").unwrap_err();
        assert_eq!(
            e.msg,
            "'utf-8' codec can't decode bytes in position 2-3: unexpected end of data"
        );
        assert_eq!((e.pos, e.type_name()), (2, "UnicodeDecodeError"));
        let e = loads_utf8_sig(b"\xef\xbb\xbf\xff").unwrap_err();
        assert_eq!(
            e.msg,
            "'utf-8' codec can't decode byte 0xff in position 3: invalid start byte"
        );
        assert_eq!(
            loads_utf8_sig(b"\xef\xbb\xbf[]").unwrap(),
            Value::Array(vec![])
        );
    }
}
