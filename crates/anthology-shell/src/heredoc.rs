//! Here-document bodies (SH1, fix-wave H): guard_rules.py `_find_heredoc_end` (with `_heredoc_candidates`,
//! `_heredoc_end_scan`, `_comsub_cut`) and `_join_continuations`.
//!
//! A terminator is a line equal to one of the delimiter's readings, exactly (`<<-` strips leading tabs only);
//! for an unquoted delimiter a line ending in an odd run of backslashes is joined to the next. Inside the text
//! of a `$(...)` a line that starts with the delimiter and holds a parenthesis may also end the body, read
//! three ways (see `Reading`).

use crate::lexer::{Reading, TooComplex, Work, find_from};

/// `_comsub_cut`: inside `$(...)`, does a body line made of the delimiter then `rest` end the body? bash 5 ends
/// it when `rest` holds a `)`; bash 3.2 reads on to the exact terminator. Each heredoc in `$(...)` is read three
/// ways, one per pass over the command: as bash 5 does, up to the exact terminator, and cut at any parenthesis.
/// The other passes run only when a line like this was seen (`Work::paren_lines`).
fn comsub_cut(rest: &str, work: &mut Work) -> bool {
    if !rest.contains([')', '(']) {
        return false;
    }
    work.paren_lines = true;
    match work.reading {
        Reading::Wide => true,
        Reading::Bash5 => rest.contains(')'),
        Reading::Exact => false,
    }
}

/// Leading tabs stripped when `strip_tabs`.
fn head(line: &str, strip_tabs: bool) -> &str {
    if strip_tabs {
        line.trim_start_matches('\t')
    } else {
        line
    }
}

/// `_odd_trailing_backslashes`.
fn odd_trailing_backslashes(line: &str) -> bool {
    let run = line.len() - line.trim_end_matches('\\').len();
    run % 2 == 1
}

/// `_find_heredoc_end`: for a body starting at `text[i]`, (end of the body, index just past the terminator),
/// or `None` when no terminator appears. `join` (an unquoted delimiter) reads a line ending in an odd run of
/// backslashes together with the next. With `comsub` a line that starts with the delimiter may end the body
/// (`comsub_cut`); the scan then resumes right after the delimiter.
pub(crate) fn find_heredoc_end(
    text: &str,
    i: usize,
    delimiters: &[String],
    strip_tabs: bool,
    join: bool,
    comsub: bool,
    work: &mut Work,
) -> Result<Option<(usize, usize)>, TooComplex> {
    let found = end_scan(text, i, delimiters, strip_tabs, join, comsub, work);
    let upto = found.map_or(text.len(), |(_, after)| after);
    work.charge(upto.saturating_sub(i))?;
    Ok(found)
}

/// `_heredoc_candidates`: the terminator when no line is joined, jumping between lines that start with a
/// delimiter. Lines start at `i` only when `i` starts a line (`^` with a start position).
fn candidates(
    text: &str,
    i: usize,
    delimiters: &[String],
    strip_tabs: bool,
    comsub: bool,
    work: &mut Work,
) -> Option<(usize, usize)> {
    let n = text.len();
    let bytes = text.as_bytes();
    let mut start = if i == 0 || bytes.get(i - 1) == Some(&b'\n') {
        i
    } else {
        find_from(text, i, b'\n')? + 1
    };
    loop {
        if start > n {
            return None;
        }
        let stop = find_from(text, start, b'\n').unwrap_or(n);
        // every line is looked at; Python's regex only jumps to the lines these checks can accept
        let line = &text[start..stop];
        let h = head(line, strip_tabs);
        if delimiters.iter().any(|d| d == h) {
            return Some((start, (stop + 1).min(n)));
        }
        if comsub {
            for d in delimiters {
                if h.starts_with(d.as_str()) && comsub_cut(&h[d.len()..], work) {
                    return Some((start, start + (line.len() - h.len()) + d.len()));
                }
            }
        }
        if stop >= n {
            return None;
        }
        start = stop + 1;
    }
}

fn end_scan(
    text: &str,
    mut i: usize,
    delimiters: &[String],
    strip_tabs: bool,
    join: bool,
    comsub: bool,
    work: &mut Work,
) -> Option<(usize, usize)> {
    let n = text.len();
    let found = candidates(text, i, delimiters, strip_tabs, comsub, work);
    let upto = found.map_or(n, |(start, _)| start);
    if !join || !text.get(i..upto).is_some_and(|s| s.contains("\\\n")) {
        return found; // no joined line before it
    }
    while i < n {
        let end = find_from(text, i, b'\n');
        let mut stop = end.unwrap_or(n);
        let start = i;
        let line = &text[i..stop];
        let joined;
        let logical: &str =
            if end.is_some() && line.ends_with('\\') && odd_trailing_backslashes(line) {
                let mut parts = String::from(&line[..line.len() - 1]);
                let mut end_k = end;
                loop {
                    // a logical line over several lines
                    i = end_k.map_or(n, |e| e + 1);
                    end_k = find_from(text, i, b'\n');
                    stop = end_k.unwrap_or(n);
                    let part = &text[i..stop];
                    if end_k.is_some() && part.ends_with('\\') && odd_trailing_backslashes(part) {
                        parts.push_str(&part[..part.len() - 1]);
                        continue;
                    }
                    parts.push_str(part);
                    break;
                }
                joined = parts;
                // the physical line used for the comsub check stays the first one
                let h = head(&joined, strip_tabs);
                if delimiters.iter().any(|d| d == h) {
                    return Some((start, (stop + 1).min(n)));
                }
                if comsub && (line.contains('(') || line.contains(')')) {
                    let first = head(line, strip_tabs);
                    for d in delimiters {
                        if first.starts_with(d.as_str()) && comsub_cut(&first[d.len()..], work) {
                            return Some((start, start + (line.len() - first.len()) + d.len()));
                        }
                    }
                }
                match end_k {
                    None => break,
                    Some(e) => {
                        i = e + 1;
                        continue;
                    }
                }
            } else {
                line
            };
        let h = head(logical, strip_tabs);
        if delimiters.iter().any(|d| d == h) {
            return Some((start, (stop + 1).min(n)));
        }
        if comsub && (line.contains('(') || line.contains(')')) {
            let first = head(line, strip_tabs);
            for d in delimiters {
                if first.starts_with(d.as_str()) && comsub_cut(&first[d.len()..], work) {
                    return Some((start, start + (line.len() - first.len()) + d.len()));
                }
            }
        }
        match end {
            None => break,
            Some(e) => i = e + 1,
        }
    }
    None
}

/// `_join_continuations`: an unquoted heredoc body as bash expands it, each backslash-newline removed.
pub(crate) fn join_continuations(body: &str, work: &mut Work) -> Result<String, TooComplex> {
    if !body.contains("\\\n") {
        return Ok(body.to_string());
    }
    work.charge(body.len())?;
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(k) = rest.find('\\') {
        out.push_str(&rest[..k]);
        let after = &rest[k + 1..];
        match after.chars().next() {
            Some('\n') => rest = &after[1..],
            Some(ch) => {
                out.push('\\');
                out.push(ch);
                rest = &after[ch.len_utf8()..];
            }
            None => {
                out.push('\\');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    Ok(out)
}
