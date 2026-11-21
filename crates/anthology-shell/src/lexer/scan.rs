//! The substitution scanner (fix-wave H): guard_rules.py `_scan_paren`, `_scan_paren_ex`, `_substitution`,
//! `_body_substitutions` and `_bracket_pairs`.

use std::borrow::Cow;
use std::collections::HashMap;

use super::words::{
    HeredocWord, Memo, ansi_c_end, backtick_end, brace_end, heredoc_after, input_operator,
    unescape_backticks,
};
use super::{TooComplex, Work, byte_at, find_from, skip_escape};
use crate::heredoc::find_heredoc_end;

/// Distinct heredocs searched for in vain, per scan or lexer, before giving up (`_MAX_UNCLOSED`).
pub(crate) const MAX_UNCLOSED: usize = 8;

/// The bytes `_scan_paren_walk` stops at (`_SCAN_SPECIAL`).
fn scan_special(b: u8) -> bool {
    matches!(
        b,
        b'\\' | b'<' | b'\n' | b'\'' | b'"' | b'$' | b'#' | b'(' | b')' | b'`' | b'c'
    )
}

/// `_scan_paren_ex`'s result: `inner` and `end` are Python's (inner, index after `)`); `None` for both is
/// Python's `(None, -1, False)` (an arithmetic scan that did not close with `))`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Scanned<'t> {
    pub(crate) inner: Option<&'t str>,
    pub(crate) end: Option<usize>,
    pub(crate) closes: bool,
}

/// `_scan_paren`: (inner, index after `)`) for the parenthesis opened just before `i`.
pub(crate) fn scan_paren<'t>(
    text: &'t str,
    i: usize,
    memo: &mut Memo,
    work: &mut Work,
) -> Result<(&'t str, usize), TooComplex> {
    let s = scan_paren_ex(text, i, false, memo, work)?;
    // a non-arithmetic scan always has an inner text and an end
    Ok((s.inner.unwrap_or(""), s.end.unwrap_or(text.len())))
}

/// `bytes[j..]` reads `case` and a blank once line continuations (`\\` newline) are removed (#89 review F-2:
/// `ca\\<newline>se` is `case` to bash).
fn case_word(bytes: &[u8], j: usize) -> bool {
    let mut k = j;
    let mut read = 0;
    while k < bytes.len() {
        if bytes[k] == b'\\' && bytes.get(k + 1) == Some(&b'\n') {
            k += 2;
            continue;
        }
        if read == 4 {
            return matches!(bytes[k], b' ' | b'\t' | b'\n');
        }
        if bytes[k] != b"case"[read] {
            return false;
        }
        read += 1;
        k += 1;
    }
    false
}

/// `_scan_paren_ex`: scan the parenthesis opened just before `i` as bash's parser matches it. With `arith`
/// (`text[i]` is the second `(` of `$((` or `((`) it closes only with `))`.
pub(crate) fn scan_paren_ex<'t>(
    text: &'t str,
    i: usize,
    arith: bool,
    memo: &mut Memo,
    work: &mut Work,
) -> Result<Scanned<'t>, TooComplex> {
    let (scanned, visited) = walk(text, i, arith, memo, work)?;
    work.charge(visited.saturating_sub(i))?;
    Ok(scanned)
}

/// A delimiter key of the scan: (readings, `<<-`, unquoted).
type Key = (Vec<String>, bool, bool);

fn walk<'t>(
    text: &'t str,
    i: usize,
    arith: bool,
    memo: &mut Memo,
    work: &mut Work,
) -> Result<(Scanned<'t>, usize), TooComplex> {
    let bytes = text.as_bytes();
    let n = text.len();
    let rest = |from: usize| text.get(from..).unwrap_or("");
    let mut depth = 1usize;
    let mut j = i;
    let mut arith_at = arith; // inside `$((`: `<<` is a shift
    let mut no_arith = false; // a `((` ran to the end: stop trying
    let mut delimiters: Vec<Key> = Vec::new();
    let mut unterminated: Vec<Key> = Vec::new();
    while j < n {
        let mut c = bytes[j];
        if !scan_special(c) {
            // jump to the next byte that matters
            match bytes[j..].iter().position(|&b| scan_special(b)) {
                Some(k) => {
                    j += k;
                    c = bytes[j];
                }
                None => break,
            }
        }
        if c == b'\\' {
            j = skip_escape(text, j);
            continue;
        }
        if c == b'<' {
            let (op, k) = input_operator(text, j);
            if !matches!(op, "<<" | "<<-") || arith_at || unterminated.len() >= MAX_UNCLOSED {
                j = k; // `<<<`, a shift, or too many bodies that never closed
                continue;
            }
            match heredoc_after(text, k, work)? {
                HeredocWord::Unclosed => {
                    // bash reads the rest into that word
                    let s = Scanned {
                        inner: Some(rest(i)),
                        end: Some(n),
                        closes: false,
                    };
                    return Ok((s, n));
                }
                HeredocWord::None => j = k,
                HeredocWord::Word {
                    delimiters: delims,
                    quoted,
                    end,
                    ..
                } => {
                    let key = (delims, op == "<<-", !quoted);
                    if !unterminated.contains(&key) {
                        delimiters.push(key);
                    }
                    j = end;
                }
            }
            continue;
        }
        if c == b'\n' && !delimiters.is_empty() {
            let mut pos = j + 1;
            for key in std::mem::take(&mut delimiters) {
                let found = find_heredoc_end(text, pos, &key.0, key.1, key.2, true, work)?;
                let Some((_, after)) = found else {
                    if !unterminated.contains(&key) {
                        unterminated.push(key);
                    }
                    continue;
                };
                pos = after;
                if byte_at(text, after - 1) != Some(b'\n') && after < n {
                    break; // `EOF)`: the line goes on as code
                }
            }
            j = pos; // Python: `j = pos - 1`, then the loop's `j += 1`
            continue;
        }
        match c {
            b'\'' => {
                j = find_from(text, j + 1, b'\'').map_or(n, |k| k + 1);
                continue;
            }
            b'"' => {
                j += 1;
                loop {
                    // `\"` does not close the quote
                    match bytes
                        .get(j..)
                        .and_then(|b| b.iter().position(|&x| x == b'"' || x == b'\\'))
                    {
                        None => {
                            j = n;
                            break;
                        }
                        Some(k) => {
                            let at = j + k;
                            if bytes[at] == b'"' {
                                j = at + 1;
                                break;
                            }
                            j = skip_escape(text, at);
                        }
                    }
                }
                continue;
            }
            b'$' if byte_at(text, j + 1) == Some(b'{') => {
                // `${x#<<E}` holds no heredoc
                j = match brace_end(text, j + 2, Some(memo), work)? {
                    None => j + 1,
                    Some(k) => k + 1,
                };
                continue;
            }
            b'$' if byte_at(text, j + 1) == Some(b'\'') => {
                // `$'a\'b )'` is one string
                j = ansi_c_end(text, j + 2, work)?.map_or(n, |k| k + 1);
                continue;
            }
            b'`' => {
                // `$(echo `echo )`; x)`: a `)` in a backtick span does not close
                j = backtick_end(text, j + 1, work)?.map_or(n, |k| k + 1);
                continue;
            }
            b'c' if !arith_at
                && case_word(bytes, j)
                && (j == i
                    || matches!(
                        bytes[j - 1],
                        b' ' | b'\t'
                            | b'\n'
                            | b';'
                            | b'&'
                            | b'|'
                            | b'('
                            | b')'
                            | b'{'
                            | b'}'
                            | b'!'
                    )) =>
            {
                // `$(case a in a) x;; esac)`: bash matches a pattern's `)` with its `case`, which this
                // scan does not model; never guess where the substitution ends
                return Err(TooComplex);
            }
            b'#' if !arith_at
                && (j == i
                    || matches!(
                        bytes[j - 1],
                        b' ' | b'\t' | b'\n' | b';' | b'&' | b'|' | b'(' | b')' | b'<' | b'>'
                    )) =>
            {
                // a comment: no `<<`, quote or paren
                j = find_from(text, j, b'\n').unwrap_or(n);
                continue;
            }
            b'(' => {
                if !arith_at && !no_arith && byte_at(text, j + 1) == Some(b'(') {
                    let s = scan_paren_ex(text, j + 1, true, memo, work)?;
                    if s.closes {
                        // `((...))`: skipped whole
                        j = s.end.unwrap_or(n);
                        continue;
                    }
                    no_arith = s.end.is_some_and(|e| e >= n);
                }
                depth += 1;
            }
            b')' => {
                depth -= 1;
                if depth == 0 {
                    let s = Scanned {
                        inner: Some(&text[i..j]),
                        end: Some(j + 1),
                        closes: arith,
                    };
                    return Ok((s, j + 1));
                }
                if arith && depth == 1 {
                    // `$((...)`: arithmetic only on `))`
                    if byte_at(text, j + 1) != Some(b')') {
                        let s = Scanned {
                            inner: None,
                            end: None,
                            closes: false,
                        };
                        return Ok((s, j + 1));
                    }
                    arith_at = false;
                }
            }
            _ => {}
        }
        j += 1;
    }
    let s = Scanned {
        inner: Some(rest(i)),
        end: Some(n),
        closes: false,
    };
    Ok((s, n))
}

/// `_substitution`: at a `$(` (`text[j]` is `$`), (the text to lex, index after the closing `)`, whether it
/// is arithmetic). A `$((...))` that closes with `))` is arithmetic, lexed as a `((...))` command; any other
/// `$((` is a command substitution holding a subshell. Its text can start with `((` too (`$(((1)); X)`), so
/// only the flag tells the two apart (DT-4 review C-1).
pub(crate) fn substitution<'t>(
    text: &'t str,
    j: usize,
    memo: &mut Memo,
    work: &mut Work,
) -> Result<(Cow<'t, str>, usize, bool), TooComplex> {
    if byte_at(text, j + 2) == Some(b'(') {
        let s = scan_paren_ex(text, j + 2, true, memo, work)?;
        if s.closes {
            let inner = s.inner.unwrap_or("");
            return Ok((
                Cow::Owned(format!("({inner})")),
                s.end.unwrap_or(text.len()),
                true,
            ));
        }
    }
    let (inner, end) = scan_paren(text, j + 2, memo, work)?;
    Ok((Cow::Borrowed(inner), end, false))
}

/// One span `_body_substitutions` found: (index, end index, text to lex, is a `$(...)`).
pub(crate) type BodySub<'t> = (usize, usize, Cow<'t, str>, bool);

/// `_body_substitutions`: the `$(...)` and backtick spans in text bash expands like an unquoted heredoc body
/// or a double-quoted string (quotes have no effect, `\$` is escaped). A backtick span is listed as written
/// and, when it differs, as bash runs it.
pub(crate) fn body_substitutions<'t>(
    body: &'t str,
    dq: bool,
    work: &mut Work,
) -> Result<Vec<BodySub<'t>>, TooComplex> {
    let bytes = body.as_bytes();
    let n = body.len();
    let mut found = Vec::new();
    let mut memo = Memo::default();
    work.charge(n)?;
    let mut j = 0;
    while j < n {
        match bytes[j] {
            b'\\' => j = skip_escape(body, j),
            b'$' if byte_at(body, j + 1) == Some(b'(') => {
                let (inner, k, arith) = substitution(body, j, &mut memo, work)?;
                found.push((j, k, inner, !arith));
                j = k;
            }
            b'`' => {
                let Some(k) = backtick_end(body, j + 1, work)? else {
                    break;
                };
                let raw = &body[j + 1..k];
                found.push((j, k + 1, Cow::Borrowed(raw), false));
                if let Some(unescaped) = unescape_backticks(raw, dq, work)? {
                    found.push((j, k + 1, Cow::Owned(unescaped), false));
                }
                j = k + 1;
            }
            _ => j += 1,
        }
    }
    Ok(found)
}

/// `_bracket_pairs`: {index of a `[`: index of the `]` that closes it} over all of `text`, escapes skipped.
pub(crate) fn bracket_pairs(
    text: &str,
    work: &mut Work,
) -> Result<HashMap<usize, usize>, TooComplex> {
    work.charge(text.len())?;
    let bytes = text.as_bytes();
    let mut pairs = HashMap::new();
    let mut stack = Vec::new();
    let mut j = 0;
    while j < bytes.len() {
        match bytes[j] {
            b'\\' => {
                j = skip_escape(text, j);
                continue;
            }
            b'[' => stack.push(j),
            b']' => {
                if let Some(open) = stack.pop() {
                    pairs.insert(open, j);
                }
            }
            _ => {}
        }
        j += 1;
    }
    Ok(pairs)
}
