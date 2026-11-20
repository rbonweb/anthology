//! Lenient lexing of unbalanced input (SH1): guard_rules.py `_lenient_segments`, the fallback when a quote
//! or substitution is unterminated. Whitespace split, quote characters dropped, and each embedded shell
//! script (`bash -c <rest of the line>`) also read as a command of its own.

use crate::lexer::pystr;
use crate::lexer::{Segment, Shared};
use crate::shell_args::script_starts;

/// `re.split(r"[;&|\n()`]+|\$\(", text)`: the pieces between separators (empty pieces included).
fn pieces(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let (mut start, mut i) = (0, 0);
    while i < bytes.len() {
        let sep = match bytes[i] {
            b';' | b'&' | b'|' | b'\n' | b'(' | b')' | b'`' => 1,
            b'$' if bytes.get(i + 1) == Some(&b'(') => 2,
            _ => 0,
        };
        if sep == 0 {
            i += 1;
            continue;
        }
        out.push(&text[start..i]);
        i += sep;
        start = i;
    }
    out.push(&text[start..]);
    out
}

/// `_lenient_segments(text)`: every segment gets a group of its own.
pub(crate) fn lenient_segments(text: &str, sh: &mut Shared<'_>) -> Vec<Segment> {
    let mut out = Vec::new();
    for piece in pieces(text) {
        let unquoted = piece.replace(['\'', '"'], " ");
        let tokens: Vec<String> = pystr::split_ws(&unquoted)
            .into_iter()
            .map(str::to_string)
            .collect();
        if tokens.is_empty() {
            continue;
        }
        let starts = script_starts(&tokens);
        out.push(Segment {
            tokens: tokens.clone(),
            redirs: Vec::new(),
            group: sh.new_group(),
        });
        for (start, first) in starts {
            let mut extra = Vec::with_capacity(tokens.len().saturating_sub(start));
            extra.push(first);
            extra.extend(tokens.iter().skip(start + 1).cloned());
            out.push(Segment {
                tokens: extra,
                redirs: Vec::new(),
                group: sh.new_group(),
            });
        }
    }
    out
}
