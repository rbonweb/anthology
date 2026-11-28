//! The secret masker behind `anthology guard mask`: Bash output goes through it before Claude
//! reads it (the `masking.ts` mod). It uses the G6 patterns (`secrets::GUARD_SECRET_PATTERNS`) to
//! find a likely secret, then a rule per pattern decides whether the hit is one, so a lockfile
//! hash, a git SHA, a CSS class name or code that only reads a secret stays as it was.
//!
//! Each rule works on one line; a private key is the one block rule. A line the regex engine gives
//! up on (`LimitError`) is left as it is: masking fails open here, the guard's own G6 does not.
//!
//! | # | replaced | only when |
//! |---|---|---|
//! | 0 | the match | the byte before is not `[A-Za-z0-9+/]` and the byte after is not `[A-Za-z0-9+/=]` |
//! | 1 | the match | the tail after `_` is at least 30 characters |
//! | 2 | the match and the `[A-Za-z0-9_]` run after it | that run is at least 20 characters |
//! | 3 | the match | it holds a digit and an ASCII capital |
//! | 4 | the match and the `[A-Za-z0-9-]` run after it | that run is at least 10 characters and holds a digit |
//! | 5 | the lines between BEGIN and END, as one marker line | see [`key_block`] |
//! | 6, 7 | the value only, quotes kept | see [`credential_span`] |
//!
//! A token that starts `sha1-`, `sha256-`, `sha384-` or `sha512-` (an integrity string) is never
//! masked.

use std::sync::OnceLock;

use anthology_pycompat::re::{self, LimitError, Regex as PyRegex};
use regex::Regex;

use crate::secrets::GUARD_SECRET_PATTERNS;

/// The marker's label for each of `GUARD_SECRET_PATTERNS`, by index.
pub const MASK_LABELS: [&str; 8] = [
    "aws-access-key",
    "github-token",
    "github-token",
    "api-key",
    "slack-token",
    "private-key",
    "credential",
    "credential",
];

/// What the masker made of a text: the new text and how many places it masked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Masked {
    pub text: String,
    pub count: usize,
}

/// The text that takes a secret's place: it names the kind, never the value.
pub fn marker(label: &str) -> String {
    format!("[masked by anthology: {label}]")
}

/// The pattern of the private-key header, and the most lines after it that its END may be on.
const KEY_BEGIN: usize = 5;
const MAX_KEY_LINES: usize = 200;
/// Rules 6 and 7: the shortest value that counts, and the length that counts without a digit or
/// a symbol.
const MIN_VALUE: usize = 8;
const LONG_VALUE: usize = 24;
/// A value with a letter and one of these is a secret even without a digit.
const SYMBOLS: [char; 10] = ['!', '@', '#', '^', '&', '*', '+', '=', '~', '?'];

struct Engine {
    /// `GUARD_SECRET_PATTERNS` compiled once; a pattern that fails to build is skipped.
    patterns: Vec<Option<PyRegex>>,
    key_end: Regex,
    /// A `grep -n` (`path:12:`) or `cat -n` (`   12<TAB>`) prefix of a line.
    line_prefix: Regex,
    key_body: Regex,
    /// A dotted, `::` or `->` identifier path: code, not a secret.
    ident_path: Regex,
    ident: Regex,
    env_line: Regex,
    /// What may stand before the key on a YAML or INI line.
    key_lead: Regex,
}

fn engine() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        let re = |p: &str| Regex::new(p).expect("a fixed pattern");
        Engine {
            patterns: GUARD_SECRET_PATTERNS
                .iter()
                .map(|(py, ignorecase)| re::compile(py, *ignorecase).ok())
                .collect(),
            key_end: re(r"-----END [A-Z ]*PRIVATE KEY-----"),
            line_prefix: re(r"^(?:\s*[0-9]+\t|[^\s]*?:[0-9]+:)"),
            key_body: re(r"^(?:[A-Za-z0-9+/=]*|(?:Proc-Type|DEK-Info):.*)$"),
            ident_path: re(r"^[A-Za-z_]\w*(?:(?:\.|::|->)[A-Za-z_]\w*)+$"),
            ident: re(r"^[A-Za-z_]\w*$"),
            env_line: re(r"^\s*(?:export\s+)?[A-Z0-9_]*(?:PASSWORD|API_?KEY)[A-Z0-9_]*="),
            key_lead: re(r#"^\s*(?:-\s+)?["']?[A-Za-z0-9_.-]*$"#),
        }
    })
}

/// Masks the secrets in `text`: see the module docs. The rest of the text is returned byte for
/// byte, line endings included.
pub fn mask_secrets(text: &str) -> Masked {
    let e = engine();
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut out = String::with_capacity(text.len());
    let mut count = 0;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if let Some(end) = key_block(e, &lines, i) {
            // The header stays, the body is one marker line, the END line is the next line.
            out.push_str(line);
            out.push_str(&marker(MASK_LABELS[KEY_BEGIN]));
            out.push_str(ending(lines[i + 1]));
            count += 1;
            i = end;
            continue;
        }
        let (masked, n) =
            mask_line(e, content(line)).unwrap_or_else(|_| (content(line).to_owned(), 0));
        out.push_str(&masked);
        out.push_str(ending(line));
        count += n;
        i += 1;
    }
    Masked { text: out, count }
}

/// The line terminator at the end of `line`: `\r\n`, `\n` or nothing.
fn ending(line: &str) -> &str {
    if line.ends_with("\r\n") {
        "\r\n"
    } else if line.ends_with('\n') {
        "\n"
    } else {
        ""
    }
}

fn content(line: &str) -> &str {
    &line[..line.len() - ending(line).len()]
}

/// Rule 5: when `lines[start]` holds a private-key header, the index of the END line, if it is
/// within 200 lines and every line between is base64 or a PEM header (after an optional
/// `path:n:` or `cat -n` prefix) and there is at least one. Anything else is no key block.
fn key_block(e: &Engine, lines: &[&str], start: usize) -> Option<usize> {
    let begin = e.patterns[KEY_BEGIN].as_ref()?;
    if !matches!(begin.is_match(content(lines[start])), Ok(true)) {
        return None;
    }
    for (k, line) in lines.iter().enumerate().skip(start + 1).take(MAX_KEY_LINES) {
        let line = content(line);
        if e.key_end.is_match(line) {
            return (k > start + 1).then_some(k);
        }
        let body = match e.line_prefix.find(line) {
            Some(p) => &line[p.end()..],
            None => line,
        };
        if !e.key_body.is_match(body) {
            return None;
        }
    }
    None
}

struct Span {
    start: usize,
    end: usize,
    rule: usize,
}

/// The line (without its terminator) with every hit masked, and how many places that made. An
/// error is the regex engine giving up on this line.
fn mask_line(e: &Engine, line: &str) -> Result<(String, usize), LimitError> {
    let mut spans = Vec::new();
    for rule in [0, 1, 2, 3, 4, 6, 7] {
        let Some(pattern) = &e.patterns[rule] else {
            continue;
        };
        let mut pos = 0;
        while pos < line.len() {
            let Some((s, t)) = pattern.find_at(line, pos)? else {
                break;
            };
            if t <= s {
                break;
            }
            pos = t;
            let Some((start, end)) = hit(e, rule, line, s, t) else {
                continue;
            };
            if in_integrity_token(line, start) {
                continue;
            }
            pos = pos.max(end);
            spans.push(Span { start, end, rule });
        }
    }
    if spans.is_empty() {
        return Ok((line.to_owned(), 0));
    }
    // Overlapping spans become one, under the label of the first.
    spans.sort_by_key(|s| (s.start, std::cmp::Reverse(s.end), s.rule));
    let mut merged: Vec<Span> = Vec::new();
    for s in spans {
        match merged.last_mut() {
            Some(last) if s.start < last.end => last.end = last.end.max(s.end),
            _ => merged.push(s),
        }
    }
    let mut out = String::with_capacity(line.len());
    let mut at = 0;
    for s in &merged {
        out.push_str(&line[at..s.start]);
        out.push_str(&marker(MASK_LABELS[s.rule]));
        at = s.end;
    }
    out.push_str(&line[at..]);
    Ok((out, merged.len()))
}

fn is_base64(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=')
}

/// The length of the run at the start of `bytes` made of `[A-Za-z0-9]` and `extra`.
fn run(bytes: &[u8], extra: u8) -> usize {
    bytes
        .iter()
        .take_while(|b| b.is_ascii_alphanumeric() || **b == extra)
        .count()
}

/// The span the rule masks for the match `s..t` of pattern `rule`, or none when the rule says it
/// is no secret.
fn hit(e: &Engine, rule: usize, line: &str, s: usize, t: usize) -> Option<(usize, usize)> {
    let bytes = line.as_bytes();
    match rule {
        0 => {
            // `=` pads the end of a base64 blob, so it rules out a key only after it: `ID=AKIA...`
            // is how an env file holds one.
            let before = s > 0 && is_base64(bytes[s - 1]) && bytes[s - 1] != b'=';
            let after = t < bytes.len() && is_base64(bytes[t]);
            (!before && !after).then_some((s, t))
        }
        // `gh?_` and the tail.
        1 => (t - s - 4 >= 30).then_some((s, t)),
        2 => {
            let end = t + run(&bytes[t..], b'_');
            (end - t >= 20).then_some((s, end))
        }
        3 => {
            let text = &line[s..t];
            (text.bytes().any(|b| b.is_ascii_digit())
                && text.bytes().any(|b| b.is_ascii_uppercase()))
            .then_some((s, t))
        }
        4 => {
            let end = t + run(&bytes[t..], b'-');
            (end - t >= 10 && bytes[t..end].iter().any(u8::is_ascii_digit)).then_some((s, end))
        }
        6 => credential_span(e, line, s, s + "password".len()),
        _ => {
            // `api`, an optional `_` or `-`, `key`.
            let mut key_end = s + 3;
            if matches!(bytes.get(key_end), Some(b'_' | b'-')) {
                key_end += 1;
            }
            credential_span(e, line, s, key_end + 3)
        }
    }
}

fn skip_blanks(bytes: &[u8], mut p: usize) -> usize {
    while matches!(bytes.get(p), Some(b' ' | b'\t')) {
        p += 1;
    }
    p
}

/// A quote at `p`: its length (a backslash before it counts) and the quote byte.
fn quote_at(bytes: &[u8], p: usize) -> Option<(usize, u8)> {
    match (bytes.get(p)?, bytes.get(p + 1)) {
        (b'\\', Some(q @ (b'"' | b'\''))) => Some((2, *q)),
        (q @ (b'"' | b'\''), _) => Some((1, *q)),
        _ => None,
    }
}

/// Rules 6 and 7: the value of the credential whose key is `key_start..key_end`: to the matching
/// quote on the line when quoted, else to a blank or one of `, ; } ) ] " '`. It counts only when
/// it is 8 characters or more, has no blank and none of `( [ $ { % <`, is no identifier path, and
/// holds a digit, or a letter and one of `! @ # ^ & * + = ~ ?` or `/`, or is 24 characters or
/// more. An unquoted value shaped like an identifier is code (a variable) unless the line is an
/// env line (`DB_PASSWORD=value`) or a YAML or INI line, with the key first and the value last.
fn credential_span(
    e: &Engine,
    line: &str,
    key_start: usize,
    key_end: usize,
) -> Option<(usize, usize)> {
    let bytes = line.as_bytes();
    let mut p = key_end;
    if let Some((len, _)) = quote_at(bytes, p) {
        p += len;
    }
    let blank_before = skip_blanks(bytes, p) != p;
    p = skip_blanks(bytes, p);
    let sep = *bytes.get(p)?;
    if sep != b':' && sep != b'=' {
        return None;
    }
    p += 1;
    // `==` and `:=` compare or assign in code.
    if bytes.get(p) == Some(&b'=') {
        return None;
    }
    let spaced = blank_before || skip_blanks(bytes, p) != p;
    p = skip_blanks(bytes, p);
    let (start, end, quoted) = match quote_at(bytes, p) {
        Some((len, q)) => {
            let start = p + len;
            let found = bytes[start..].iter().position(|b| *b == q);
            let mut end = found.map_or(line.trim_end().len().max(start), |f| start + f);
            if len == 2 && found.is_some() && end > start && bytes[end - 1] == b'\\' {
                end -= 1;
            }
            (start, end, true)
        }
        None => {
            let stop = |b: &u8| b.is_ascii_whitespace() || b",;})]\"'".contains(b);
            let len = bytes[p..].iter().position(stop).unwrap_or(bytes.len() - p);
            (p, p + len, false)
        }
    };
    let value = line.get(start..end)?;
    let length = value.chars().count();
    if length < MIN_VALUE
        || value.chars().any(char::is_whitespace)
        || value.contains(['(', '[', '$', '{', '%', '<'])
        || e.ident_path.is_match(value)
    {
        return None;
    }
    let letter = value.chars().any(char::is_alphabetic);
    let symbol = value.contains(SYMBOLS) || value.contains('/');
    if !(value.bytes().any(|b| b.is_ascii_digit()) || (letter && symbol) || length >= LONG_VALUE) {
        return None;
    }
    if !quoted && e.ident.is_match(value) {
        let env = e.env_line.is_match(line);
        let to_end = line[end..].trim().is_empty();
        let yaml_ini =
            e.key_lead.is_match(&line[..key_start]) && to_end && (sep == b':' || !spaced);
        if !env && !yaml_ini {
            return None;
        }
    }
    Some((start, end))
}

/// Whether the token that holds byte `at` starts `sha1-`, `sha256-`, `sha384-` or `sha512-`.
fn in_integrity_token(line: &str, at: usize) -> bool {
    let bytes = line.as_bytes();
    let delimiter = |b: &u8| b.is_ascii_whitespace() || b"\"'`,;()<>".contains(b);
    let from = bytes[..at].iter().rposition(delimiter).map_or(0, |i| i + 1);
    let token = &line[from..];
    ["sha1-", "sha256-", "sha384-", "sha512-"]
        .iter()
        .any(|p| token.starts_with(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pattern_compiles_through_pycompat() {
        let e = engine();
        assert_eq!(e.patterns.len(), MASK_LABELS.len());
        assert!(e.patterns.iter().all(Option::is_some));
    }
}
