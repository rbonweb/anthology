//! The two Python helpers ST1 needs that `anthology_pycompat` does not publish:
//!
//! - [`splitlines`]: Python's `str.splitlines()`.
//! - [`dirname`]: `posixpath.dirname` (pycompat's `path::split` is private).
//!
//! Everything else ST1 used (`str.isspace`, `difflib.unified_diff`, `os.path.realpath`,
//! `repr(str)`) is `anthology_pycompat`'s, called where it is needed.

/// `s.splitlines()`: split at `\n`, `\r`, `\r\n`, `\v`, `\f`, `\x1c`–`\x1e`, U+0085, U+2028 and
/// U+2029; a final line break adds no empty line.
pub(crate) fn splitlines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let brk = matches!(
            c,
            '\n' | '\r'
                | '\u{b}'
                | '\u{c}'
                | '\u{1c}'
                | '\u{1d}'
                | '\u{1e}'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        );
        if !brk {
            continue;
        }
        out.push(&s[start..i]);
        let mut end = i + c.len_utf8();
        if c == '\r' && matches!(it.peek(), Some((_, '\n'))) {
            it.next();
            end += 1;
        }
        start = end;
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// `posixpath.split(p)`.
fn split(p: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let i = p.iter().rposition(|&c| c == b'/').map_or(0, |i| i + 1);
    let (head, tail) = (&p[..i], &p[i..]);
    let mut head = head.to_vec();
    if !head.is_empty() && head.iter().any(|&c| c != b'/') {
        while head.last() == Some(&b'/') {
            head.pop();
        }
    }
    (head, tail.to_vec())
}

/// `posixpath.dirname(p)`.
pub(crate) fn dirname(p: &str) -> String {
    let (head, _) = split(p.as_bytes());
    String::from_utf8_lossy(&head).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitlines_matches_python() {
        assert_eq!(splitlines(""), Vec::<&str>::new());
        assert_eq!(splitlines("a\nb\n"), vec!["a", "b"]);
        assert_eq!(
            splitlines("a\r\nb\rc\u{2028}d\u{85}e\u{1e}f\u{b}"),
            vec!["a", "b", "c", "d", "e", "f"]
        );
        assert_eq!(splitlines("\n\n"), vec!["", ""]);
        assert_eq!(splitlines("x\u{1f}y"), vec!["x\u{1f}y"]);
    }

    #[test]
    fn dirname_matches_posixpath() {
        assert_eq!(dirname("/a/b/"), "/a/b");
        assert_eq!(dirname("/a"), "/");
        assert_eq!(dirname("/"), "/");
        assert_eq!(dirname("//a"), "//");
        assert_eq!(dirname("a"), "");
    }
}
