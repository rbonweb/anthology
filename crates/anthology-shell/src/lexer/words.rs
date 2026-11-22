//! Word-level scanners of the lexer (FW-H): `$'...'` decoding, `$(`/`${`/backtick matching inside words,
//! the heredoc delimiter word, and the operator reader. Ports of guard_rules.py (fix-wave H) `_ansi_c_end`,
//! `_ansi_c_decode`, `_word_parens`, `_brace_end`, `_backtick_end`, `_unescape_backticks`, `_heredoc_word`,
//! `_remove_quotes`, `_skip_continuations`, `_input_operator` and `_heredoc_after`.
//!
//! As in the rest of the lexer, indexes are bytes: every decision is taken at an ASCII byte, and the code
//! point after a backslash, or any copied character, is taken whole. Each scan charges the bytes it visited
//! to the work budget, where Python charges the characters (`_charge`).

use super::{TooComplex, Work, byte_at, char_at, find_from, skip_escape};

/// The end of a word in a heredoc delimiter (`_WORD_END`).
fn word_end(b: u8) -> bool {
    matches!(
        b,
        b' ' | b'\t' | b'\n' | b';' | b'&' | b'|' | b'<' | b'>' | b'(' | b')'
    )
}

/// `_ansi_c_end`: the index of the `'` closing a `$'...'` whose body starts at `j`, or `None`. The end is
/// found first, by backslash pairs (`\c'` ends the string), and decoded after.
pub(crate) fn ansi_c_end(
    text: &str,
    mut j: usize,
    work: &mut Work,
) -> Result<Option<usize>, TooComplex> {
    let bytes = text.as_bytes();
    let (n, start) = (text.len(), j);
    while j < n {
        match bytes[j] {
            b'\\' => j = skip_escape(text, j),
            b'\'' => {
                work.charge(j - start)?;
                return Ok(Some(j));
            }
            _ => j += 1,
        }
    }
    work.charge(n.saturating_sub(start))?;
    Ok(None)
}

/// A code point as Python's `chr` gives it; a lone surrogate (which a Rust `char` cannot hold) becomes
/// U+FFFD (GC-16 D-6).
fn py_chr(value: u32) -> char {
    char::from_u32(value).unwrap_or('\u{fffd}')
}

/// `_ansi_c_decode`: the text of a `$'...'` body as bash 5 decodes it, or as bash 3.2 does when `bash3` (no
/// `\u`/`\U`, its own `\c`). A NUL ends the string.
pub(crate) fn ansi_c_decode(
    body: &str,
    bash3: bool,
    work: &mut Work,
) -> Result<String, TooComplex> {
    work.charge(body.len())?;
    let bytes = body.as_bytes();
    let n = body.len();
    let mut out = String::with_capacity(n);
    let mut j = 0;
    while j < n {
        let c = bytes[j];
        if c != b'\\' || j + 1 >= n {
            let ch = char_at(body, j).unwrap_or('\u{fffd}');
            out.push(ch);
            j += ch.len_utf8();
            continue;
        }
        let e = char_at(body, j + 1).unwrap_or('\u{fffd}');
        let simple = match e {
            'a' => Some('\x07'),
            'b' => Some('\x08'),
            'e' | 'E' => Some('\x1b'),
            'f' => Some('\x0c'),
            'n' => Some('\n'),
            'r' => Some('\r'),
            't' => Some('\t'),
            'v' => Some('\x0b'),
            '\\' => Some('\\'),
            '\'' => Some('\''),
            '"' => Some('"'),
            '?' => Some('?'),
            _ => None,
        };
        if let Some(s) = simple {
            out.push(s);
            j += 2;
        } else if ('0'..='7').contains(&e) {
            let mut k = j + 1;
            while k < (j + 4).min(n) && (b'0'..=b'7').contains(&bytes[k]) {
                k += 1;
            }
            let value = u32::from_str_radix(&body[j + 1..k], 8).unwrap_or(0) & 0xFF;
            out.push(py_chr(value));
            j = k;
        } else if matches!(e, 'x' | 'u' | 'U') && !(bash3 && e != 'x') {
            let width = match e {
                'x' => 2,
                'u' => 4,
                _ => 8,
            };
            let mut k = j + 2;
            while k < (j + 2 + width).min(n) && bytes[k].is_ascii_hexdigit() {
                k += 1;
            }
            if k == j + 2 {
                out.push_str(&body[j..k]); // `\x` without digits stays as is
            } else {
                let value = u32::from_str_radix(&body[j + 2..k], 16).unwrap_or(0);
                out.push(if value <= 0x10FFFF {
                    py_chr(value)
                } else {
                    '\u{fffd}'
                });
            }
            j = k;
        } else if e == 'c' {
            // `\cX`: control-X
            if j + 2 >= n {
                out.push_str(if bash3 { "\\" } else { "\\c" }); // a trailing `\c`
                j += 2;
                continue;
            }
            let x = char_at(body, j + 2).unwrap_or('\u{fffd}');
            j += 2 + x.len_utf8();
            if x == '\\' && !bash3 && byte_at(body, j) == Some(b'\\') {
                j += 1; // bash 5: `\c\\` is one control-\\
            }
            let x = if x.is_ascii_lowercase() {
                x.to_ascii_uppercase()
            } else {
                x
            };
            out.push(if x == '?' && !bash3 {
                '\x7f'
            } else {
                py_chr(x as u32 & 0x1F)
            });
        } else {
            out.push('\\');
            out.push(e);
            j += 1 + e.len_utf8();
        }
    }
    if let Some(nul) = out.find('\0') {
        out.truncate(nul);
    }
    Ok(out)
}

/// `_ansi_c`: decode the `$'...'` whose body starts at `j` (bash 5): (decoded, index past the closing quote),
/// or `None` when the quote never closes.
pub(crate) fn ansi_c(
    text: &str,
    j: usize,
    work: &mut Work,
) -> Result<Option<(String, usize)>, TooComplex> {
    let Some(k) = ansi_c_end(text, j, work)? else {
        return Ok(None);
    };
    Ok(Some((ansi_c_decode(&text[j..k], false, work)?, k + 1)))
}

/// `_word_parens`: (inner, index after `)`) for a `$(` opened just before `j` inside a delimiter word or a
/// `${...}`; only quotes and escapes are skipped. Unclosed: (rest, `None`).
pub(crate) fn word_parens<'t>(
    text: &'t str,
    mut j: usize,
    work: &mut Work,
) -> Result<(&'t str, Option<usize>), TooComplex> {
    let bytes = text.as_bytes();
    let (n, start) = (text.len(), j);
    let mut depth = 1usize;
    while j < n {
        let c = bytes[j];
        if c == b'\\' {
            j = skip_escape(text, j);
            continue;
        }
        if c == b'\'' || c == b'"' {
            j = find_from(text, j + 1, c).map_or(n, |k| k + 1);
            continue;
        }
        if c == b'(' {
            depth += 1;
        } else if c == b')' {
            depth -= 1;
            if depth == 0 {
                work.charge(j - start)?;
                return Ok((&text[start..j], Some(j + 1)));
            }
        }
        j += 1;
    }
    work.charge(n.saturating_sub(start))?;
    Ok((text.get(start..).unwrap_or(""), None))
}

/// `_Memo`: scans of one text that ran to its end without closing are not repeated from a later index.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Memo {
    /// The least index a `${` was found unclosed from.
    pub(crate) brace: Option<usize>,
}

/// `_brace_end`: the index of the `}` closing a `${` opened just before `j`, or `None`. Nested `${`, quotes
/// (also inside double quotes), escapes, `$'...'` and `$(...)` are skipped.
pub(crate) fn brace_end(
    text: &str,
    j: usize,
    memo: Option<&mut Memo>,
    work: &mut Work,
) -> Result<Option<usize>, TooComplex> {
    if let Some(m) = &memo
        && let Some(at) = m.brace
        && j >= at
    {
        return Ok(None);
    }
    let k = brace_scan(text, j, work)?;
    if k.is_none()
        && let Some(m) = memo
    {
        m.brace = Some(m.brace.map_or(j, |at| at.min(j)));
    }
    Ok(k)
}

fn brace_scan(text: &str, mut j: usize, work: &mut Work) -> Result<Option<usize>, TooComplex> {
    let bytes = text.as_bytes();
    let (n, start) = (text.len(), j);
    let mut depth = 1usize;
    while j < n {
        match bytes[j] {
            b'\\' => j = skip_escape(text, j),
            b'\'' => match find_from(text, j + 1, b'\'') {
                Some(k) => j = k + 1,
                None => break,
            },
            b'"' => {
                j += 1;
                while j < n && bytes[j] != b'"' {
                    j = if bytes[j] == b'\\' {
                        skip_escape(text, j)
                    } else {
                        j + 1
                    };
                }
                if j >= n {
                    break;
                }
                j += 1;
            }
            b'$' if byte_at(text, j + 1) == Some(b'{') => {
                depth += 1;
                j += 2;
            }
            b'$' if byte_at(text, j + 1) == Some(b'\'') => match ansi_c_end(text, j + 2, work)? {
                Some(k) => j = k + 1,
                None => break,
            },
            b'$' if byte_at(text, j + 1) == Some(b'(') => match word_parens(text, j + 2, work)?.1 {
                Some(k) => j = k,
                None => break,
            },
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    work.charge(j - start)?;
                    return Ok(Some(j));
                }
                j += 1;
            }
            _ => j += 1,
        }
    }
    work.charge(n.saturating_sub(start))?;
    Ok(None)
}

/// `_backtick_end`: the backtick closing one opened just before `j` (escaped ones skipped), or `None`.
pub(crate) fn backtick_end(
    text: &str,
    mut j: usize,
    work: &mut Work,
) -> Result<Option<usize>, TooComplex> {
    let bytes = text.as_bytes();
    let (n, start) = (text.len(), j);
    while j < n {
        match bytes[j] {
            b'\\' => j = skip_escape(text, j),
            b'`' => {
                work.charge(j - start)?;
                return Ok(Some(j));
            }
            _ => j += 1,
        }
    }
    work.charge(n.saturating_sub(start))?;
    Ok(None)
}

/// `_unescape_backticks`: the text bash runs for a backtick substitution: `\\`, `` \` `` and `\$` (and `\"`
/// inside double quotes) lose their backslash. `None` when nothing changes.
pub(crate) fn unescape_backticks(
    inner: &str,
    dq: bool,
    work: &mut Work,
) -> Result<Option<String>, TooComplex> {
    if !inner.contains('\\') {
        return Ok(None);
    }
    work.charge(inner.len())?;
    let bytes = inner.as_bytes();
    let mut out = String::with_capacity(inner.len());
    let mut j = 0;
    let mut changed = false;
    while j < inner.len() {
        if bytes[j] == b'\\'
            && let Some(next @ (b'\\' | b'`' | b'$' | b'"')) = byte_at(inner, j + 1)
            && (next != b'"' || dq)
        {
            out.push(next as char);
            j += 2;
            changed = true;
            continue;
        }
        let ch = char_at(inner, j).unwrap_or('\u{fffd}');
        out.push(ch);
        j += ch.len_utf8();
    }
    Ok(changed.then_some(out))
}

/// A parsed heredoc delimiter word (`_heredoc_word`).
#[derive(Clone, Debug)]
pub(crate) enum HeredocWord<'t> {
    /// No word at that index: no heredoc.
    None,
    /// A quote or substitution in the word never closes: bash reads the rest of the input into it.
    Unclosed,
    Word {
        /// The word as bash 5 reads it, then as bash 3.2 reads it when that differs (`$'A'`).
        delimiters: Vec<String>,
        quoted: bool,
        /// The `$(...)` / backtick spans spelled in the word (bash does not run them).
        subs: Vec<&'t str>,
        end: usize,
    },
}

/// `_heredoc_word`: the heredoc delimiter word starting at `j`.
pub(crate) fn heredoc_word<'t>(
    text: &'t str,
    j: usize,
    work: &mut Work,
) -> Result<HeredocWord<'t>, TooComplex> {
    let word = heredoc_word_scan(text, j, work)?;
    match &word {
        HeredocWord::None => {}
        HeredocWord::Unclosed => work.charge(text.len().saturating_sub(j))?,
        HeredocWord::Word { end, .. } => work.charge(end - j)?,
    }
    Ok(word)
}

fn heredoc_word_scan<'t>(
    text: &'t str,
    mut j: usize,
    work: &mut Work,
) -> Result<HeredocWord<'t>, TooComplex> {
    let bytes = text.as_bytes();
    let (n, start) = (text.len(), j);
    let mut out = String::new();
    let mut out3 = String::new();
    let mut subs = Vec::new();
    let mut quoted = false;
    fn put(out: &mut String, out3: &mut String, piece: &str) {
        out.push_str(piece);
        out3.push_str(piece);
    }
    while j < n && !word_end(bytes[j]) {
        let c = bytes[j];
        let nxt = byte_at(text, j + 1);
        match c {
            b'\\' => {
                if nxt == Some(b'\n') {
                    j += 2; // line continuation, not a quote
                    continue;
                }
                quoted = true;
                match char_at(text, j + 1) {
                    Some(ch) => {
                        let mut buf = [0u8; 4];
                        put(&mut out, &mut out3, ch.encode_utf8(&mut buf));
                        j += 1 + ch.len_utf8();
                    }
                    None => {
                        put(&mut out, &mut out3, "\\");
                        j += 1;
                    }
                }
            }
            b'\'' => {
                let Some(k) = find_from(text, j + 1, b'\'') else {
                    return Ok(HeredocWord::Unclosed);
                };
                quoted = true;
                put(&mut out, &mut out3, &text[j + 1..k]);
                j = k + 1;
            }
            b'"' => {
                quoted = true;
                j += 1;
                while j < n && bytes[j] != b'"' {
                    let d = bytes[j];
                    if d == b'\\'
                        && matches!(
                            byte_at(text, j + 1),
                            Some(b'\\' | b'$' | b'"' | b'`' | b'\n')
                        )
                    {
                        if bytes[j + 1] != b'\n' {
                            put(&mut out, &mut out3, &text[j + 1..j + 2]);
                        }
                        j += 2;
                    } else if d == b'$' && byte_at(text, j + 1) == Some(b'(') {
                        let (inner, k) = word_parens(text, j + 2, work)?;
                        let Some(k) = k else {
                            return Ok(HeredocWord::Unclosed);
                        };
                        subs.push(inner);
                        put(&mut out, &mut out3, &text[j..k]);
                        j = k;
                    } else {
                        let ch = char_at(text, j).unwrap_or('\u{fffd}');
                        let mut buf = [0u8; 4];
                        put(&mut out, &mut out3, ch.encode_utf8(&mut buf));
                        j += ch.len_utf8();
                    }
                }
                if j >= n {
                    return Ok(HeredocWord::Unclosed);
                }
                j += 1;
            }
            b'$' if nxt == Some(b'\'') => {
                let Some(k) = ansi_c_end(text, j + 2, work)? else {
                    return Ok(HeredocWord::Unclosed);
                };
                quoted = true;
                out.push_str(&ansi_c_decode(&text[j + 2..k], false, work)?);
                out3.push_str(&ansi_c_decode(&text[j + 2..k], true, work)?);
                j = k + 1;
            }
            b'$' if nxt == Some(b'"') => j += 1, // `$"..."` quotes like `"..."`
            b'$' if nxt == Some(b'(') => {
                let (inner, k) = word_parens(text, j + 2, work)?;
                let Some(k) = k else {
                    return Ok(HeredocWord::Unclosed);
                };
                subs.push(inner);
                out.push_str(&text[j..k]);
                out3.push_str(&remove_quotes(&text[j..k], work)?);
                j = k;
            }
            b'$' if nxt == Some(b'{') => {
                let Some(k) = brace_end(text, j + 2, None, work)? else {
                    return Ok(HeredocWord::Unclosed);
                };
                out.push_str(&text[j..=k]);
                out3.push_str(&remove_quotes(&text[j..=k], work)?);
                j = k + 1;
            }
            b'`' => {
                let Some(k) = backtick_end(text, j + 1, work)? else {
                    return Ok(HeredocWord::Unclosed);
                };
                subs.push(&text[j + 1..k]);
                out.push_str(&text[j..=k]);
                out3.push_str(&remove_quotes(&text[j..=k], work)?);
                j = k + 1;
            }
            _ => {
                let ch = char_at(text, j).unwrap_or('\u{fffd}');
                let mut buf = [0u8; 4];
                put(&mut out, &mut out3, ch.encode_utf8(&mut buf));
                j += ch.len_utf8();
            }
        }
    }
    if j == start {
        return Ok(HeredocWord::None);
    }
    let delimiters = if out3 == out {
        vec![out]
    } else {
        vec![out, out3]
    };
    Ok(HeredocWord::Word {
        delimiters,
        quoted,
        subs,
        end: j,
    })
}

/// `_remove_quotes`: `piece` after shell quote removal (bash 3.2 applies it to the whole delimiter word).
fn remove_quotes(piece: &str, work: &mut Work) -> Result<String, TooComplex> {
    if !piece.contains(['\\', '\'', '"']) {
        return Ok(piece.to_string());
    }
    work.charge(piece.len())?;
    let bytes = piece.as_bytes();
    let n = piece.len();
    let mut out = String::with_capacity(n);
    let mut j = 0;
    while j < n {
        match bytes[j] {
            b'\\' => {
                if let Some(ch) = char_at(piece, j + 1) {
                    out.push(ch);
                }
                j = skip_escape(piece, j);
            }
            b'\'' => {
                let k = find_from(piece, j + 1, b'\'').unwrap_or(n);
                out.push_str(&piece[j + 1..k]);
                j = k + 1;
            }
            b'"' => {
                j += 1;
                while j < n && bytes[j] != b'"' {
                    if bytes[j] == b'\\'
                        && matches!(byte_at(piece, j + 1), Some(b'\\' | b'$' | b'"' | b'`'))
                    {
                        j += 1;
                    }
                    if let Some(ch) = char_at(piece, j) {
                        out.push(ch);
                        j += ch.len_utf8();
                    } else {
                        j += 1;
                    }
                }
                j += 1;
            }
            _ => {
                let ch = char_at(piece, j).unwrap_or('\u{fffd}');
                out.push(ch);
                j += ch.len_utf8();
            }
        }
    }
    Ok(out)
}

/// `_skip_continuations`: the index past any backslash-newlines at `k`.
pub(crate) fn skip_continuations(text: &str, mut k: usize) -> usize {
    while text.as_bytes().get(k..k + 2) == Some(b"\\\n") {
        k += 2;
    }
    k
}

/// `_input_operator`: at a `<`, (operator, index after it): `<`, `<<`, `<<-`, `<<<`, `<>` or `<&`, read
/// across backslash-newlines.
pub(crate) fn input_operator(text: &str, j: usize) -> (&'static str, usize) {
    let k = skip_continuations(text, j + 1);
    match byte_at(text, k) {
        Some(b'>') => return ("<>", skip_continuations(text, k + 1)),
        Some(b'&') => return ("<&", skip_continuations(text, k + 1)),
        Some(b'<') => {}
        _ => return ("<", k),
    }
    let k = skip_continuations(text, k + 1);
    match byte_at(text, k) {
        Some(b'<') => ("<<<", skip_continuations(text, k + 1)),
        Some(b'-') => ("<<-", skip_continuations(text, k + 1)),
        _ => ("<<", k),
    }
}

/// `_heredoc_after`: after a `<<` / `<<-` operator, the delimiter word, blanks and backslash-newlines before
/// it skipped.
pub(crate) fn heredoc_after<'t>(
    text: &'t str,
    mut k: usize,
    work: &mut Work,
) -> Result<HeredocWord<'t>, TooComplex> {
    loop {
        match byte_at(text, k) {
            Some(b' ' | b'\t') => k += 1,
            Some(b'\\') if byte_at(text, k + 1) == Some(b'\n') => k += 2,
            _ => return heredoc_word(text, k, work),
        }
    }
}
