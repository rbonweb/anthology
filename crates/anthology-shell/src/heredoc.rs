//! Here-document bodies (SH1): guard_rules.py `_find_heredoc_end`, `_body_substitutions` and
//! `_HEREDOC_DELIMITER`. `_HEREDOC_START` (fancy-regex, GC-16) is matched by `lexer::Shared::heredoc_start`.

use crate::lexer::pystr;
use crate::lexer::{Shared, find_from, scan_paren, skip_escape};

/// `_find_heredoc_end`: for a body starting at `text[i]`, (start of the terminator line, index just past
/// it), or `None` when the terminator never appears. A line is the terminator when it `strip()`s to it.
pub(crate) fn find_heredoc_end(
    text: &str,
    mut i: usize,
    delimiter: &str,
) -> Option<(usize, usize)> {
    let n = text.len();
    while i <= n {
        let end = find_from(text, i, b'\n');
        let stop = end.unwrap_or(n);
        if pystr::strip_ws(text.get(i..stop)?) == delimiter {
            return Some((i, (stop + 1).min(n)));
        }
        i = end? + 1;
    }
    None
}

/// `_HEREDOC_DELIMITER.match(word)`: `[A-Za-z_][A-Za-z0-9_.-]*\Z`.
pub(crate) fn is_heredoc_delimiter(word: &str) -> bool {
    let mut bytes = word.bytes();
    matches!(bytes.next(), Some(b) if b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

/// `_body_substitutions`: the `$(...)` and backtick spans inside an unquoted heredoc body (bash runs them).
pub(crate) fn body_substitutions<'t>(body: &'t str, sh: &mut Shared<'_>) -> Vec<&'t str> {
    let bytes = body.as_bytes();
    let n = body.len();
    let mut found = Vec::new();
    let mut j = 0;
    while j < n {
        match bytes[j] {
            b'\\' => j = skip_escape(body, j),
            b'$' if bytes.get(j + 1) == Some(&b'(') => {
                let (inner, after) = scan_paren(body, j + 2, sh);
                found.push(inner);
                j = after;
            }
            b'`' => {
                let Some(k) = find_from(body, j + 1, b'`') else {
                    break;
                };
                found.push(&body[j + 1..k]);
                j = k + 1;
            }
            _ => j += 1,
        }
    }
    found
}
