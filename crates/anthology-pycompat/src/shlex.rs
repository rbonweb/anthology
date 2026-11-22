//! `pycompat::shlex`: `shlex.split` and `shlex.quote` of CPython 3.12 (design §12; smoke).

/// `shlex.split(s)` (POSIX mode, no comments, whitespace-split). `Err` is the `ValueError`
/// text: `No closing quotation` or `No escaped character`.
pub fn split(s: &str) -> Result<Vec<String>, String> {
    #[derive(Clone, Copy, PartialEq)]
    enum State {
        Space,
        Word,
        Quote(char),
        Escape(char),
    }
    let is_ws = |c: char| matches!(c, ' ' | '\t' | '\r' | '\n');
    let mut out = Vec::new();
    let mut token = String::new();
    let mut quoted = false;
    let mut state = State::Space;
    let mut chars = s.chars();
    loop {
        let next = chars.next();
        match state {
            State::Space | State::Word => match next {
                None => break,
                Some(c) if is_ws(c) => {
                    if !token.is_empty() || quoted {
                        out.push(std::mem::take(&mut token));
                        quoted = false;
                    }
                    state = State::Space;
                }
                Some('\\') => state = State::Escape('a'),
                Some(c @ ('\'' | '"')) => state = State::Quote(c),
                Some(c) => {
                    token.push(c);
                    state = State::Word;
                }
            },
            State::Quote(q) => {
                quoted = true;
                match next {
                    None => return Err("No closing quotation".into()),
                    Some(c) if c == q => state = State::Word,
                    Some('\\') if q == '"' => state = State::Escape(q),
                    Some(c) => token.push(c),
                }
            }
            State::Escape(from) => {
                let Some(c) = next else {
                    return Err("No escaped character".into());
                };
                // Inside double quotes, a backslash only escapes `"` and itself.
                if from == '"' && c != '\\' && c != '"' {
                    token.push('\\');
                }
                token.push(c);
                state = if from == '"' {
                    State::Quote('"')
                } else {
                    State::Word
                };
            }
        }
    }
    if !token.is_empty() || quoted {
        out.push(token);
    }
    Ok(out)
}

/// `shlex.quote(s)`: `''` for empty, `s` itself when it holds only `[A-Za-z0-9_@%+=:,./-]`,
/// otherwise `s` in single quotes, each `'` written as `'"'"'`.
pub fn quote(s: &str) -> String {
    if s.is_empty() {
        return "''".into();
    }
    let safe = |c: char| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c);
    if s.chars().all(safe) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_and_quote() {
        assert_eq!(
            split("a 'b c' \"d\\\"e\" f\\ g").unwrap(),
            ["a", "b c", "d\"e", "f g"]
        );
        assert_eq!(split("''").unwrap(), [""]);
        assert_eq!(split("a\"\"b").unwrap(), ["ab"]);
        assert_eq!(split("\"a\\b\"").unwrap(), ["a\\b"]);
        assert_eq!(split("'unterminated").unwrap_err(), "No closing quotation");
        assert_eq!(split("x\\").unwrap_err(), "No escaped character");
        assert_eq!(quote(""), "''");
        assert_eq!(quote("a-b/c.d"), "a-b/c.d");
        assert_eq!(quote("it's"), "'it'\"'\"'s'");
        assert_eq!(quote("é"), "'é'");
    }
}
