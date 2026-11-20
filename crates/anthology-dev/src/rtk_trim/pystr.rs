//! The pieces of Python's `str` that scripts/rtk_trim.py relies on, written once: `strip`,
//! `lstrip`, `rstrip`, `splitlines`, `repr` and the `\s` / `\w` classes of `re`.
//!
//! Python's whitespace (`str.isspace`, and `\s` in a `str` pattern) is the Unicode White_Space
//! set plus U+001C..U+001F, which Rust's `char::is_whitespace` lacks.
//!
//! TODO P1: replace these helpers with `pycompat::str` / `pycompat::re` once they exist.
//!
//! Known gap (ruling, P1 owns the Unicode tables): Python's `\w` is `str.isalnum()` or `_`, i.e.
//! categories L* and N*. `char::is_alphanumeric` also counts the `Other_Alphabetic` marks (some
//! combining characters), so those differ. Every identifier the trim looks for is ASCII.

/// Python's `str.isspace` for one character (and the `\s` class of `re` for `str` patterns).
pub fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// Python's `\w` for one character: `str.isalnum()` or `_` (see the module note for the gap).
pub fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `str.strip()` without arguments.
pub fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// `str.lstrip()` without arguments.
pub fn lstrip(s: &str) -> &str {
    s.trim_start_matches(is_space)
}

/// `str.rstrip()` without arguments.
pub fn rstrip(s: &str) -> &str {
    s.trim_end_matches(is_space)
}

/// Whether `c` ends a line for `str.splitlines()`.
fn is_line_boundary(c: char) -> bool {
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

/// `str.splitlines()` (no `keepends`): every Python line boundary, `\r\n` as one, and no empty
/// last line after a final boundary.
pub fn splitlines(s: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if !is_line_boundary(c) {
            continue;
        }
        lines.push(&s[start..i]);
        start = i + c.len_utf8();
        if c == '\r'
            && let Some(&(_, '\n')) = chars.peek()
        {
            chars.next();
            start += 1;
        }
    }
    if start < s.len() {
        lines.push(&s[start..]);
    }
    lines
}

/// `text.splitlines()[0] if text.strip() else text`.
pub fn first_line(text: &str) -> &str {
    if strip(text).is_empty() {
        return text;
    }
    // Not blank, so at least one line exists.
    splitlines(text).first().copied().unwrap_or(text)
}

/// Python's `str.isprintable` for one character, as far as std tells: controls and every
/// whitespace but the plain space are not printable. (Format, unassigned and private-use
/// characters are not told apart: documented best effort, the anchors the trim reports are ASCII.)
fn is_printable(c: char) -> bool {
    c == ' ' || !(c.is_control() || c.is_whitespace())
}

/// `repr(str)`.
pub fn repr(s: &str) -> String {
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
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if is_printable(c) => out.push(c),
            c => {
                let n = u32::from(c);
                let escaped = if n < 0x100 {
                    format!("\\x{n:02x}")
                } else if n < 0x1_0000 {
                    format!("\\u{n:04x}")
                } else {
                    format!("\\U{n:08x}")
                };
                out.push_str(&escaped);
            }
        }
    }
    out.push(quote);
    out
}
