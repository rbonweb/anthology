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
//! - `re.IGNORECASE` never uses the engines' `(?i)`: CPython matches a cased literal `x`
//!   against `c.lower()` and its fixed equivalences (`i`/`ı`, `s`/`ſ`), so an ASCII letter
//!   becomes `[xX]`, `i` becomes `[iIİı]`, `s` `[sSſ]` and `k` `[kKK]` (checked over every code
//!   point in the 3.12 image). Uncased characters and the classes above match the same under
//!   it. A non-ASCII cased literal in a static pattern is refused.

use super::pyclass;

/// A translated pattern, and whether it needs `fancy-regex` (a look-around).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Translated {
    pub pattern: String,
    pub fancy: bool,
}

/// How a non-ASCII cased literal is handled under IGNORECASE.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NonAsciiCase {
    /// A translation error (the static patterns are ASCII).
    Refuse,
    /// The engine's own `(?i:c)` (the machine's hostname, which is data, not code).
    EngineFold,
}

/// Translates the Python pattern `py`, compiled with `re.IGNORECASE` when `ignorecase`.
pub fn translate(py: &str, ignorecase: bool) -> Result<Translated, String> {
    let mut t = Translator {
        chars: py.chars().collect(),
        i: 0,
        ignorecase,
        non_ascii: NonAsciiCase::Refuse,
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
pub fn literal(s: &str, ignorecase: bool, non_ascii: NonAsciiCase) -> Result<String, String> {
    let mut out = String::new();
    for c in s.chars() {
        push_literal(&mut out, c, ignorecase, non_ascii)?;
    }
    Ok(out)
}

struct Translator {
    chars: Vec<char>,
    i: usize,
    ignorecase: bool,
    non_ascii: NonAsciiCase,
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
                    self.out.push_str(r"\A");
                    quantifiable = false;
                }
                '$' => {
                    self.i += 1;
                    self.out.push_str(r"(?=\n?\z)");
                    self.fancy = true;
                    quantifiable = false;
                }
                '.' => {
                    self.i += 1;
                    self.out.push_str(r"[^\n]");
                    quantifiable = true;
                }
                _ => {
                    self.i += 1;
                    push_literal(&mut self.out, c, self.ignorecase, self.non_ascii)
                        .map_err(|e| self.err(&e))?;
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
        self.depth += 1;
        if self.peek(0) != Some('?') {
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
        if opener != "(?:" {
            self.fancy = true;
        }
        self.i += opener.chars().count() - 1;
        self.out.push_str(opener);
        Ok(())
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
                let w = class_text(pyclass::W, false);
                let boundary = if e == 'b' {
                    format!("(?:(?<={w})(?!{w})|(?<!{w})(?={w}))")
                } else {
                    format!("(?:(?<={w})(?={w})|(?<!{w})(?!{w}))")
                };
                self.out.push_str(&boundary);
                self.fancy = true;
                return Ok(false);
            }
            _ => {
                let c = self.simple_escape(e)?;
                push_literal(&mut self.out, c, self.ignorecase, self.non_ascii)
                    .map_err(|m| self.err(&m))?;
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
        let mut explicit = normalize(explicit);
        if self.ignorecase {
            explicit = self.fold_ranges(explicit)?;
        }
        explicit.extend(sets);
        Ok(class_text(&normalize(explicit), negated))
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

    /// Explicit class members under IGNORECASE: every ASCII letter brings its other case and
    /// its fixed equivalents; a non-ASCII cased member is refused (it would need CPython's
    /// case tables, and no static pattern has one).
    fn fold_ranges(&self, ranges: Vec<(u32, u32)>) -> Result<Vec<(u32, u32)>, String> {
        let mut out = ranges.clone();
        for &(lo, hi) in &ranges {
            for cp in lo..=hi.min(0x7f) {
                let c = char::from(cp as u8);
                if c.is_ascii_alphabetic() {
                    let folded = fold_chars(c.to_ascii_lowercase()).chars();
                    out.extend(folded.map(|f| (f as u32, f as u32)));
                }
            }
            if hi > 0x7f && (lo.max(0x80)..=hi).filter_map(char::from_u32).any(is_cased) {
                return Err(self.err("a non-ASCII cased class member under IGNORECASE"));
            }
        }
        Ok(normalize(out))
    }
}

/// CPython's `_sre.unicode_iscased`.
fn is_cased(c: char) -> bool {
    !c.to_lowercase().eq(std::iter::once(c)) || !c.to_uppercase().eq(std::iter::once(c))
}

/// Every character an ASCII lowercase letter `x` matches under IGNORECASE (CPython 3.12).
fn fold_chars(x: char) -> &'static str {
    const ALL: [&str; 26] = [
        "aA",
        "bB",
        "cC",
        "dD",
        "eE",
        "fF",
        "gG",
        "hH",
        "iI\u{130}\u{131}",
        "jJ",
        "kK\u{212a}",
        "lL",
        "mM",
        "nN",
        "oO",
        "pP",
        "qQ",
        "rR",
        "sS\u{17f}",
        "tT",
        "uU",
        "vV",
        "wW",
        "xX",
        "yY",
        "zZ",
    ];
    ALL[(x as u8 - b'a') as usize]
}

/// One literal character, folded under IGNORECASE.
fn push_literal(
    out: &mut String,
    c: char,
    ignorecase: bool,
    non_ascii: NonAsciiCase,
) -> Result<(), String> {
    if ignorecase {
        // CPython folds by `lower()`: İ and the Kelvin sign lower into ASCII letters.
        let lo = c.to_lowercase().next().unwrap_or(c);
        if lo.is_ascii_alphabetic() {
            let set: Vec<(u32, u32)> = fold_chars(lo)
                .chars()
                .map(|f| (f as u32, f as u32))
                .collect();
            out.push_str(&class_text(&normalize(set), false));
            return Ok(());
        }
        if !c.is_ascii() && is_cased(c) {
            return match non_ascii {
                NonAsciiCase::Refuse => {
                    Err(format!("a non-ASCII cased literal {c:?} under IGNORECASE"))
                }
                NonAsciiCase::EngineFold => {
                    out.push_str("(?i:");
                    push_escaped(out, c);
                    out.push(')');
                    Ok(())
                }
            };
        }
    }
    push_escaped(out, c);
    Ok(())
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

    #[test]
    fn ignorecase_is_cpythons() {
        let i = fancy("^i$", true);
        for c in ["i", "I", "\u{130}", "\u{131}"] {
            assert!(i.is_match(c).unwrap(), "{c:?}");
        }
        let k = fancy("^[a-z]$", true);
        for c in ["K", "\u{212a}", "\u{17f}", "\u{131}", "\u{130}"] {
            assert!(k.is_match(c).unwrap(), "{c:?}");
        }
        assert!(!k.is_match("é").unwrap());
        assert!(translate("é", true).is_err());
        assert!(translate("é", false).is_ok());
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
}
