//! Python `str` predicates DV2's tools need (CPython 3.12, Unicode 15.0), over the generated
//! [`super::unicode`] table. DV2 ruling: a stand-in until P1's `pycompat::str` lands.

use super::unicode::{Class, LINE_BOUNDARIES, RANGES, SPACE};

/// The kept general-category class of `c` (`None`: another class).
pub fn class(c: char) -> Option<Class> {
    let c = u32::from(c);
    let i = RANGES.partition_point(|&(_, last, _)| last < c);
    match RANGES.get(i) {
        Some(&(first, _, k)) if first <= c => Some(k),
        _ => None,
    }
}

/// `str.isspace()` for one code point.
pub fn is_space(c: char) -> bool {
    SPACE.binary_search(&u32::from(c)).is_ok()
}

/// `str.isprintable()` for one code point: not `C*`/`Z*`, except the space itself.
pub fn is_printable(c: char) -> bool {
    c == ' ' || class(c).is_none_or(|k| k == Class::Nd)
}

/// `re`'s `\d` (and `str.isdecimal()`): category `Nd`.
pub fn is_decimal(c: char) -> bool {
    class(c) == Some(Class::Nd)
}

/// `unicodedata.decimal(c)`: the digit value of an `Nd` code point. Unicode allocates every
/// `Nd` run as whole blocks of ten starting at a zero, so the value is the offset modulo 10.
pub fn decimal_value(c: char) -> Option<u8> {
    let n = u32::from(c);
    let i = RANGES.partition_point(|&(_, last, _)| last < n);
    match RANGES.get(i) {
        Some(&(first, _, Class::Nd)) if first <= n => Some(((n - first) % 10) as u8),
        _ => None,
    }
}

/// `str.strip()` with no argument.
pub fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// `str.split()` with no argument.
pub fn split_ws(s: &str) -> Vec<&str> {
    s.split(is_space).filter(|p| !p.is_empty()).collect()
}

/// `str.splitlines(keepends)`.
pub fn splitlines(s: &str, keepends: bool) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        if LINE_BOUNDARIES.binary_search(&u32::from(c)).is_err() {
            continue;
        }
        let mut end = i + c.len_utf8();
        if c == '\r' && matches!(it.peek(), Some(&(_, '\n'))) {
            it.next();
            end += 1;
        }
        out.push(if keepends {
            &s[start..end]
        } else {
            &s[start..i]
        });
        start = end;
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories() {
        assert_eq!(class('\0'), Some(Class::C));
        assert_eq!(class('\u{200b}'), Some(Class::C)); // Cf
        assert_eq!(class('\u{e000}'), Some(Class::C)); // Co
        assert_eq!(class('\u{378}'), Some(Class::C)); // Cn
        assert_eq!(class('\u{2028}'), Some(Class::Zl));
        assert_eq!(class('\u{2029}'), Some(Class::Zp));
        assert_eq!(class('\u{a0}'), Some(Class::Zs));
        assert_eq!(class('\u{663}'), Some(Class::Nd));
        assert_eq!(class('a'), None);
        assert_eq!(class('\u{10ffff}'), Some(Class::C));
        assert!(is_printable(' ') && !is_printable('\u{a0}') && is_printable('é'));
        assert_eq!(decimal_value('7'), Some(7));
        assert_eq!(decimal_value('\u{669}'), Some(9));
        assert_eq!(decimal_value('\u{1d7ff}'), Some(9));
        assert_eq!(decimal_value('a'), None);
    }

    #[test]
    fn whitespace_and_lines() {
        assert_eq!(strip("\u{1c} a b\u{3000}"), "a b");
        assert_eq!(split_ws(" a\u{85}b  c "), ["a", "b", "c"]);
        assert_eq!(
            splitlines("a\r\nb\rc\u{2028}d\n", false),
            ["a", "b", "c", "d"]
        );
        assert_eq!(splitlines("a\r\nb\n\nc", true), ["a\r\n", "b\n", "\n", "c"]);
        assert!(splitlines("", false).is_empty());
    }
}
