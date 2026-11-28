//! The string-to-regex translation and the literal prefilter (Task 5).
//!
//! Every YARA string becomes one `regex::bytes::Regex`. Next to it goes a set of literal "atoms":
//! bytes that any match must contain. One Aho-Corasick automaton over the atoms of all strings
//! tells, in one pass over a file, which strings can match at all; only those run their regex.

use regex::bytes::{Regex, RegexBuilder};
use regex_syntax::hir::literal::{ExtractKind, Extractor};
use regex_syntax::hir::{Hir, HirKind};

use super::parse::{Body, Hex, Str};

/// The shortest literal worth a place in the prefilter.
const MIN_ATOM: usize = 3;
/// The most alternative literals one string may contribute.
const MAX_ATOMS: usize = 64;
/// A compiled regex above this many bytes is refused (a hex gap such as `[1-999999]`).
const SIZE_LIMIT: usize = 2 << 20;

/// A string ready to match.
pub(super) struct Built {
    pub re: Regex,
    /// Lowercase literals; a match contains at least one of them. Empty means "always run".
    pub atoms: Vec<Vec<u8>>,
}

pub(super) fn build(s: &Str) -> Result<Built, String> {
    let (pat, nocase, dotall, can_fall_back) = match &s.body {
        Body::Text(bytes) => (text_pattern(bytes, s.wide, s.ascii), s.nocase, false, false),
        Body::Regex {
            pat,
            nocase,
            dotall,
        } => {
            if s.wide {
                return Err("the `wide` modifier on a regex string is not supported".to_string());
            }
            (pat.clone(), *nocase || s.nocase, *dotall, !pat.is_ascii())
        }
        Body::Hex(seq) => {
            if s.nocase || s.wide || s.ascii || s.fullword {
                return Err("a hex string takes no modifiers".to_string());
            }
            (hex_pattern(seq), false, true, false)
        }
    };
    match compile(&pat, nocase, dotall, false) {
        Ok(re) => {
            let atoms = match (&s.body, s.wide) {
                (Body::Text(bytes), false) => text_atoms(bytes),
                (Body::Text(_), true) => Vec::new(),
                _ => regex_atoms(&pat, dotall),
            };
            Ok(Built { re, atoms })
        }
        // A pattern with non-ASCII text needs Unicode mode; its atoms stay off, as case folding
        // differs there.
        Err(_) if can_fall_back => match compile(&pat, nocase, dotall, true) {
            Ok(re) => Ok(Built {
                re,
                atoms: Vec::new(),
            }),
            Err(e) => Err(reason(&e)),
        },
        Err(first) => Err(reason(&first)),
    }
}

fn reason(e: &regex::Error) -> String {
    let text = e.to_string();
    let line = text
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("invalid pattern");
    format!("regex: {}", line.trim())
}

fn compile(pat: &str, nocase: bool, dotall: bool, unicode: bool) -> Result<Regex, regex::Error> {
    RegexBuilder::new(pat)
        .unicode(unicode)
        .case_insensitive(nocase)
        .dot_matches_new_line(dotall)
        .size_limit(SIZE_LIMIT)
        .build()
}

/// `bytes` as a regex that matches exactly them; `wide` is the same with a NUL after each byte.
fn text_pattern(bytes: &[u8], wide: bool, ascii: bool) -> String {
    let lit = |wide: bool| -> String {
        let mut out = String::new();
        for &b in bytes {
            if b.is_ascii_alphanumeric() {
                out.push(b as char);
            } else {
                out.push_str(&format!("\\x{b:02X}"));
            }
            if wide {
                out.push_str("\\x00");
            }
        }
        out
    };
    match (wide, ascii) {
        (false, _) => lit(false),
        (true, false) => lit(true),
        (true, true) => format!("(?:{}|{})", lit(false), lit(true)),
    }
}

fn hex_pattern(seq: &[Hex]) -> String {
    let mut out = String::new();
    for h in seq {
        match h {
            Hex::Byte(b) => out.push_str(&format!("\\x{b:02X}")),
            Hex::Any => out.push('.'),
            Hex::Gap(lo, hi) if lo == hi => out.push_str(&format!(".{{{lo}}}")),
            Hex::Gap(lo, hi) => out.push_str(&format!(".{{{lo},{hi}}}")),
            Hex::Alt(alts) => {
                out.push_str("(?:");
                for (n, alt) in alts.iter().enumerate() {
                    if n > 0 {
                        out.push('|');
                    }
                    out.push_str(&hex_pattern(alt));
                }
                out.push(')');
            }
        }
    }
    out
}

fn text_atoms(bytes: &[u8]) -> Vec<Vec<u8>> {
    if bytes.len() >= MIN_ATOM {
        vec![bytes.to_ascii_lowercase()]
    } else {
        Vec::new()
    }
}

/// The atoms of a regex pattern, taken from its syntax tree parsed without case folding: the
/// automaton ignores ASCII case, so a lowercase atom is right for both.
fn regex_atoms(pat: &str, dotall: bool) -> Vec<Vec<u8>> {
    let hir = regex_syntax::ParserBuilder::new()
        .unicode(false)
        .utf8(false)
        .dot_matches_new_line(dotall)
        .build()
        .parse(pat);
    let Ok(hir) = hir else {
        return Vec::new();
    };
    let mut extractor = Extractor::new();
    extractor.kind(ExtractKind::Prefix);
    let seq = extractor.extract(&hir);
    if let Some(lits) = seq.literals()
        && !lits.is_empty()
        && lits.len() <= MAX_ATOMS
        && lits.iter().all(|l| l.as_bytes().len() >= MIN_ATOM)
    {
        let mut atoms: Vec<Vec<u8>> = lits
            .iter()
            .map(|l| l.as_bytes().to_ascii_lowercase())
            .collect();
        atoms.sort();
        atoms.dedup();
        return atoms;
    }
    // No literal prefix: the longest literal that every match must contain.
    match required_literal(&hir) {
        Some(lit) if lit.len() >= MIN_ATOM => vec![lit.to_ascii_lowercase()],
        _ => Vec::new(),
    }
}

/// The longest literal that sits on every path through `hir`.
fn required_literal(hir: &Hir) -> Option<Vec<u8>> {
    match hir.kind() {
        HirKind::Literal(l) => Some(l.0.to_vec()),
        HirKind::Capture(c) => required_literal(&c.sub),
        HirKind::Repetition(r) if r.min >= 1 => required_literal(&r.sub),
        HirKind::Concat(parts) => parts
            .iter()
            .filter_map(required_literal)
            .max_by_key(Vec::len),
        _ => None,
    }
}
