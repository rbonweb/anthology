//! The pieces of Python's `str` that scripts/rtk_trim.py relies on: `strip` and `repr` come from
//! `anthology_pycompat::str`; `lstrip`, `rstrip` and `splitlines` are local (pycompat has none).
//!
//! Whitespace is `anthology_pycompat::str::is_space` (the `\s` of a `str` pattern); `\w` is
//! `anthology_pycompat::str::is_word`.

use anthology_pycompat::str::is_space;
pub use anthology_pycompat::str::{repr, strip_ws as strip};

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
