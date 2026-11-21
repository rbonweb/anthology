//! Local stand-ins for the `pycompat` functions the smoke needs while P1 has not landed.
//!
//! `pycompat::shlex::split` and `pycompat::str::{split_ws, strip_ws}` are empty stubs today. Every
//! use of them in the smoke goes through this one module, with the plan §3.2 signatures, so the
//! lead swaps the bodies for the real functions with a one-line change each.
//!
//! SWAP: use `pycompat::shlex::split` / `pycompat::str::split_ws` / `pycompat::str::strip_ws`
//! once P1 merges, and delete this file's bodies (keep `splitlines`, which pycompat has no
//! counterpart for).

/// Python's `str.isspace()` for one code point: the Unicode White_Space set plus U+001C..U+001F.
fn is_py_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// `s.split()`: the runs between whitespace, none of them empty.
pub fn split_ws(s: &str) -> Vec<&str> {
    s.split(is_py_space)
        .filter(|part| !part.is_empty())
        .collect()
}

/// `s.strip()`.
pub fn strip_ws(s: &str) -> &str {
    s.trim_matches(is_py_space)
}

/// `s.splitlines()`: lines without their ends, the boundaries being the ones `str` knows
/// (`\n`, `\r`, `\r\n`, `\v`, `\f`, U+001C..U+001E, U+0085, U+2028, U+2029). No pycompat
/// counterpart; kept here for the same reason as the rest.
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

/// `shlex.split(s)` (POSIX mode, `comments=False`, whitespace splitting): quotes, backslash
/// escapes (inside double quotes only before `"` and `\`), adjacent pieces joined into one word.
/// `Err` carries Python's `ValueError` text.
pub fn shlex_split(s: &str) -> Result<Vec<String>, String> {
    #[derive(Clone, Copy, PartialEq)]
    enum State {
        Between,
        Word,
        Quote(char),
        Escape(Option<char>),
    }
    let mut out = Vec::new();
    let mut token = String::new();
    let mut quoted = false;
    let mut state = State::Between;
    for c in s.chars() {
        state = match state {
            State::Between | State::Word => {
                if matches!(c, ' ' | '\t' | '\r' | '\n') {
                    if state == State::Word && (!token.is_empty() || quoted) {
                        out.push(std::mem::take(&mut token));
                    }
                    quoted = false;
                    State::Between
                } else if c == '\\' {
                    State::Escape(None)
                } else if c == '\'' || c == '"' {
                    quoted = true;
                    State::Quote(c)
                } else {
                    token.push(c);
                    State::Word
                }
            }
            State::Quote(q) => {
                if c == q {
                    State::Word
                } else if c == '\\' && q == '"' {
                    State::Escape(Some(q))
                } else {
                    token.push(c);
                    State::Quote(q)
                }
            }
            State::Escape(back) => match back {
                Some(q) => {
                    // Inside double quotes, a backslash only escapes the quote and itself.
                    if c != q && c != '\\' {
                        token.push('\\');
                    }
                    token.push(c);
                    State::Quote(q)
                }
                None => {
                    token.push(c);
                    State::Word
                }
            },
        };
    }
    match state {
        State::Quote(_) => return Err("No closing quotation".to_string()),
        State::Escape(_) => return Err("No escaped character".to_string()),
        State::Word if !token.is_empty() || quoted => out.push(token),
        State::Between | State::Word => {}
    }
    Ok(out)
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
