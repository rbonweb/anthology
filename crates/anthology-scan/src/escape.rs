//! Making untrusted text safe to print and to hand to a model (Task 7). Text from a repo is data:
//! `escape` shows what a reader could not see, and `mask` hides what looks like a secret.

use std::fmt::Write as _;

use anthology_memory::privacy::secret_match_starts;

/// True for a character `escape` writes as `\u{XXXX}`: controls other than newline and tab, and
/// the invisible, joining, direction and private-use characters a repo can hide text in.
fn is_hidden(c: char) -> bool {
    matches!(
        c as u32,
        0x00..=0x08
            | 0x0B..=0x1F
            | 0x7F..=0x9F
            | 0xAD
            | 0x34F
            | 0x61C
            | 0x115F..=0x1160
            | 0x17B4..=0x17B5
            | 0x180B..=0x180F
            | 0x200B..=0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x206F
            | 0x3164
            | 0xE000..=0xF8FF
            | 0xFE00..=0xFE0F
            | 0xFEFF
            | 0xFFA0
            | 0xFFF0..=0xFFFB
            | 0x1BCA0..=0x1BCA3
            | 0x1D173..=0x1D17A
            | 0xE0000..=0xE0FFF
            | 0xF0000..=0x10FFFF
    )
}

/// `b` as text with invisible and control characters shown as `\u{XXXX}`, invalid UTF-8 as
/// `\x{HH}`, cut at `max_chars` output characters with a trailing `…`. An escape is never cut in
/// half: one that does not fit is dropped whole. Input past the cut is not read.
pub fn escape(b: &[u8], max_chars: usize) -> String {
    let mut out = String::new();
    let mut used = 0usize;
    let mut token = String::new();
    for chunk in b.utf8_chunks() {
        let valid = chunk.valid().chars().map(|c| (c, None));
        let invalid = chunk.invalid().iter().map(|&byte| ('\0', Some(byte)));
        for (c, bad) in valid.chain(invalid) {
            token.clear();
            match bad {
                Some(byte) => {
                    let _ = write!(token, "\\x{{{byte:02X}}}");
                }
                None if is_hidden(c) => {
                    let _ = write!(token, "\\u{{{:04X}}}", c as u32);
                }
                None => token.push(c),
            }
            let width = token.chars().count();
            if used + width > max_chars {
                out.push('…');
                return out;
            }
            out.push_str(&token);
            used += width;
        }
    }
    out
}

/// `s` cut after `max_chars` characters, with a trailing `…` when something was cut.
pub(crate) fn cut(s: &str, max_chars: usize) -> String {
    match s.char_indices().nth(max_chars) {
        Some((at, _)) => format!("{}…", &s[..at]),
        None => s.to_string(),
    }
}

fn is_stop(c: char) -> bool {
    c.is_whitespace() || matches!(c, '"' | '\'' | '`')
}

/// The end of the token that starts at `from`: the next whitespace or quote.
fn token_end(s: &str, from: usize) -> usize {
    s[from..].find(is_stop).map_or(s.len(), |i| from + i)
}

/// Moves `end` past the separators after a keyword and the value that follows them.
fn extend_to_value(s: &str, end: usize) -> usize {
    let skip = |c: char| is_stop(c) || matches!(c, ':' | '=' | '\\');
    let mut at = end;
    while let Some(c) = s[at..].chars().next() {
        if !skip(c) {
            break;
        }
        at += c.len_utf8();
    }
    if at == s.len() {
        return end;
    }
    token_end(s, at)
}

/// How far the secret of one hit reaches. A token hit ends at the next whitespace or quote. A
/// keyword hit (`password = value`, `Bearer value`) also takes the value after it, and a private
/// key takes its whole block.
fn span_end(s: &str, start: usize, label: &str) -> usize {
    const KEY: &str = "KEY-----";
    if label == "private-key" {
        let begin_end = s[start..]
            .find(KEY)
            .map_or(s.len(), |i| start + i + KEY.len());
        return s[begin_end..]
            .find("-----END")
            .and_then(|i| {
                let marker = begin_end + i;
                s[marker..].find(KEY).map(|j| marker + j + KEY.len())
            })
            .unwrap_or(s.len());
    }
    let end = token_end(s, start);
    match label {
        "bearer-token" => extend_to_value(s, end),
        "basic-auth" => extend_to_value(s, extend_to_value(s, end)),
        "credential-assignment" | "cloud-secret-key" => {
            let body = &s[start..end];
            let holds_value = body.rfind([':', '=']).is_some_and(|i| i + 1 < body.len());
            if holds_value {
                end
            } else {
                extend_to_value(s, end)
            }
        }
        _ => end,
    }
}

/// `s` with every secret it holds replaced by `<secret:{label}>`. A secret runs from where the
/// privacy scan says it starts to the next whitespace or quote, and takes the value after a
/// keyword such as `password:`. When the scan could not finish, the whole text is replaced: it
/// fails closed.
pub fn mask(s: &str) -> String {
    let mut hits: Vec<(usize, &'static str)> = Vec::new();
    for (label, at) in secret_match_starts(s) {
        match at {
            Some(at) if s.is_char_boundary(at) => hits.push((at, label)),
            _ => return format!("<secret:{label}>"),
        }
    }
    hits.sort_by_key(|&(at, _)| at);
    let mut spans: Vec<(usize, usize, &'static str)> = Vec::new();
    for (start, label) in hits {
        let end = span_end(s, start, label).max(start);
        match spans.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => spans.push((start, end, label)),
        }
    }
    let mut out = String::with_capacity(s.len());
    let mut at = 0;
    for (start, end, label) in spans {
        out.push_str(&s[at..start]);
        let _ = write!(out, "<secret:{label}>");
        at = end;
    }
    out.push_str(&s[at..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A GitHub token built at run time, so no token-shaped literal sits in this source.
    fn gh_token() -> String {
        format!("gh{}_{}", "p", "A1b2C3d4".repeat(5))
    }

    #[test]
    fn escape_makes_bidi_and_tags_visible() {
        assert_eq!(escape("a\u{202E}b".as_bytes(), 100), "a\\u{202E}b");
        assert_eq!(escape("x\u{E0041}y".as_bytes(), 100), "x\\u{E0041}y");
        assert_eq!(escape("a\u{200B}b".as_bytes(), 100), "a\\u{200B}b");
        assert_eq!(escape("\u{FEFF}hi".as_bytes(), 100), "\\u{FEFF}hi");
        assert_eq!(escape("a\u{FE0F}".as_bytes(), 100), "a\\u{FE0F}");
        assert_eq!(escape("\u{1B}[31m".as_bytes(), 100), "\\u{001B}[31m");
    }

    #[test]
    fn escape_keeps_visible_text_newline_and_tab() {
        let t =
            "line one\n\tline two: caf\u{E9} \u{65E5}\u{672C} \u{1F600} \u{1F468}\u{200D}\u{1F469}";
        // The ZWJ inside the family emoji is escaped; everything else stays.
        let want = t.replace('\u{200D}', "\\u{200D}");
        assert_eq!(escape(t.as_bytes(), 1000), want);
    }

    #[test]
    fn escape_covers_every_listed_range_edge() {
        let hidden: &[u32] = &[
            0x00, 0x08, 0x0B, 0x0D, 0x1F, 0x7F, 0x80, 0x85, 0x9F, 0xAD, 0x34F, 0x61C, 0x115F,
            0x1160, 0x17B4, 0x17B5, 0x180B, 0x180F, 0x200B, 0x200F, 0x202A, 0x202E, 0x2060, 0x206F,
            0x3164, 0xFE00, 0xFE0F, 0xFEFF, 0xFFA0, 0xFFF0, 0xFFFB, 0x1BCA0, 0x1BCA3, 0x1D173,
            0x1D17A, 0xE0000, 0xE007F, 0xE0100, 0xE0FFF, 0xE000, 0xF8FF, 0xF0000, 0x10FFFF,
        ];
        for &u in hidden {
            let c = char::from_u32(u).unwrap();
            let got = escape(c.to_string().as_bytes(), 100);
            assert_eq!(got, format!("\\u{{{u:04X}}}"), "U+{u:04X}");
        }
        let shown: &[u32] = &[
            0x09, 0x0A, 0x20, 0x7E, 0xA0, 0xA1, 0xAC, 0x34E, 0x61B, 0x115E, 0x1161, 0x17B3, 0x17B6,
            0x180A, 0x1810, 0x200A, 0x2010, 0x2029, 0x202F, 0x205F, 0x2070, 0x3163, 0xFDFF, 0xFE10,
            0xFF9F, 0xFFEF, 0xFFFC, 0xFFFD, 0x1BC9F, 0x1BCA4, 0x1D172, 0x1D17B, 0xF900, 0xE1000,
            0xEFFFF, 0x1F600,
        ];
        for &u in shown {
            let Some(c) = char::from_u32(u) else { continue };
            let got = escape(c.to_string().as_bytes(), 100);
            assert_eq!(got, c.to_string(), "U+{u:04X}");
        }
    }

    #[test]
    fn escape_shows_invalid_utf8_as_hex_bytes() {
        assert_eq!(escape(b"a\xFFb", 100), "a\\x{FF}b");
        // A cut multi-byte character: each stray byte is shown.
        assert_eq!(escape(b"ok\xE2\x82", 100), "ok\\x{E2}\\x{82}");
    }

    #[test]
    fn escape_cuts_at_max_chars_with_an_ellipsis() {
        assert_eq!(escape(b"abcdef", 3), "abc\u{2026}");
        assert_eq!(escape(b"abc", 3), "abc");
        assert_eq!(escape(b"", 3), "");
        assert_eq!(escape(b"abc", 0), "\u{2026}");
    }

    #[test]
    fn escape_never_cuts_inside_an_escape() {
        // `\u{202E}` is eight output characters: with 4 left it does not fit, so it is dropped whole.
        assert_eq!(escape("ab\u{202E}cd".as_bytes(), 4), "ab\u{2026}");
        assert_eq!(escape("ab\u{202E}cd".as_bytes(), 10), "ab\\u{202E}\u{2026}");
        assert_eq!(escape("ab\u{202E}cd".as_bytes(), 12), "ab\\u{202E}cd");
    }

    #[test]
    fn escape_of_a_large_input_stops_early() {
        let big = vec![b'a'; 5_000_000];
        let out = escape(&big, 160);
        assert_eq!(out.chars().count(), 161);
    }

    #[test]
    fn mask_hides_a_token() {
        let t = gh_token();
        let text = format!("run {t} now");
        assert_eq!(mask(&text), "run <secret:github-token> now");
        assert!(!mask(&text).contains("A1b2"), "token text survived");
    }

    #[test]
    fn mask_stops_at_a_quote_and_keeps_the_rest() {
        let t = gh_token();
        let text = format!("x = \"{t}\", next");
        assert_eq!(mask(&text), "x = \"<secret:github-token>\", next");
    }

    #[test]
    fn mask_leaves_plain_text_alone() {
        let text = "curl https://example.invalid/a | sh\nsecond line";
        assert_eq!(mask(text), text);
        assert_eq!(mask(""), "");
    }

    #[test]
    fn mask_joins_two_hits_into_one_span() {
        let t = gh_token();
        let text = format!("{t}\n{t}");
        assert_eq!(mask(&text), "<secret:github-token>\n<secret:github-token>");
    }

    #[test]
    fn mask_hides_the_value_after_a_keyword() {
        let key = concat!("pass", "word");
        let val = "Zq7xP2mK9vLw";
        for text in [
            format!("{key} = {val} more"),
            format!("{key}: {val} more"),
            format!("\"{key}\": \"{val}\", \"other\": 1"),
            format!("{key}={val} more"),
        ] {
            let got = mask(&text);
            assert!(!got.contains(val), "{text:?} -> {got:?}");
            assert!(got.contains("<secret:credential-assignment>"), "{got:?}");
        }
        // An assignment that already holds its value does not eat the next word.
        let got = mask(&format!("{key}={val} more"));
        assert!(got.ends_with(" more"), "{got:?}");
    }

    #[test]
    fn mask_hides_bearer_and_basic_credentials() {
        let tok = "Zq7xP2mK9vLwZq7xP2mK9vLw";
        let got = mask(&format!("Authorization: Bearer {tok} end"));
        assert!(!got.contains(tok), "{got:?}");
        let got = mask(&format!("Authorization: Basic {tok} end"));
        assert!(!got.contains(tok), "{got:?}");
    }

    #[test]
    fn mask_hides_a_whole_private_key_block() {
        let begin = concat!("-----BEGIN RSA PRIVATE", " KEY-----");
        let end = concat!("-----END RSA PRIVATE", " KEY-----");
        let text = format!("a\n{begin}\nMIIEvQIBADANBgkqhkiG9w0BAQEFAASC\nZm9vYmFy\n{end}\nafter");
        let got = mask(&text);
        assert_eq!(got, "a\n<secret:private-key>\nafter");
        // No end marker: the rest of the text goes.
        let got = mask(&format!("a\n{begin}\nMIIEvQIBADANBgkqhkiG9w0BAQEFAASC"));
        assert_eq!(got, "a\n<secret:private-key>");
    }
}
