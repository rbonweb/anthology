//! The `pycompat` functions the smoke uses, in one place.
//!
//! `shlex_split`, `split_ws` and `strip_ws` are `pycompat::shlex::split` and
//! `pycompat::str::{split_ws, strip_ws}` (P1; EV1b swapped the EV1a stand-ins). `splitlines`
//! stays here: pycompat has no counterpart.

pub use anthology_pycompat::shlex::split as shlex_split;
pub use anthology_pycompat::str::{split_ws, strip_ws};

/// `s.splitlines()`: lines without their ends, the boundaries being the ones `str` knows
/// (`\n`, `\r`, `\r\n`, `\v`, `\f`, U+001C..U+001E, U+0085, U+2028, U+2029). No pycompat
/// counterpart.
pub fn splitlines(s: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let boundary = matches!(
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
        if !boundary {
            continue;
        }
        lines.push(&s[start..i]);
        start = i + c.len_utf8();
        if c == '\r' && matches!(chars.peek(), Some(&(_, '\n'))) {
            chars.next();
            start += 1;
        }
    }
    if start < s.len() {
        lines.push(&s[start..]);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_ws_uses_the_python_whitespace_set() {
        assert_eq!(split_ws(" a \u{1f}b\u{a0}\tc\n"), ["a", "b", "c"]);
        assert!(split_ws("").is_empty());
        assert_eq!(strip_ws("\u{1c} x \u{2003}"), "x");
    }

    #[test]
    fn splitlines_follows_python() {
        assert_eq!(splitlines("a\r\nb\rc\u{2028}d\n"), ["a", "b", "c", "d"]);
        assert_eq!(splitlines("a\n\nb"), ["a", "", "b"]);
        assert!(splitlines("").is_empty());
    }

    #[test]
    fn shlex_split_matches_python() {
        let split = |s| shlex_split(s).expect("splits");
        assert_eq!(split("a  b"), ["a", "b"]);
        assert_eq!(split("a 'b c' \"d e\" f\\ g"), ["a", "b c", "d e", "f g"]);
        assert_eq!(split("x\"a b\"y''"), ["xa by"]);
        assert_eq!(split("''"), [""]);
        assert_eq!(split("\"a\\\"b\\\\c\\d\""), ["a\"b\\c\\d"]);
        assert_eq!(split("'a\\b'"), ["a\\b"]);
        assert_eq!(split("a #b"), ["a", "#b"]);
        assert_eq!(
            split("${CLAUDE_PLUGIN_ROOT}/s.sh --quick"),
            ["${CLAUDE_PLUGIN_ROOT}/s.sh", "--quick"]
        );
        assert_eq!(shlex_split("'a"), Err("No closing quotation".to_string()));
        assert_eq!(shlex_split("a\\"), Err("No escaped character".to_string()));
    }
}
