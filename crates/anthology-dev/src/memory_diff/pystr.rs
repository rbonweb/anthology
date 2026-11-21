//! `str.splitlines`, which P1's `pycompat::str` does not have (the rest of the Python `str`
//! predicates DV2's tools need come from `anthology_pycompat::str`).

/// The code points `str.splitlines` breaks on (CPython 3.12): `\n`, `\v`, `\f`, `\r`,
/// `\x1c`-`\x1e`, `\x85`, U+2028 and U+2029.
const LINE_BOUNDARIES: [char; 10] = [
    '\n', '\u{b}', '\u{c}', '\r', '\u{1c}', '\u{1d}', '\u{1e}', '\u{85}', '\u{2028}', '\u{2029}',
];

/// `str.splitlines(keepends)`.
pub fn splitlines(s: &str, keepends: bool) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        if !LINE_BOUNDARIES.contains(&c) {
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
    fn lines() {
        assert_eq!(
            splitlines("a\r\nb\rc\u{2028}d\n", false),
            ["a", "b", "c", "d"]
        );
        assert_eq!(splitlines("a\r\nb\n\nc", true), ["a\r\n", "b\n", "\n", "c"]);
        assert!(splitlines("", false).is_empty());
    }
}
