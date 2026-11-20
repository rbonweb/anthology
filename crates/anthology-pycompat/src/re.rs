//! `pycompat::re`: Python `re` patterns on the `regex` and `fancy-regex` engines (design §11.1,
//! §12; plan §3.2, GC-16).
//!
//! [`translate`] rewrites a Python (3.12) str pattern into the syntax both Rust engines share:
//!
//! - `\w \W \s \S \d \D`, in and out of classes, become the classes CPython's str patterns use
//!   ([`W`], [`S`], [`D`], generated from CPython's tables), or the ASCII ones under `(?a)`;
//! - `\Z` becomes `\z`; Python's `$` outside MULTILINE ("at the end, or before a final
//!   newline") becomes the look-ahead `(?=\n?\z)`, which needs fancy-regex;
//! - `(?P=name)` becomes `\k<name>`; numbered back-references, look-arounds and atomic groups
//!   pass through (fancy-regex);
//! - every literal is written as `\x{…}`, so no Python literal (`{`, `[` in a class, `&&`, `~~`,
//!   `--`, `\<`) can turn into Rust syntax; a `{` that is not a valid repeat is a literal, as in
//!   Python; `{,n}` is `{0,n}`;
//! - inline flags `i m s` pass through, `x` (verbose) and `a` (ASCII) are applied here, `u` is a
//!   no-op, and `L` is refused (a str pattern), as Python refuses it.
//!
//! Anything Python reads differently from both engines, or that this does not support exactly
//! (`\N{…}`, conditionals `(?(1)…)`, possessive repeats, a lone surrogate), is an `Err`, never a
//! silent difference. `\b` and `\B` keep Rust's Unicode meaning (D-3).
//!
//! [`compile`] picks `regex` when the translation needs no look-around, back-reference or
//! atomic group, and `fancy-regex` (with its default backtrack limit) otherwise. Spans are byte
//! offsets into the haystack. [`Regex::find_iter`] walks matches as Python's `finditer` does
//! (an empty match right after a non-empty one is reported, e.g. `x*` on `abxd` gives
//! `(0,0) (1,1) (2,3) (3,3) (4,4)`), with one limit: after an empty match, Python may still
//! return a lower-priority non-empty match at the same position (`a??` on `a`); this moves on
//! one character instead.

use crate::tables;

/// A str pattern's `\w`: what `c.isalnum() or c == "_"` accepts (CPython 3.12, Unicode 15.0).
pub const W: &str = tables::WORD_CLASS;
/// A str pattern's `\s`: what `c.isspace()` accepts.
pub const S: &str = tables::SPACE_CLASS;
/// A str pattern's `\d`: what `c.isdecimal()` accepts.
pub const D: &str = tables::DECIMAL_CLASS;

/// The backtrack limit of a fancy-regex match was hit (D-7): the caller fails closed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LimitError(pub String);

impl std::fmt::Display for LimitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "regex match failed: {}", self.0)
    }
}

impl std::error::Error for LimitError {}

/// Python's compile flags (`re.I`, `re.M`, `re.S`, `re.X`, `re.A`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Flags {
    pub ignorecase: bool,
    pub multiline: bool,
    pub dotall: bool,
    pub verbose: bool,
    pub ascii: bool,
}

/// A translated pattern: the Rust syntax and whether it needs fancy-regex.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Translated {
    pub pattern: String,
    pub fancy: bool,
}

/// A compiled pattern.
#[derive(Clone, Debug)]
pub enum Regex {
    Plain(regex::Regex),
    Fancy(fancy_regex::Regex),
}

/// `translate_with(py, Flags::default())`'s pattern: Python syntax to fancy-regex syntax.
pub fn translate(py: &str) -> Result<String, String> {
    translate_with(py, Flags::default()).map(|t| t.pattern)
}

/// `re.compile(py, re.I if ignorecase else 0)`.
pub fn compile(py: &str, ignorecase: bool) -> Result<Regex, String> {
    compile_with(
        py,
        Flags {
            ignorecase,
            ..Flags::default()
        },
    )
}

/// `re.compile(py, flags)`.
pub fn compile_with(py: &str, flags: Flags) -> Result<Regex, String> {
    let t = translate_with(py, flags)?;
    if t.fancy {
        fancy_regex::Regex::new(&t.pattern)
            .map(Regex::Fancy)
            .map_err(|e| e.to_string())
    } else {
        regex::Regex::new(&t.pattern)
            .map(Regex::Plain)
            .map_err(|e| e.to_string())
    }
}

/// One match's groups: `[0]` is the whole match, `None` a group that did not take part.
pub type Groups = Vec<Option<(usize, usize)>>;

impl Regex {
    /// Is it fancy-regex?
    pub fn is_fancy(&self) -> bool {
        matches!(self, Regex::Fancy(_))
    }

    /// `bool(p.search(s))`.
    pub fn is_match(&self, s: &str) -> Result<bool, LimitError> {
        match self {
            Regex::Plain(r) => Ok(r.is_match(s)),
            Regex::Fancy(r) => r.is_match(s).map_err(limit),
        }
    }

    /// `p.search(s, pos)`'s span: the leftmost match starting at or after byte `pos`, with the
    /// whole string as context (look-behind, `\b`, `^`).
    pub fn find_at(&self, s: &str, pos: usize) -> Result<Option<(usize, usize)>, LimitError> {
        match self {
            Regex::Plain(r) => Ok(r.find_at(s, pos).map(|m| (m.start(), m.end()))),
            Regex::Fancy(r) => r
                .find_from_pos(s, pos)
                .map(|m| m.map(|m| (m.start(), m.end())))
                .map_err(limit),
        }
    }

    /// `p.search(s, pos)`'s groups.
    pub fn captures_at(&self, s: &str, pos: usize) -> Result<Option<Groups>, LimitError> {
        match self {
            Regex::Plain(r) => Ok(r
                .captures_at(s, pos)
                .map(|c| c.iter().map(|g| g.map(|m| (m.start(), m.end()))).collect())),
            Regex::Fancy(r) => r
                .captures_from_pos(s, pos)
                .map(|c| c.map(|c| c.iter().map(|g| g.map(|m| (m.start(), m.end()))).collect()))
                .map_err(limit),
        }
    }

    /// The number of groups, the whole match included (`p.groups + 1`).
    pub fn captures_len(&self) -> usize {
        match self {
            Regex::Plain(r) => r.captures_len(),
            Regex::Fancy(r) => r.captures_len(),
        }
    }

    /// `[m.span() for m in p.finditer(s)]`, in bytes (module docs).
    pub fn find_iter<'a>(
        &'a self,
        s: &'a str,
    ) -> impl Iterator<Item = Result<(usize, usize), LimitError>> + 'a {
        let mut walk = Walk::default();
        std::iter::from_fn(move || {
            walk.next(s, |pos| self.find_at(s, pos).map(|m| m.map(|m| (m, m))))
                .map(|r| r.map(|(span, _)| span))
        })
    }

    /// `[m.regs for m in p.finditer(s)]`, in bytes.
    pub fn captures_iter<'a>(
        &'a self,
        s: &'a str,
    ) -> impl Iterator<Item = Result<Groups, LimitError>> + 'a {
        let mut walk = Walk::default();
        std::iter::from_fn(move || {
            walk.next(s, |pos| {
                self.captures_at(s, pos)
                    .map(|c| c.map(|g| (g[0].unwrap_or((pos, pos)), g)))
            })
            .map(|r| r.map(|(_, g)| g))
        })
    }
}

fn limit(e: fancy_regex::Error) -> LimitError {
    LimitError(e.to_string())
}

/// One search result: the whole match's span and what the caller wants of it.
type Hit<T> = Result<((usize, usize), T), LimitError>;

/// Python's `finditer` walk (`_sre` scanner): after an empty match the next one must not be
/// empty at the same position.
#[derive(Default)]
struct Walk {
    pos: usize,
    must_advance: bool,
    done: bool,
}

impl Walk {
    fn next<T>(
        &mut self,
        s: &str,
        mut search: impl FnMut(usize) -> Result<Option<((usize, usize), T)>, LimitError>,
    ) -> Option<Hit<T>> {
        if self.done || self.pos > s.len() {
            return None;
        }
        let mut found = match search(self.pos) {
            Ok(f) => f,
            Err(e) => {
                self.done = true;
                return Some(Err(e));
            }
        };
        if self.must_advance && matches!(found, Some(((a, b), _)) if a == self.pos && b == a) {
            let next = match s[self.pos..].chars().next() {
                Some(c) => self.pos + c.len_utf8(),
                None => {
                    self.done = true;
                    return None;
                }
            };
            found = match search(next) {
                Ok(f) => f,
                Err(e) => {
                    self.done = true;
                    return Some(Err(e));
                }
            };
        }
        match found {
            None => {
                self.done = true;
                None
            }
            Some(((a, b), t)) => {
                self.must_advance = a == b;
                self.pos = b;
                Some(Ok(((a, b), t)))
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Translation: a port of the parts of CPython 3.12's `re._parser` that decide meaning.

const ASCII_WORD: &str = "0-9A-Za-z_";
const ASCII_SPACE: &str = "\\x{9}-\\x{d}\\x{20}";
const ASCII_DIGIT: &str = "0-9";

#[derive(Clone, Copy)]
struct Scope {
    multiline: bool,
    verbose: bool,
    ascii: bool,
}

struct Parser<'a> {
    src: &'a [char],
    i: usize,
    out: String,
    fancy: bool,
    scopes: Vec<Scope>,
    /// Group numbers opened so far, and whether each is closed (index = number - 1).
    groups: Vec<bool>,
    /// Open groups: `Some(number)` for a capture, `None` otherwise.
    open: Vec<Option<usize>>,
    /// Whether the last thing written can take a repeat ("nothing to repeat" otherwise).
    can_repeat: bool,
    /// Only global flags written so far.
    at_start: bool,
}

/// [`translate`] with Python's compile flags, saying whether fancy-regex is needed.
pub fn translate_with(py: &str, flags: Flags) -> Result<Translated, String> {
    let src: Vec<char> = py.chars().collect();
    let mut p = Parser {
        src: &src,
        i: 0,
        out: String::new(),
        fancy: false,
        scopes: vec![Scope {
            multiline: flags.multiline,
            verbose: flags.verbose,
            ascii: flags.ascii,
        }],
        groups: Vec::new(),
        open: Vec::new(),
        can_repeat: false,
        at_start: true,
    };
    let mut prefix = String::new();
    if flags.ignorecase {
        prefix.push('i');
    }
    if flags.multiline {
        prefix.push('m');
    }
    if flags.dotall {
        prefix.push('s');
    }
    if !prefix.is_empty() {
        p.out = format!("(?{prefix})");
    }
    p.parse()?;
    Ok(Translated {
        pattern: p.out,
        fancy: p.fancy,
    })
}

fn hex_lit(c: char) -> String {
    format!("\\x{{{:x}}}", u32::from(c))
}

impl Parser<'_> {
    fn scope(&self) -> Scope {
        *self.scopes.last().unwrap_or(&Scope {
            multiline: false,
            verbose: false,
            ascii: false,
        })
    }

    fn peek(&self) -> Option<char> {
        self.src.get(self.i).copied()
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn err<T>(&self, msg: &str) -> Result<T, String> {
        Err(format!("{msg} at position {}", self.i))
    }

    /// Starts an atom: remembers where it begins for a following repeat.
    fn atom(&mut self, text: &str) {
        self.at_start = false;
        self.can_repeat = true;
        self.out.push_str(text);
    }

    /// A zero-width assertion: it cannot be repeated.
    fn assertion(&mut self, text: &str) {
        self.at_start = false;
        self.can_repeat = false;
        self.out.push_str(text);
    }

    fn parse(&mut self) -> Result<(), String> {
        while let Some(c) = self.peek() {
            self.i += 1;
            if self.scope().verbose {
                if matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c') {
                    continue;
                }
                if c == '#' {
                    while let Some(d) = self.peek() {
                        self.i += 1;
                        if d == '\n' {
                            break;
                        }
                    }
                    continue;
                }
            }
            match c {
                '|' => {
                    self.at_start = false;
                    self.can_repeat = false;
                    self.out.push('|');
                }
                '(' => self.group()?,
                ')' => {
                    let Some(g) = self.open.pop() else {
                        return self.err("unbalanced parenthesis");
                    };
                    if let Some(n) = g {
                        self.groups[n - 1] = true;
                    }
                    self.scopes.pop();
                    // The group as a whole is the atom a repeat applies to.
                    self.out.push(')');
                    self.can_repeat = true;
                }
                '*' | '+' | '?' => {
                    self.repeat_check()?;
                    self.out.push(c);
                    self.repeat_suffix()?;
                }
                '{' => self.brace()?,
                '[' => self.class()?,
                '.' => self.atom("."),
                '^' => self.assertion("^"),
                '$' => {
                    if self.scope().multiline {
                        self.assertion("$");
                    } else {
                        self.fancy = true;
                        self.assertion("(?=\\x{a}?\\z)");
                    }
                }
                '\\' => self.escape()?,
                c if c.is_ascii_alphanumeric() => self.atom(&c.to_string()),
                c => {
                    if (0xD800..=0xDFFF).contains(&u32::from(c)) {
                        return self.err("a lone surrogate");
                    }
                    self.atom(&hex_lit(c));
                }
            }
        }
        if !self.open.is_empty() {
            return self.err("missing ), unterminated subpattern");
        }
        Ok(())
    }

    fn repeat_check(&self) -> Result<(), String> {
        if !self.can_repeat {
            return self.err("nothing to repeat");
        }
        Ok(())
    }

    /// After a repeat: `?` makes it lazy; `+` (possessive) is refused.
    fn repeat_suffix(&mut self) -> Result<(), String> {
        if self.eat('?') {
            self.out.push('?');
        } else if self.peek() == Some('+') {
            return self.err("possessive repeats are not supported");
        }
        // A repeat cannot itself be repeated (Python: "multiple repeat").
        self.can_repeat = false;
        if matches!(self.peek(), Some('*' | '+' | '?')) {
            return self.err("multiple repeat");
        }
        Ok(())
    }

    /// `{`: a repeat if it reads as one (`{m}`, `{m,}`, `{,n}`, `{m,n}`, `{,}`), else a literal.
    fn brace(&mut self) -> Result<(), String> {
        let here = self.i;
        if self.peek() == Some('}') {
            self.atom(&hex_lit('{'));
            return Ok(());
        }
        let digits = |p: &mut Self| {
            let mut s = String::new();
            while let Some(d) = p.peek().filter(char::is_ascii_digit) {
                s.push(d);
                p.i += 1;
            }
            s
        };
        let lo = digits(self);
        let hi = if self.eat(',') {
            Some(digits(self))
        } else {
            None
        };
        if !self.eat('}') {
            self.i = here;
            self.atom(&hex_lit('{'));
            return Ok(());
        }
        self.repeat_check()?;
        let parse = |s: &str| -> Result<Option<u64>, String> {
            if s.is_empty() {
                Ok(None)
            } else {
                s.parse::<u64>()
                    .ok()
                    .filter(|&n| n < u64::from(u32::MAX))
                    .map(Some)
                    .ok_or_else(|| "the repetition number is too large".to_string())
            }
        };
        let min = parse(&lo)?.unwrap_or(0);
        let rep = match hi {
            None => format!("{{{min}}}"),
            Some(h) => match parse(&h)? {
                None => format!("{{{min},}}"),
                Some(max) => {
                    if max < min {
                        return self.err("min repeat greater than max repeat");
                    }
                    format!("{{{min},{max}}}")
                }
            },
        };
        self.out.push_str(&rep);
        self.repeat_suffix()
    }

    fn group(&mut self) -> Result<(), String> {
        let scope = self.scope();
        if !self.eat('?') {
            self.groups.push(false);
            self.open.push(Some(self.groups.len()));
            self.scopes.push(scope);
            self.at_start = false;
            self.can_repeat = false;
            self.out.push('(');
            return Ok(());
        }
        let Some(c) = self.peek() else {
            return self.err("unexpected end of pattern");
        };
        self.i += 1;
        match c {
            ':' => self.open_plain("(?:", scope),
            'P' => {
                if self.eat('<') {
                    let name = self.name('>')?;
                    self.groups.push(false);
                    self.open.push(Some(self.groups.len()));
                    self.scopes.push(scope);
                    self.at_start = false;
                    self.can_repeat = false;
                    self.out.push_str(&format!("(?P<{name}>"));
                    Ok(())
                } else if self.eat('=') {
                    let name = self.name(')')?;
                    self.fancy = true;
                    self.atom(&format!("\\k<{name}>"));
                    Ok(())
                } else {
                    self.err("unknown extension ?P")
                }
            }
            '=' | '!' => {
                self.fancy = true;
                self.open_plain(if c == '=' { "(?=" } else { "(?!" }, scope)
            }
            '<' => {
                let Some(d) = self.peek().filter(|d| matches!(d, '=' | '!')) else {
                    return self.err("unknown extension ?<");
                };
                self.i += 1;
                self.fancy = true;
                self.open_plain(if d == '=' { "(?<=" } else { "(?<!" }, scope)
            }
            '>' => {
                self.fancy = true;
                self.open_plain("(?>", scope)
            }
            '#' => {
                while let Some(d) = self.peek() {
                    self.i += 1;
                    if d == ')' {
                        return Ok(());
                    }
                }
                self.err("missing ), unterminated comment")
            }
            '(' => self.err("conditional groups are not supported"),
            c if "aiLmsux-".contains(c) => {
                self.i -= 1;
                self.flags(scope)
            }
            _ => self.err("unknown extension"),
        }
    }

    fn open_plain(&mut self, text: &str, scope: Scope) -> Result<(), String> {
        self.open.push(None);
        self.scopes.push(scope);
        self.at_start = false;
        self.can_repeat = false;
        self.out.push_str(text);
        Ok(())
    }

    fn name(&mut self, end: char) -> Result<String, String> {
        let mut name = String::new();
        loop {
            match self.peek() {
                None => return self.err("missing group name terminator"),
                Some(c) if c == end => {
                    self.i += 1;
                    break;
                }
                Some(c) => {
                    name.push(c);
                    self.i += 1;
                }
            }
        }
        let mut cs = name.chars();
        let ok = cs
            .next()
            .is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
            && cs.all(|c| c == '_' || c.is_ascii_alphanumeric());
        if !ok {
            return self.err("bad character in group name (only ASCII names are supported)");
        }
        Ok(name)
    }

    /// `(?aiLmsux)` (global, only at the start) or `(?aiLmsux-imsx:…)` (scoped).
    fn flags(&mut self, scope: Scope) -> Result<(), String> {
        let mut on = String::new();
        let mut off = String::new();
        let mut neg = false;
        let mut new = scope;
        loop {
            let Some(c) = self.peek() else {
                return self.err("missing -, : or )");
            };
            self.i += 1;
            match c {
                '-' if !neg => neg = true,
                'L' => return self.err("bad inline flags: cannot use 'L' flag with a str pattern"),
                'a' | 'u' if neg => {
                    return self.err("bad inline flags: cannot turn off flags 'a', 'u' and 'L'");
                }
                'a' => new.ascii = true,
                'u' => new.ascii = false,
                'i' | 's' => {
                    if neg {
                        off.push(c);
                    } else {
                        on.push(c);
                    }
                }
                'm' => {
                    new.multiline = !neg;
                    if neg {
                        off.push(c);
                    } else {
                        on.push(c);
                    }
                }
                'x' => new.verbose = !neg,
                ')' if !neg => {
                    // Global flags.
                    if !self.at_start {
                        return self.err("global flags not at the start of the expression");
                    }
                    if let Some(s) = self.scopes.first_mut() {
                        *s = new;
                    }
                    if !on.is_empty() {
                        self.out.push_str(&format!("(?{on})"));
                    }
                    return Ok(());
                }
                ':' => {
                    let text = match (on.is_empty(), off.is_empty()) {
                        (true, true) => "(?:".to_string(),
                        (_, true) => format!("(?{on}:"),
                        _ => format!("(?{on}-{off}:"),
                    };
                    return self.open_plain(&text, new);
                }
                _ => return self.err("unknown flag"),
            }
        }
    }

    fn escape(&mut self) -> Result<(), String> {
        let Some(c) = self.peek() else {
            return self.err("bad escape (end of pattern)");
        };
        self.i += 1;
        let ascii = self.scope().ascii;
        match c {
            'w' => self.atom(&if ascii {
                format!("[{ASCII_WORD}]")
            } else {
                W.to_string()
            }),
            'W' => self.atom(&if ascii {
                format!("[^{ASCII_WORD}]")
            } else {
                tables::NOT_WORD_CLASS.to_string()
            }),
            's' => self.atom(&if ascii {
                format!("[{ASCII_SPACE}]")
            } else {
                S.to_string()
            }),
            'S' => self.atom(&if ascii {
                format!("[^{ASCII_SPACE}]")
            } else {
                tables::NOT_SPACE_CLASS.to_string()
            }),
            'd' => self.atom(&if ascii {
                format!("[{ASCII_DIGIT}]")
            } else {
                D.to_string()
            }),
            'D' => self.atom(&if ascii {
                format!("[^{ASCII_DIGIT}]")
            } else {
                tables::NOT_DECIMAL_CLASS.to_string()
            }),
            'A' => self.assertion("\\A"),
            'Z' => self.assertion("\\z"),
            'b' | 'B' => {
                if ascii {
                    self.fancy = true;
                    let w = format!("[{ASCII_WORD}]");
                    let t = if c == 'b' {
                        format!("(?:(?<={w})(?!{w})|(?<!{w})(?={w}))")
                    } else {
                        format!("(?:(?<={w})(?={w})|(?<!{w})(?!{w}))")
                    };
                    self.assertion(&t);
                } else {
                    self.assertion(if c == 'b' { "\\b" } else { "\\B" });
                }
            }
            '0'..='9' => self.digit_escape(c)?,
            _ => {
                let lit = self.char_escape(c)?;
                self.atom(&hex_lit(lit));
            }
        }
        Ok(())
    }

    /// `\0…` (octal), `\ddd` (octal) or `\d`/`\dd` (a group reference).
    fn digit_escape(&mut self, c: char) -> Result<(), String> {
        if c == '0' {
            let v = self.octal_tail(2);
            return self.literal_code(v);
        }
        let mut num = String::from(c);
        if let Some(d) = self.peek().filter(char::is_ascii_digit) {
            num.push(d);
            self.i += 1;
            let third = self.peek().filter(|t| ('0'..='7').contains(t));
            if ('0'..='7').contains(&c) && ('0'..='7').contains(&d) && third.is_some() {
                self.i += 1;
                num.extend(third);
                let v = u32::from_str_radix(&num, 8).unwrap_or(u32::MAX);
                if v > 0o377 {
                    return self.err("octal escape value outside of range 0-0o377");
                }
                return self.literal_code(v);
            }
        }
        let group: usize = num.parse().unwrap_or(usize::MAX);
        if group == 0 || group > self.groups.len() {
            return self.err("invalid group reference");
        }
        if !self.groups[group - 1] {
            return self.err("cannot refer to an open group");
        }
        self.fancy = true;
        // Wrapped so a following digit stays a literal.
        self.atom(&format!("(?:\\{group})"));
        Ok(())
    }

    /// Up to `max` more octal digits after a leading `\0`.
    fn octal_tail(&mut self, max: usize) -> u32 {
        let mut v = 0u32;
        for _ in 0..max {
            match self.peek() {
                Some(d @ '0'..='7') => {
                    v = v * 8 + (u32::from(d) - u32::from('0'));
                    self.i += 1;
                }
                _ => break,
            }
        }
        v
    }

    fn literal_code(&mut self, v: u32) -> Result<(), String> {
        let Some(ch) = char::from_u32(v) else {
            return self.err("a lone surrogate or bad code point");
        };
        self.atom(&hex_lit(ch));
        Ok(())
    }

    /// The character a non-class, non-digit escape stands for (`\n`, `\x41`, `\.`).
    fn char_escape(&mut self, c: char) -> Result<char, String> {
        let code = match c {
            'a' => 7,
            'f' => 12,
            'n' => 10,
            'r' => 13,
            't' => 9,
            'v' => 11,
            '\\' => 92,
            'x' => self.hex(2, "incomplete escape \\x")?,
            'u' => self.hex(4, "incomplete escape \\u")?,
            'U' => {
                let v = self.hex(8, "incomplete escape \\U")?;
                if v > 0x10_FFFF {
                    return self.err("bad escape \\U");
                }
                v
            }
            'N' => return self.err("named Unicode escapes \\N{...} are not supported"),
            c if c.is_ascii_alphabetic() => return self.err(&format!("bad escape \\{c}")),
            c => u32::from(c),
        };
        match char::from_u32(code) {
            Some(ch) => Ok(ch),
            None => self.err("a lone surrogate"),
        }
    }

    fn hex(&mut self, n: usize, msg: &str) -> Result<u32, String> {
        let mut v = 0u32;
        for _ in 0..n {
            match self.peek().and_then(|d| d.to_digit(16)) {
                Some(d) => {
                    v = v * 16 + d;
                    self.i += 1;
                }
                None => return self.err(msg),
            }
        }
        Ok(v)
    }

    /// `[…]`, Python's class rules (`]` first is literal, ranges, class escapes, `\b` is a
    /// backspace), written with every literal escaped.
    fn class(&mut self) -> Result<(), String> {
        let negate = self.eat('^');
        let mut items = String::new();
        let mut any = false;
        let ascii = self.scope().ascii;
        loop {
            let Some(c) = self.peek() else {
                return self.err("unterminated character set");
            };
            self.i += 1;
            if c == ']' && any {
                break;
            }
            any = true;
            let item = if c == '\\' {
                self.class_escape(ascii)?
            } else {
                ClassItem::Lit(c)
            };
            if self.peek() == Some('-') {
                self.i += 1;
                let Some(that) = self.peek() else {
                    return self.err("unterminated character set");
                };
                self.i += 1;
                if that == ']' {
                    items.push_str(&item.text());
                    items.push_str(&hex_lit('-'));
                    break;
                }
                let hi = if that == '\\' {
                    self.class_escape(ascii)?
                } else {
                    ClassItem::Lit(that)
                };
                match (item, hi) {
                    (ClassItem::Lit(a), ClassItem::Lit(b)) => {
                        if b < a {
                            return self.err("bad character range");
                        }
                        items.push_str(&format!("{}-{}", hex_lit(a), hex_lit(b)));
                    }
                    _ => return self.err("bad character range"),
                }
            } else {
                items.push_str(&item.text());
            }
        }
        let text = format!("[{}{items}]", if negate { "^" } else { "" });
        self.atom(&text);
        Ok(())
    }

    fn class_escape(&mut self, ascii: bool) -> Result<ClassItem, String> {
        let Some(c) = self.peek() else {
            return self.err("unterminated character set");
        };
        self.i += 1;
        let set = |items: &'static str, not: bool| ClassItem::Set(items, not);
        Ok(match c {
            'w' => set(
                if ascii {
                    ASCII_WORD
                } else {
                    tables::WORD_ITEMS
                },
                false,
            ),
            'W' => set(
                if ascii {
                    ASCII_WORD
                } else {
                    tables::WORD_ITEMS
                },
                true,
            ),
            's' => set(
                if ascii {
                    ASCII_SPACE
                } else {
                    tables::SPACE_ITEMS
                },
                false,
            ),
            'S' => set(
                if ascii {
                    ASCII_SPACE
                } else {
                    tables::SPACE_ITEMS
                },
                true,
            ),
            'd' => set(
                if ascii {
                    ASCII_DIGIT
                } else {
                    tables::DECIMAL_ITEMS
                },
                false,
            ),
            'D' => set(
                if ascii {
                    ASCII_DIGIT
                } else {
                    tables::DECIMAL_ITEMS
                },
                true,
            ),
            'b' => ClassItem::Lit('\u{8}'),
            '0'..='7' => {
                let mut v = u32::from(c) - u32::from('0');
                for _ in 0..2 {
                    match self.peek() {
                        Some(d @ '0'..='7') => {
                            v = v * 8 + (u32::from(d) - u32::from('0'));
                            self.i += 1;
                        }
                        _ => break,
                    }
                }
                if v > 0o377 {
                    return self.err("octal escape value outside of range 0-0o377");
                }
                ClassItem::Lit(char::from_u32(v).unwrap_or('\0'))
            }
            '8' | '9' => return self.err("bad escape in a class"),
            c => ClassItem::Lit(self.char_escape(c)?),
        })
    }
}

enum ClassItem {
    Lit(char),
    /// A shorthand class's items, and whether it is negated.
    Set(&'static str, bool),
}

impl ClassItem {
    fn text(&self) -> String {
        match self {
            ClassItem::Lit(c) => hex_lit(*c),
            ClassItem::Set(items, false) => (*items).to_string(),
            ClassItem::Set(items, true) => format!("[^{items}]"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walk_reports_an_empty_match_after_a_non_empty_one() {
        let r = compile("x*", false).unwrap();
        let spans: Vec<_> = r.find_iter("abxd").map(Result::unwrap).collect();
        assert_eq!(spans, [(0, 0), (1, 1), (2, 3), (3, 3), (4, 4)]);
    }
}
