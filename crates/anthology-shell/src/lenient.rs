//! Lenient lexing of unbalanced input (SH1, fix-wave H): guard_rules.py `_lenient_segments`, the fallback
//! when a quote or substitution is unterminated. Whitespace split with the quote characters read as spaces
//! and, when that differs, also with them dropped (`x=$'..' cmd`, `n'p'x` stay one word); each embedded
//! shell script (`bash -c <rest of the line>`) is also read as a command of its own.
//!
//! Rust stricter (D-8, FW-H review R1-M1): a third reading decodes each closed `$'...'` (bash 5) before the
//! quotes are dropped, so `$'\x6epx' playwright install` followed by an unbalanced quote still shows `npx`.
//!
//! The output can grow with the cube of the word count (each `fish` word rescans the rest, and each start
//! copies the rest of the words), so every copied word is charged to the work budget (fail closed, D-7).

use crate::lexer::words::ansi_c;
use crate::lexer::{Segment, Shared, TooComplex, Work, pystr};
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

/// `piece` with each closed `$'...'` replaced by its bash 5 decoding, then quotes dropped as `drop_quotes`;
/// `None` when it holds no closed `$'...'`.
fn decode_ansi_c(piece: &str, work: &mut Work) -> Result<Option<String>, TooComplex> {
    let mut out = String::with_capacity(piece.len());
    let mut last = 0;
    let mut found = false;
    let mut from = 0;
    while let Some(at) = piece[from..].find("$'").map(|k| from + k) {
        match ansi_c(piece, at + 2, work)? {
            Some((decoded, end)) => {
                out.push_str(&drop_quotes(&piece[last..at]));
                out.push_str(&decoded);
                last = end;
                from = end;
                found = true;
            }
            None => break,
        }
    }
    if !found {
        return Ok(None);
    }
    out.push_str(&drop_quotes(&piece[last..]));
    Ok(Some(out))
}

fn words(text: &str) -> Vec<String> {
    pystr::split_ws(text)
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// SH-F2 review M-2: a `<<` right after a compound's closing word (`done`, `fi`, `esac`, `}`, `)`).
fn compound_heredoc(text: &str) -> bool {
    text.match_indices("<<").any(|(at, _)| {
        let before = text[..at].trim_end_matches([' ', '\t']);
        before.ends_with(['}', ')']) || ["done", "fi", "esac"].iter().any(|w| before.ends_with(w))
    })
}

/// `_lenient_segments(text)`: every segment gets a group of its own.
pub(crate) fn lenient_segments(text: &str, sh: &mut Shared) -> Result<Vec<Segment>, TooComplex> {
    let mut out = Vec::new();
    sh.work.charge(2 * text.len())?;
    // #89 review F-1: the split cuts a process substitution and drops a pipe, though bash runs every complete
    // line before the broken one; a broken command with either is too complex. SH-F2 review M-2: so is one
    // with a here-string, or a heredoc given to a compound (the split loses whom it feeds: `( sh ) <<< X`,
    // `done <<EOF`, then a broken line); a heredoc of a plain command stays (`git commit -m "$(cat <<'EOF'`)
    if text.contains("<(")
        || text.contains(">(")
        || text.contains('|')
        || text.contains("<<<")
        || compound_heredoc(text)
    {
        return Err(TooComplex);
    }
    for piece in pieces(text) {
        let spaced = words(&piece.replace(['\'', '"'], " "));
        let joined = words(&drop_quotes(piece));
        let mut readings = if joined != spaced {
            vec![spaced, joined]
        } else {
            vec![spaced]
        };
        if let Some(decoded) = decode_ansi_c(piece, &mut sh.work)? {
            let decoded = words(&decoded);
            if !readings.contains(&decoded) {
                readings.push(decoded);
            }
        }
        for tokens in readings {
            if tokens.is_empty() {
                continue;
            }
            // Every copied word costs WORD + its length. `cost[k]` is the cost of tokens[k..].
            let mut cost = vec![0usize; tokens.len() + 1];
            for k in (0..tokens.len()).rev() {
                cost[k] = cost[k + 1].saturating_add(WORD + tokens[k].len());
            }
            sh.work.charge_extra(cost[0])?;
            out.push(Segment {
                tokens: tokens.clone(),
                redirs: Vec::new(),
                group: sh.new_group(),
                arith: false,
                braces: true,
                after_compound: false,
                quoted_braces: true, // quoting is unknown: no brace reading is trusted
                defines: true,       // nor whether a `(` follows
            });
            let starts = script_starts_in(&tokens, &mut sh.work.extra).ok_or(TooComplex)?;
            for (start, first) in starts {
                let rest = tokens.get(start + 1..).unwrap_or(&[]);
                let rest_cost = cost.get(start + 1).copied().unwrap_or(0);
                sh.work.charge(tokens.len() - start)?;
                sh.work.charge_extra(WORD + first.len() + rest_cost)?;
                let mut extra = Vec::with_capacity(1 + rest.len());
                extra.push(first);
                extra.extend(rest.iter().cloned());
                out.push(Segment {
                    tokens: extra,
                    redirs: Vec::new(),
                    group: sh.new_group(),
                    arith: false,
                    braces: true,
                    after_compound: false,
                    quoted_braces: true, // quoting is unknown: no brace reading is trusted
                    defines: true,
                });
            }
        }
    }
    Ok(out)
}
