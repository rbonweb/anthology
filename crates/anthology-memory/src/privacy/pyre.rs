//! A strict translator from the Python `re` patterns of the privacy scan to `regex` or
//! `fancy-regex` syntax, with CPython 3.12's semantics (GC-16).
//!
//! It understands exactly the constructs those patterns use and refuses everything else, so a
//! Python change it does not know is a test failure, never a silent mistranslation:
//! literals and escapes (`\\x22`, `\\r`, `\\n`, `\\t`, `\\f`, `\\v`, escaped punctuation), classes
//! with ranges and `^`, `(?:` `(?=` `(?!` `(?<=` `(?<!`, capture groups, `|`, greedy `* + ?
//! {m} {m,} {m,n} {,n}`, `^`, `$`, `.`, `\\b`, `\\B` and the classes `\\w \\W \\s \\S \\d \\D`.
//!
//! - `\\w`, `\\s` and `\\d` are CPython's ([`super::pyclass`]), not Rust's Unicode classes.
//! - `\\b` is CPython's word boundary over that `\\w`, written as look-arounds. Rust's `\\b`
//!   differs next to marks, `No` digits and `Pc` (D-3), and here that would lose findings.
//! - `$` (no MULTILINE) is the end, or just before a final `\\n`: `(?=\\n?\\z)`.
//! - `re.IGNORECASE` never uses the engines' `(?i)`. CPython (`_compiler._compile` and
//!   `_optimize_charset`) lowers each cased member `x` of a literal or class with
//!   `_sre.unicode_tolower`, adds `_casefix._EXTRA_CASES` of the result, and matches a character
//!   `c` when `unicode_tolower(c)` is in that set. The translation writes that set out as a
//!   class, from CPython's own tables ([`super::pyclass::LOWER`] and `EXTRA_CASES`), so it is
//!   exact for every character: `i` becomes `[iIİı]`, `k` `[kK]` plus the Kelvin sign, and `ı`
//!   `[iIİı]` too. Uncased
//!   characters match only themselves (no character lowers to one), and the `\\w`/`\\s`/`\\d`
//!   classes are unchanged by lowering (both checked over every code point in the 3.12 image).
//!
//! [`Mode::Prefilter`] writes the same pattern with every assertion left out (look-arounds,
//! `\\b`, `$`, `^`). Leaving an assertion out only widens what a pattern matches, so when that
//! pattern (plain `regex`, linear, no limits) finds nothing on a line, the exact pattern cannot
//! either, and the scan skips it.

use std::collections::BTreeSet;

use super::pyclass;

/// A translated pattern, and whether it needs `fancy-regex` (a look-around).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Translated {
    pub pattern: String,
    pub fancy: bool,
}

/// What to translate.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// The pattern, exactly.
    Exact,
    /// A pattern that matches wherever the exact one does: every assertion left out.
    Prefilter,
}

/// Translates the Python pattern `py`, compiled with `re.IGNORECASE` when `ignorecase`.
pub fn translate(py: &str, ignorecase: bool) -> Result<Translated, String> {
    translate_mode(py, ignorecase, Mode::Exact)
}

pub fn translate_mode(py: &str, ignorecase: bool, mode: Mode) -> Result<Translated, String> {
    let mut t = Translator {
        chars: py.chars().collect(),
        i: 0,
        ignorecase,
        mode,
        out: String::new(),
        fancy: false,
        depth: 0,
    };
    t.run()?;
    Ok(Translated {
        pattern: t.out,
        fancy: t.fancy,
    })
}

/// `s` as a literal (Python `re.escape(s)`), compiled with IGNORECASE when `ignorecase`.
pub fn literal(s: &str, ignorecase: bool) -> String {
    let mut out = String::new();
    for c in s.chars() {
        push_literal(&mut out, c, ignorecase);
    }
    out
}

struct Translator {
    chars: Vec<char>,
    i: usize,
    ignorecase: bool,
    mode: Mode,
    out: String,
    fancy: bool,
    depth: usize,
}

/// One element of a class.
enum Item {
    Char(char),
    Range(char, char),
    Set(Vec<(u32, u32)>),
}

impl Translator {
    fn peek(&self, k: usize) -> Option<char> {
        self.chars.get(self.i + k).copied()
    }

    fn err(&self, what: &str) -> String {
        let at: String = self.chars.iter().collect();
        format!("pyre: {what} at {} in {at:?}", self.i)
    }

    fn prefilter(&self) -> bool {
        self.mode == Mode::Prefilter
    }

    fn run(&mut self) -> Result<(), String> {
        // Whether the previous element can take a quantifier.
        let mut quantifiable = false;
        while let Some(c) = self.peek(0) {
            match c {
                '\\' => {
                    quantifiable = self.escape()?;
                }
                '[' => {
                    self.i += 1;
                    let class = self.class()?;
                    self.out.push_str(&class);
                    quantifiable = true;
                }
                '(' => {
                    self.group_open()?;
                    quantifiable = false;
                }
                ')' => {
                    if self.depth == 0 {
                        return Err(self.err("unbalanced `)`"));
                    }
                    self.depth -= 1;
                    self.i += 1;
                    self.out.push(')');
                    quantifiable = true;
                }
                '|' => {
                    self.i += 1;
                    self.out.push('|');
                    quantifiable = false;
                }
                '*' | '+' | '?' => {
                    if !quantifiable {
                        return Err(self.err("nothing to repeat"));
                    }
                    self.i += 1;
                    self.out.push(c);
                    if matches!(self.peek(0), Some('?' | '+')) {
                        return Err(self.err("lazy and possessive quantifiers are not supported"));
                    }
                    quantifiable = false;
                }
                '{' => {
                    if let Some((text, len)) = self.repeat_at(self.i) {
                        if !quantifiable {
                            return Err(self.err("nothing to repeat"));
                        }
                        self.i += len;
                        self.out.push_str(&text);
                        if matches!(self.peek(0), Some('?' | '+')) {
                            return Err(
                                self.err("lazy and possessive quantifiers are not supported")
                            );
                        }
                        quantifiable = false;
                    } else {
                        // Python reads a `{` that starts no repeat as a literal.
                        self.i += 1;
                        push_escaped(&mut self.out, '{');
                        quantifiable = true;
                    }
                }
                '^' => {
                    self.i += 1;
                    if !self.prefilter() {
                        self.out.push_str(r"\A");
                    }
                    quantifiable = false;
                }
                '$' => {
                    self.i += 1;
                    if !self.prefilter() {
                        self.out.push_str(r"(?=\n?\z)");
                        self.fancy = true;
                    }
                    quantifiable = false;
                }
                '.' => {
                    self.i += 1;
                    self.out.push_str(r"[^\n]");
                    quantifiable = true;
                }
                _ => {
                    self.i += 1;
                    push_literal(&mut self.out, c, self.ignorecase);
                    quantifiable = true;
                }
            }
        }
        if self.depth != 0 {
            return Err(self.err("unbalanced `(`"));
        }
        Ok(())
    }

    /// A repeat at `at`, as CPython's parser reads one: `{` digits, then `,` digits or nothing,
    /// then `}`, except `{}` (`{,}` is `{0,}`, `{,n}` is `{0,n}`). Its translation and length in
    /// characters, or `None` for a literal `{`.
    fn repeat_at(&self, at: usize) -> Option<(String, usize)> {
        if self.chars.get(at + 1) == Some(&'}') {
            return None;
        }
        let mut k = at + 1;
        let digits = |k: &mut usize| {
            let start = *k;
            while self.chars.get(*k).is_some_and(char::is_ascii_digit) {
                *k += 1;
            }
            self.chars[start..*k].iter().collect::<String>()
        };
        let lo = digits(&mut k);
        let hi = if self.chars.get(k) == Some(&',') {
            k += 1;
            Some(digits(&mut k))
        } else {
            None
        };
        if self.chars.get(k) != Some(&'}') {
            return None;
        }
        let lo = if lo.is_empty() { "0".to_string() } else { lo };
        let text = match hi {
            None => format!("{{{lo}}}"),
            Some(hi) if hi.is_empty() => format!("{{{lo},}}"),
            Some(hi) => format!("{{{lo},{hi}}}"),
        };
        Some((text, k + 1 - at))
    }

    fn group_open(&mut self) -> Result<(), String> {
        self.i += 1;
        if self.peek(0) != Some('?') {
            self.depth += 1;
            self.out.push('(');
            return Ok(());
        }
        let opener = match (self.peek(1), self.peek(2)) {
            (Some(':'), _) => "(?:",
            (Some('='), _) => "(?=",
            (Some('!'), _) => "(?!",
            (Some('<'), Some('=')) => "(?<=",
            (Some('<'), Some('!')) => "(?<!",
            _ => return Err(self.err("unsupported group")),
        };
        self.i += opener.chars().count() - 1;
        if opener == "(?:" {
            self.depth += 1;
            self.out.push_str(opener);
            return Ok(());
        }
        if self.prefilter() {
            // Check the whole assertion as the exact mode would, then leave it out.
            let start = self.i;
            let inner = self.skip_group()?;
            translate_mode(&inner, self.ignorecase, Mode::Exact)
                .map_err(|e| format!("{e} (in the assertion at {start})"))?;
            if matches!(self.peek(0), Some('*' | '+' | '?' | '{')) {
                return Err(self.err("a repeated assertion"));
            }
            return Ok(());
        }
        self.depth += 1;
        self.fancy = true;
        self.out.push_str(opener);
        Ok(())
    }

    /// The source of a group body after its opener, through its closing `)` (consumed).
    fn skip_group(&mut self) -> Result<String, String> {
        let start = self.i;
        let mut depth = 1usize;
        let mut in_class = false;
        while let Some(c) = self.peek(0) {
            match c {
                '\\' => {
                    self.i += 2;
                    continue;
                }
                '[' if !in_class => {
                    in_class = true;
                    // A `]` right after `[` or `[^` is a member.
                    self.i += 1;
                    if self.peek(0) == Some('^') {
                        self.i += 1;
                    }
                    if self.peek(0) == Some(']') {
                        self.i += 1;
                    }
                    continue;
                }
                ']' if in_class => in_class = false,
                '(' if !in_class => depth += 1,
                ')' if !in_class => {
                    depth -= 1;
                    if depth == 0 {
                        let inner: String = self.chars[start..self.i].iter().collect();
                        self.i += 1;
                        return Ok(inner);
                    }
                }
                _ => {}
            }
            self.i += 1;
        }
        Err(self.err("unbalanced `(`"))
    }

    /// An escape outside a class; returns whether it can take a quantifier.
    fn escape(&mut self) -> Result<bool, String> {
        let Some(e) = self.peek(1) else {
            return Err(self.err("trailing backslash"));
        };
        self.i += 2;
        match e {
            'w' => self.out.push_str(&class_text(pyclass::W, false)),
            'W' => self.out.push_str(&class_text(pyclass::W, true)),
            's' => self.out.push_str(&class_text(pyclass::S, false)),
            'S' => self.out.push_str(&class_text(pyclass::S, true)),
            'd' => self.out.push_str(&class_text(pyclass::D, false)),
            'D' => self.out.push_str(&class_text(pyclass::D, true)),
            'b' | 'B' => {
                if !self.prefilter() {
                    let w = class_text(pyclass::W, false);
                    let boundary = if e == 'b' {
                        format!("(?:(?<={w})(?!{w})|(?<!{w})(?={w}))")
                    } else {
                        format!("(?:(?<={w})(?={w})|(?<!{w})(?!{w}))")
                    };
                    self.out.push_str(&boundary);
                    self.fancy = true;
                }
                return Ok(false);
            }
            _ => {
                let c = self.simple_escape(e)?;
                push_literal(&mut self.out, c, self.ignorecase);
            }
        }
        Ok(true)
    }

    /// The character of a one-character escape `\e` (already consumed, as is `\xHH`'s `x`).
    fn simple_escape(&mut self, e: char) -> Result<char, String> {
        Ok(match e {
            'x' => {
                let hex: String = self
                    .chars
                    .get(self.i..self.i + 2)
                    .unwrap_or(&[])
                    .iter()
                    .collect();
                if hex.len() != 2 || !hex.chars().all(|h| h.is_ascii_hexdigit()) {
                    return Err(self.err("bad \\x escape"));
                }
                self.i += 2;
                char::from(u8::from_str_radix(&hex, 16).map_err(|_| self.err("bad \\x"))?)
            }
            'r' => '\r',
            'n' => '\n',
            't' => '\t',
            'f' => '\x0c',
            'v' => '\x0b',
            _ if e.is_ascii_punctuation() || e == ' ' => e,
            _ => return Err(self.err(&format!("unsupported escape \\{e}"))),
        })
    }

    /// A class after its `[`, through its `]`, as one regex-syntax class.
    fn class(&mut self) -> Result<String, String> {
        let negated = self.peek(0) == Some('^');
        if negated {
            self.i += 1;
        }
        let mut items = Vec::new();
        let mut first = true;
        loop {
            let Some(c) = self.peek(0) else {
                return Err(self.err("unterminated class"));
            };
            if c == ']' && !first {
                self.i += 1;
                break;
            }
            first = false;
            let item = self.class_atom()?;
            if let Item::Char(lo) = item
                && self.peek(0) == Some('-')
                && self.peek(1).is_some_and(|n| n != ']')
            {
                self.i += 1;
                let Item::Char(hi) = self.class_atom()? else {
                    return Err(self.err("bad class range"));
                };
                if hi < lo {
                    return Err(self.err("bad class range"));
                }
                items.push(Item::Range(lo, hi));
                continue;
            }
            items.push(item);
        }
        // Explicit members fold under IGNORECASE; the \w/\s/\d sets are case-stable.
        let mut explicit = Vec::new();
        let mut sets = Vec::new();
        for item in items {
            match item {
                Item::Char(c) => explicit.push((c as u32, c as u32)),
                Item::Range(lo, hi) => explicit.push((lo as u32, hi as u32)),
                Item::Set(r) => sets.extend(r),
            }
        }
        let mut members = normalize(explicit);
        if self.ignorecase {
            let size: u32 = members.iter().map(|(lo, hi)| hi - lo + 1).sum();
            if size > 0x2_0000 {
                return Err(self.err("an IGNORECASE class range too large to fold"));
            }
            let points = members.iter().flat_map(|&(lo, hi)| lo..=hi);
            members = fold(points);
        }
        members.extend(sets);
        Ok(class_text(&normalize(members), negated))
    }

    fn class_atom(&mut self) -> Result<Item, String> {
        let c = self.peek(0).ok_or_else(|| self.err("unterminated class"))?;
        if c != '\\' {
            self.i += 1;
            return Ok(Item::Char(c));
        }
        let e = self.peek(1).ok_or_else(|| self.err("trailing backslash"))?;
        self.i += 2;
        Ok(match e {
            'w' => Item::Set(pyclass::W.to_vec()),
            'W' => Item::Set(complement(pyclass::W)),
            's' => Item::Set(pyclass::S.to_vec()),
            'S' => Item::Set(complement(pyclass::S)),
            'd' => Item::Set(pyclass::D.to_vec()),
            'D' => Item::Set(complement(pyclass::D)),
            _ => Item::Char(self.simple_escape(e)?),
        })
    }
}

/// `_sre.unicode_tolower(cp)`.
fn py_lower(cp: u32) -> u32 {
    pyclass::LOWER
        .binary_search_by_key(&cp, |&(k, _)| k)
        .map_or(cp, |i| pyclass::LOWER[i].1)
}

/// `_casefix._EXTRA_CASES[lo]`, or nothing.
fn extra_cases(lo: u32) -> &'static [u32] {
    pyclass::EXTRA_CASES
        .binary_search_by_key(&lo, |&(k, _)| k)
        .map_or(&[], |i| pyclass::EXTRA_CASES[i].1)
}

/// Every character CPython's IGNORECASE matches for the members `points` (a literal or the
/// explicit members of a class): each lowered, with its extra cases, then every character whose
/// lowercase is one of those.
fn fold(points: impl Iterator<Item = u32>) -> Vec<(u32, u32)> {
    let mut lowered: BTreeSet<u32> = BTreeSet::new();
    for cp in points {
        let lo = py_lower(cp);
        lowered.insert(lo);
        lowered.extend(extra_cases(lo));
    }
    let mut out: Vec<(u32, u32)> = lowered
        .iter()
        .filter(|&&x| py_lower(x) == x)
        .map(|&x| (x, x))
        .collect();
    out.extend(
        pyclass::LOWER
            .iter()
            .filter(|(_, lo)| lowered.contains(lo))
            .map(|&(k, _)| (k, k)),
    );
    normalize(out)
}

/// One literal character, folded under IGNORECASE.
fn push_literal(out: &mut String, c: char, ignorecase: bool) {
    if ignorecase {
        let set = fold(std::iter::once(c as u32));
        if set != [(c as u32, c as u32)] {
            out.push_str(&class_text(&set, false));
            return;
        }
    }
    push_escaped(out, c);
}

/// `c` as a literal in either engine's syntax.
fn push_escaped(out: &mut String, c: char) {
    if c.is_alphanumeric() || (!c.is_ascii() && !c.is_control()) {
        out.push(c);
    } else {
        out.push_str(&format!("\\x{{{:x}}}", c as u32));
    }
}

/// Sorted, merged ranges.
fn normalize(mut ranges: Vec<(u32, u32)>) -> Vec<(u32, u32)> {
    ranges.sort_unstable();
    let mut out: Vec<(u32, u32)> = Vec::with_capacity(ranges.len());
    for (lo, hi) in ranges {
        match out.last_mut() {
            Some(last) if lo <= last.1.saturating_add(1) => last.1 = last.1.max(hi),
            _ => out.push((lo, hi)),
        }
    }
    out
}

/// The scalar values (no surrogates) outside `ranges` (sorted, merged).
fn complement(ranges: &[(u32, u32)]) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    let mut next = 0u32;
    for &(lo, hi) in ranges {
        if lo > next {
            out.push((next, lo - 1));
        }
        next = hi + 1;
    }
    if next <= 0x10_ffff {
        out.push((next, 0x10_ffff));
    }
    // Cut the surrogate block out: neither engine accepts it in a class.
    let mut cut = Vec::with_capacity(out.len() + 1);
    for (lo, hi) in out {
        if hi < 0xd800 || lo > 0xdfff {
            cut.push((lo, hi));
            continue;
        }
        if lo < 0xd800 {
            cut.push((lo, 0xd7ff));
        }
        if hi > 0xdfff {
            cut.push((0xe000, hi));
        }
    }
    cut
}

/// `ranges` as one class, `[...]` or `[^...]`.
fn class_text(ranges: &[(u32, u32)], negated: bool) -> String {
    let mut s = String::with_capacity(ranges.len() * 16 + 3);
    s.push('[');
    if negated {
        s.push('^');
    }
    for &(lo, hi) in ranges {
        s.push_str(&format!("\\x{{{lo:x}}}"));
        if hi != lo {
            s.push_str(&format!("-\\x{{{hi:x}}}"));
        }
    }
    s.push(']');
    s
}

/// The characters [`literal`] can fold into or out of (every lowercase mapping's two ends and
/// every extra case, plus ASCII letters): the candidates of the difftest's `fold` check.
pub fn case_candidates() -> Vec<u32> {
    let mut all: BTreeSet<u32> = (u32::from(b'A')..=u32::from(b'Z'))
        .chain(u32::from(b'a')..=u32::from(b'z'))
        .collect();
    for &(k, v) in pyclass::LOWER {
        all.insert(k);
        all.insert(v);
    }
    for &(k, vs) in pyclass::EXTRA_CASES {
        all.insert(k);
        all.extend(vs);
    }
    all.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fancy(py: &str, ic: bool) -> fancy_regex::Regex {
        let t = translate(py, ic).unwrap();
        fancy_regex::Regex::new(&t.pattern).unwrap()
    }

    #[test]
    fn classes_are_pythons() {
        let w = fancy(r"^\w$", false);
        for c in ['a', '_', '7', '²', 'é', '一'] {
            assert!(w.is_match(&c.to_string()).unwrap(), "{c:?}");
        }
        // A combining mark, a connector and a joiner are \w in Rust, not in Python.
        for c in ['\u{301}', '\u{203f}', '\u{200d}', '-', ' '] {
            assert!(!w.is_match(&c.to_string()).unwrap(), "{c:?}");
        }
        let s = fancy(r"^\s$", false);
        for c in ['\x1c', '\x1f', '\u{85}', '\u{a0}', '\u{3000}'] {
            assert!(s.is_match(&c.to_string()).unwrap(), "{c:?}");
        }
        assert!(!s.is_match("\u{200b}").unwrap());
        let d = fancy(r"^\d$", false);
        assert!(d.is_match("\u{663}").unwrap());
        assert!(!d.is_match("²").unwrap());
    }

    /// The sets CPython 3.12 matches, checked over every code point in the pinned image.
    #[test]
    fn ignorecase_is_cpythons() {
        let set = |c: char| -> String {
            let v = fold(std::iter::once(c as u32));
            v.iter()
                .flat_map(|&(lo, hi)| lo..=hi)
                .filter_map(char::from_u32)
                .collect()
        };
        assert_eq!(set('i'), "Iiİı");
        assert_eq!(set('\u{131}'), "Iiİı");
        assert_eq!(set('\u{130}'), "Iiİı");
        assert_eq!(set('s'), "Ssſ");
        assert_eq!(set('k'), "Kk\u{212a}");
        assert_eq!(set('a'), "Aa");
        assert_eq!(set('-'), "-");
        assert_eq!(set('é'), "Éé");
        let k = fancy("^[a-z]$", true);
        for c in ["K", "\u{212a}", "\u{17f}", "\u{131}", "\u{130}"] {
            assert!(k.is_match(c).unwrap(), "{c:?}");
        }
        assert!(!k.is_match("é").unwrap());
        assert!(fancy("^é$", true).is_match("É").unwrap());
        let host = regex::Regex::new(&format!("^{}$", literal("k\u{131}wi", true))).unwrap();
        for text in ["kiwi", "KIWI", "kİwi", "\u{212a}\u{131}wi"] {
            assert!(host.is_match(text), "{text}");
        }
    }

    #[test]
    fn boundary_and_end_are_pythons() {
        let b = fancy(r"\bgh", false);
        assert!(
            b.is_match("\u{301}gh").unwrap(),
            "a mark is not \\w in Python"
        );
        assert!(!b.is_match("²gh").unwrap(), "² is \\w in Python");
        let end = fancy("a$", false);
        assert!(end.is_match("a").unwrap());
        assert!(end.is_match("a\n").unwrap());
        assert!(!end.is_match("a\n\n").unwrap());
    }

    #[test]
    fn unknown_constructs_are_refused() {
        for py in [
            r"(?P<x>a)",
            r"(?i)a",
            r"a*?",
            r"\1",
            r"\A",
            r"\Z",
            "(a",
            "a)",
            "[a",
            r"\q",
        ] {
            assert!(translate(py, false).is_err(), "{py}");
        }
        assert!(translate_mode(r"(?!x)*", false, Mode::Prefilter).is_err());
        assert!(translate_mode(r"a(?!\q)", false, Mode::Prefilter).is_err());
    }

    #[test]
    fn braces_and_ranges() {
        assert_eq!(translate("a{2,}", false).unwrap().pattern, "a{2,}");
        assert_eq!(translate("a{,3}", false).unwrap().pattern, "a{0,3}");
        assert_eq!(translate("a{,}", false).unwrap().pattern, "a{0,}");
        let lit = fancy("^a{x}$", false);
        assert!(lit.is_match("a{x}").unwrap());
        assert!(fancy("^a{}$", false).is_match("a{}").unwrap());
        let class = fancy(r"^[\w.-]+$", false);
        assert!(class.is_match("a.b-c").unwrap());
        assert!(!class.is_match("a b").unwrap());
        let neg = fancy(r"^[^\s/:@]+$", false);
        assert!(neg.is_match("abc").unwrap());
        assert!(!neg.is_match("a:b").unwrap());
    }

    #[test]
    fn a_prefilter_drops_only_assertions() {
        let pre = |py: &str| translate_mode(py, false, Mode::Prefilter).unwrap();
        let t = pre(r"(?<![\w-])id_(?:rsa|dsa)(?![\w-])$");
        assert!(!t.fancy);
        let rx = regex::Regex::new(&t.pattern).unwrap();
        assert!(rx.is_match("xid_rsa-"));
        assert!(!rx.is_match("id_ecdsa"));
        let t = pre(r"\bgh[pousr]_(?=[)\]])x");
        assert!(regex::Regex::new(&t.pattern).unwrap().is_match("_ghp_x"));
        // A `)` or `]` inside a class or an escape does not end the skipped assertion.
        let t = pre(r"a(?![)\]](?:b|\))c)d");
        assert_eq!(
            regex::Regex::new(&t.pattern)
                .unwrap()
                .find("ad")
                .map(|m| m.as_str()),
            Some("ad")
        );
    }
}
