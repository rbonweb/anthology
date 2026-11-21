//! Lenient lexing of unbalanced input (SH1, fix-wave H): guard_rules.py `_lenient_segments`, the fallback
//! when a quote or substitution is unterminated. Whitespace split with the quote characters read as spaces
//! and, when that differs, also with them dropped (`x=$'..' cmd`, `n'p'x` stay one word); each embedded
//! shell script (`bash -c <rest of the line>`) is also read as a command of its own.
//!
//! The output can grow with the cube of the word count (each `fish` word rescans the rest, and each start
//! copies the rest of the words), so every copied word is charged to the work budget (fail closed, D-7).

use crate::lexer::pystr;
use crate::lexer::{Segment, Shared, TooComplex};
use crate::shell_args::{WORD, script_starts_in};

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

/// `_LENIENT_QUOTES.sub("", piece)`: `'`, `"`, `$'` and `$"` dropped.
fn drop_quotes(piece: &str) -> String {
    let mut out = String::with_capacity(piece.len());
    let mut chars = piece.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '$' if matches!(chars.peek(), Some('\'' | '"')) => {
                chars.next();
            }
            '\'' | '"' => {}
            _ => out.push(c),
        }
    }
    out
}

fn words(text: &str) -> Vec<String> {
    pystr::split_ws(text)
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// `_lenient_segments(text)`: every segment gets a group of its own.
pub(crate) fn lenient_segments(text: &str, sh: &mut Shared) -> Result<Vec<Segment>, TooComplex> {
    let mut out = Vec::new();
    sh.work.charge(2 * text.len())?;
    for piece in pieces(text) {
        let spaced = words(&piece.replace(['\'', '"'], " "));
        let joined = words(&drop_quotes(piece));
        let readings = if joined != spaced {
            vec![spaced, joined]
        } else {
            vec![spaced]
        };
        for tokens in readings {
            if tokens.is_empty() {
                continue;
            }
            // Every copied word costs WORD + its length. `cost[k]` is the cost of tokens[k..].
            let mut cost = vec![0usize; tokens.len() + 1];
            for k in (0..tokens.len()).rev() {
                cost[k] = cost[k + 1].saturating_add(WORD + tokens[k].len());
            }
            sh.work.charge(cost[0])?;
            out.push(Segment {
                tokens: tokens.clone(),
                redirs: Vec::new(),
                group: sh.new_group(),
            });
            let starts = script_starts_in(&tokens, &mut sh.work.budget).ok_or(TooComplex)?;
            for (start, first) in starts {
                let rest = tokens.get(start + 1..).unwrap_or(&[]);
                let rest_cost = cost.get(start + 1).copied().unwrap_or(0);
                sh.work.charge(WORD + first.len() + rest_cost)?;
                let mut extra = Vec::with_capacity(1 + rest.len());
                extra.push(first);
                extra.extend(rest.iter().cloned());
                out.push(Segment {
                    tokens: extra,
                    redirs: Vec::new(),
                    group: sh.new_group(),
                });
            }
        }
    }
    Ok(out)
}
