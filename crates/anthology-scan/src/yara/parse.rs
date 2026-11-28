//! The rule parser: `rule NAME { meta: strings: condition: }` (Task 5).
//!
//! The grammar is a subset of YARA's. Syntax outside it is an error for the one rule that uses it:
//! the rule is dropped, the reason is reported, and parsing resumes at the next `rule`.

pub(super) type R<T> = Result<T, String>;

/// How deep parentheses and `not` may nest. Deeper input is an error, not a stack overflow.
const MAX_DEPTH: usize = 48;

/// A meta value. Only strings are read; a number or a boolean is parsed and dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum MetaVal {
    Str(String),
    Other,
}

/// One element of a hex string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Hex {
    Byte(u8),
    /// `??`.
    Any,
    /// `[n-m]`: between `n` and `m` bytes of anything.
    Gap(u32, u32),
    /// `( a | b )`.
    Alt(Vec<Vec<Hex>>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Body {
    Text(Vec<u8>),
    Regex {
        pat: String,
        nocase: bool,
        dotall: bool,
    },
    Hex(Vec<Hex>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Str {
    /// The name without the `$`.
    pub id: String,
    pub body: Body,
    pub nocase: bool,
    pub wide: bool,
    pub ascii: bool,
    pub fullword: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Quant {
    Any,
    All,
    None,
    N(usize),
}

/// A condition. Booleans are 0 and 1, as in YARA.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Expr {
    Bool(bool),
    Int(i64),
    FileSize,
    /// `$id`: the string's index in the rule.
    Str(usize),
    /// `#id`.
    Count(usize),
    /// A rule named in a condition, before the compiler resolves it.
    Rule(String),
    /// The same rule as an index into the set.
    RuleRef(usize),
    /// `ecosystem == "x"` or `ecosystem != "x"`.
    Eco {
        eq: bool,
        name: String,
    },
    Not(Box<Expr>),
    And(Vec<Expr>),
    Or(Vec<Expr>),
    Cmp(Op, Box<Expr>, Box<Expr>),
    /// `a + b - c`: each term with `true` when it is subtracted.
    Sum(Vec<(bool, Expr)>),
    /// `any|all|none|N of` a set of string indices.
    Of(Quant, Vec<usize>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Rule {
    pub name: String,
    pub private: bool,
    pub meta: Vec<(String, MetaVal)>,
    pub strings: Vec<Str>,
    pub cond: Expr,
}

impl Rule {
    pub(super) fn meta_str(&self, key: &str) -> Option<&str> {
        self.meta.iter().find_map(|(k, v)| match v {
            MetaVal::Str(s) if k == key => Some(s.as_str()),
            _ => None,
        })
    }
}

#[derive(Default)]
pub(super) struct Parsed {
    pub rules: Vec<Rule>,
    /// `(rule, reason)`; the file name stands in when the rule has no name yet.
    pub errors: Vec<(String, String)>,
}

/// Parses one file. It never panics, whatever `src` holds.
pub(super) fn parse(file: &str, src: &str) -> Parsed {
    let mut p = P {
        src,
        b: src.as_bytes(),
        i: 0,
        depth: 0,
        rule_name: None,
    };
    let mut out = Parsed::default();
    loop {
        if let Err(e) = p.ws() {
            out.errors.push((file.to_string(), e));
            break;
        }
        if p.i >= p.b.len() {
            break;
        }
        let item_start = p.i;
        p.rule_name = None;
        match p.item() {
            Ok(Some(rule)) => out.rules.push(rule),
            Ok(None) => {}
            Err(reason) => {
                let name = p.rule_name.take().unwrap_or_else(|| file.to_string());
                out.errors.push((name, reason));
                if !p.resync(item_start) {
                    break;
                }
            }
        }
    }
    out
}

struct P<'a> {
    src: &'a str,
    b: &'a [u8],
    i: usize,
    depth: usize,
    rule_name: Option<String>,
}

fn is_word(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

impl<'a> P<'a> {
    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn peek_at(&self, k: usize) -> Option<u8> {
        self.b.get(self.i + k).copied()
    }

    /// The text of the next token, for an error message.
    fn tok(&self) -> String {
        match self.peek() {
            None => "end of input".to_string(),
            Some(c) if is_word(c) => {
                let end = self.b[self.i..]
                    .iter()
                    .position(|&c| !is_word(c))
                    .map_or(self.b.len(), |n| self.i + n);
                self.src[self.i..end].to_string()
            }
            Some(_) => self
                .src
                .get(self.i..)
                .and_then(|s| s.chars().next())
                .map_or("?".to_string(), String::from),
        }
    }

    /// Skips spaces and comments.
    fn ws(&mut self) -> R<()> {
        loop {
            match (self.peek(), self.peek_at(1)) {
                (Some(c), _) if c.is_ascii_whitespace() => self.i += 1,
                (Some(b'/'), Some(b'/')) => {
                    while self.peek().is_some_and(|c| c != b'\n') {
                        self.i += 1;
                    }
                }
                (Some(b'/'), Some(b'*')) => match self.src[self.i + 2..].find("*/") {
                    Some(n) => self.i += 2 + n + 2,
                    None => return Err("unterminated comment".to_string()),
                },
                _ => return Ok(()),
            }
        }
    }

    /// The identifier at the cursor, not consumed.
    fn word(&self) -> Option<&'a str> {
        let c = self.peek()?;
        if !(c.is_ascii_alphabetic() || c == b'_') {
            return None;
        }
        let end = self.b[self.i..]
            .iter()
            .position(|&c| !is_word(c))
            .map_or(self.b.len(), |n| self.i + n);
        Some(&self.src[self.i..end])
    }

    fn take_word(&mut self) -> Option<&'a str> {
        let w = self.word()?;
        self.i += w.len();
        Some(w)
    }

    fn eat(&mut self, s: &str) -> bool {
        if self.b[self.i..].starts_with(s.as_bytes()) {
            self.i += s.len();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, s: &str) -> R<()> {
        if self.eat(s) {
            Ok(())
        } else {
            Err(format!("expected `{s}`, found `{}`", self.tok()))
        }
    }

    fn enter(&mut self) -> R<()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err("the condition nests too deeply".to_string());
        }
        Ok(())
    }

    fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    /// After an error: moves to the next line that starts a rule. `false` when there is none.
    fn resync(&mut self, item_start: usize) -> bool {
        self.depth = 0;
        let mut at = self.i.max(item_start + 1).min(self.b.len());
        loop {
            match self.b[at..].iter().position(|&c| c == b'\n') {
                None => {
                    self.i = self.b.len();
                    return false;
                }
                Some(n) => at += n + 1,
            }
            let line = &self.b[at..];
            let trimmed = line
                .iter()
                .position(|c| !c.is_ascii_whitespace() || *c == b'\n')
                .unwrap_or(line.len());
            let rest = &line[trimmed..];
            let starts_rule = [
                &b"rule "[..],
                b"rule\t",
                b"private rule ",
                b"private\trule ",
                b"global rule ",
            ]
            .iter()
            .any(|k| rest.starts_with(k));
            if starts_rule {
                self.i = at;
                return true;
            }
        }
    }

    // --- items -----------------------------------------------------------------------------

    fn item(&mut self) -> R<Option<Rule>> {
        let Some(w) = self.take_word() else {
            return Err(format!("unexpected `{}`", self.tok()));
        };
        match w {
            "include" => {
                self.ws()?;
                self.text()?;
                Ok(None)
            }
            "import" => Err("unsupported syntax `import`".to_string()),
            "global" => Err("unsupported modifier `global`".to_string()),
            "private" => {
                self.ws()?;
                if self.take_word() != Some("rule") {
                    return Err("expected `rule` after `private`".to_string());
                }
                self.rule(true).map(Some)
            }
            "rule" => self.rule(false).map(Some),
            other => Err(format!("unexpected `{other}`")),
        }
    }

    fn rule(&mut self, private: bool) -> R<Rule> {
        self.ws()?;
        let name = self
            .take_word()
            .ok_or_else(|| format!("expected a rule name, found `{}`", self.tok()))?;
        self.rule_name = Some(name.to_string());
        self.ws()?;
        if self.eat(":") {
            // Tags are accepted and ignored.
            loop {
                self.ws()?;
                if self.take_word().is_none() {
                    break;
                }
            }
        }
        self.expect("{")?;
        let mut meta = Vec::new();
        let mut strings = Vec::new();
        let mut cond = None;
        loop {
            self.ws()?;
            if self.eat("}") {
                break;
            }
            let Some(section) = self.take_word() else {
                return Err(format!("unsupported syntax `{}`", self.tok()));
            };
            match section {
                "meta" | "strings" | "condition" => {
                    self.ws()?;
                    self.expect(":")?;
                }
                other => return Err(format!("unsupported syntax `{other}`")),
            }
            match section {
                "meta" => meta = self.meta()?,
                "strings" => strings = self.strings()?,
                _ => cond = Some(self.condition(&strings)?),
            }
        }
        let cond = cond.ok_or_else(|| "the rule has no condition".to_string())?;
        Ok(Rule {
            name: name.to_string(),
            private,
            meta,
            strings,
            cond,
        })
    }

    // --- meta ------------------------------------------------------------------------------

    fn meta(&mut self) -> R<Vec<(String, MetaVal)>> {
        let mut out = Vec::new();
        loop {
            self.ws()?;
            match self.word() {
                None | Some("strings" | "condition" | "meta") => return Ok(out),
                Some(_) => {}
            }
            let key = self.take_word().unwrap_or_default().to_string();
            self.ws()?;
            self.expect("=")?;
            self.ws()?;
            let val = match self.peek() {
                Some(b'"') => MetaVal::Str(String::from_utf8_lossy(&self.text()?).into_owned()),
                Some(c) if c.is_ascii_digit() || c == b'-' => {
                    self.int()?;
                    MetaVal::Other
                }
                _ => match self.take_word() {
                    Some("true" | "false") => MetaVal::Other,
                    _ => return Err(format!("unsupported meta value for `{key}`")),
                },
            };
            out.push((key, val));
        }
    }

    // --- strings ---------------------------------------------------------------------------

    fn strings(&mut self) -> R<Vec<Str>> {
        let mut out: Vec<Str> = Vec::new();
        loop {
            self.ws()?;
            if self.peek() != Some(b'$') {
                return Ok(out);
            }
            self.i += 1;
            let id = self.take_word_chars();
            if id.is_empty() {
                return Err("unsupported anonymous string `$`".to_string());
            }
            if out.iter().any(|s| s.id == id) {
                return Err(format!("duplicate string ${id}"));
            }
            self.ws()?;
            self.expect("=")?;
            self.ws()?;
            let body = match self.peek() {
                Some(b'"') => Body::Text(self.text()?),
                Some(b'/') => self.regex()?,
                Some(b'{') => Body::Hex(self.hex()?),
                _ => return Err(format!("unsupported string syntax `{}`", self.tok())),
            };
            let mut s = Str {
                id,
                body,
                nocase: false,
                wide: false,
                ascii: false,
                fullword: false,
            };
            loop {
                self.ws()?;
                match self.word() {
                    None | Some("condition" | "strings" | "meta") => break,
                    Some(_) => {}
                }
                match self.take_word().unwrap_or_default() {
                    "nocase" => s.nocase = true,
                    "wide" => s.wide = true,
                    "ascii" => s.ascii = true,
                    "fullword" => s.fullword = true,
                    other => return Err(format!("unsupported modifier `{other}`")),
                }
            }
            out.push(s);
        }
    }

    /// Letters, digits and underscores at the cursor; may be empty.
    fn take_word_chars(&mut self) -> String {
        let start = self.i;
        while self.peek().is_some_and(is_word) {
            self.i += 1;
        }
        self.src[start..self.i].to_string()
    }

    /// A double-quoted string as bytes. The cursor is on the opening quote.
    fn text(&mut self) -> R<Vec<u8>> {
        self.expect("\"")?;
        let mut out = Vec::new();
        loop {
            match self.peek() {
                None | Some(b'\n') => return Err("unterminated string".to_string()),
                Some(b'"') => {
                    self.i += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    let esc = self
                        .peek_at(1)
                        .ok_or_else(|| "unterminated string".to_string())?;
                    self.i += 2;
                    match esc {
                        b'n' => out.push(b'\n'),
                        b't' => out.push(b'\t'),
                        b'r' => out.push(b'\r'),
                        b'\\' => out.push(b'\\'),
                        b'"' => out.push(b'"'),
                        b'x' => {
                            let hex = self
                                .src
                                .get(self.i..self.i + 2)
                                .and_then(|h| u8::from_str_radix(h, 16).ok());
                            out.push(
                                hex.ok_or_else(|| "bad `\\x` escape in a string".to_string())?,
                            );
                            self.i += 2;
                        }
                        other => {
                            return Err(format!(
                                "unsupported escape `\\{}` in a string",
                                other as char
                            ));
                        }
                    }
                }
                Some(c) => {
                    out.push(c);
                    self.i += 1;
                }
            }
        }
    }

    /// `/pattern/flags`. The cursor is on the opening slash.
    fn regex(&mut self) -> R<Body> {
        self.i += 1;
        let mut pat = String::new();
        loop {
            match self.peek() {
                None | Some(b'\n') => return Err("unterminated regex".to_string()),
                Some(b'/') => {
                    self.i += 1;
                    break;
                }
                Some(b'\\') => {
                    self.i += 1;
                    if self.peek() == Some(b'/') {
                        pat.push('/');
                        self.i += 1;
                    } else {
                        pat.push('\\');
                        let ch = self.src.get(self.i..).and_then(|s| s.chars().next());
                        let ch = ch.ok_or_else(|| "unterminated regex".to_string())?;
                        pat.push(ch);
                        self.i += ch.len_utf8();
                    }
                }
                Some(_) => {
                    let ch = self.src.get(self.i..).and_then(|s| s.chars().next());
                    let ch = ch.ok_or_else(|| "unterminated regex".to_string())?;
                    pat.push(ch);
                    self.i += ch.len_utf8();
                }
            }
        }
        let (mut nocase, mut dotall) = (false, false);
        loop {
            match self.peek() {
                Some(b'i') => nocase = true,
                Some(b's') => dotall = true,
                _ => break,
            }
            self.i += 1;
        }
        Ok(Body::Regex {
            pat,
            nocase,
            dotall,
        })
    }

    /// `{ 4D 5A ?? [2-4] ( 00 | 01 ) }`. The cursor is on the opening brace.
    fn hex(&mut self) -> R<Vec<Hex>> {
        self.i += 1;
        let seq = self.hex_seq()?;
        self.expect("}")?;
        Ok(seq)
    }

    /// Elements up to a `}`, `|` or `)`, none of them consumed.
    fn hex_seq(&mut self) -> R<Vec<Hex>> {
        let mut out = Vec::new();
        loop {
            self.ws()?;
            match self.peek() {
                None => return Err("unterminated hex string".to_string()),
                Some(b'}' | b'|' | b')') => return Ok(out),
                Some(b'(') => {
                    self.enter()?;
                    self.i += 1;
                    let mut alts = vec![self.hex_seq()?];
                    while self.eat("|") {
                        alts.push(self.hex_seq()?);
                    }
                    self.expect(")")?;
                    self.leave();
                    out.push(Hex::Alt(alts));
                }
                Some(b'[') => {
                    self.i += 1;
                    out.push(self.hex_gap()?);
                }
                Some(b'?') => {
                    if self.peek_at(1) != Some(b'?') {
                        return Err(
                            "nibble wildcards in a hex string are not supported".to_string()
                        );
                    }
                    self.i += 2;
                    out.push(Hex::Any);
                }
                Some(c) if c.is_ascii_hexdigit() => {
                    let second = self.peek_at(1);
                    match second {
                        Some(d) if d.is_ascii_hexdigit() => {
                            let byte = u8::from_str_radix(&self.src[self.i..self.i + 2], 16)
                                .unwrap_or_default();
                            self.i += 2;
                            out.push(Hex::Byte(byte));
                        }
                        Some(b'?') => {
                            return Err(
                                "nibble wildcards in a hex string are not supported".to_string()
                            );
                        }
                        _ => return Err("a hex string takes whole bytes".to_string()),
                    }
                }
                Some(_) => return Err(format!("unexpected `{}` in a hex string", self.tok())),
            }
        }
    }

    /// `n]`, `n-m]`, `-m]`; the `[` is consumed.
    fn hex_gap(&mut self) -> R<Hex> {
        self.ws()?;
        let lo = self.digits();
        self.ws()?;
        let hi = if self.eat("-") {
            self.ws()?;
            let hi = self.digits();
            if hi.is_none() {
                return Err("an unbounded gap in a hex string is not supported".to_string());
            }
            hi
        } else {
            lo
        };
        self.ws()?;
        self.expect("]")?;
        let lo = lo.unwrap_or(0);
        let hi = hi.ok_or_else(|| "an empty gap in a hex string".to_string())?;
        if lo > hi {
            return Err("a gap in a hex string has its bounds reversed".to_string());
        }
        Ok(Hex::Gap(lo, hi))
    }

    fn digits(&mut self) -> Option<u32> {
        let start = self.i;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.i += 1;
        }
        if start == self.i {
            return None;
        }
        // Too large for a u32 is still a number: the regex size limit rejects it later.
        Some(self.src[start..self.i].parse::<u32>().unwrap_or(u32::MAX))
    }

    // --- conditions ------------------------------------------------------------------------

    fn condition(&mut self, strings: &[Str]) -> R<Expr> {
        self.depth = 0;
        self.or(strings)
    }

    fn or(&mut self, strings: &[Str]) -> R<Expr> {
        let mut terms = vec![self.and(strings)?];
        loop {
            self.ws()?;
            if self.word() != Some("or") {
                break;
            }
            self.i += 2;
            terms.push(self.and(strings)?);
        }
        Ok(if terms.len() == 1 {
            terms.remove(0)
        } else {
            Expr::Or(terms)
        })
    }

    fn and(&mut self, strings: &[Str]) -> R<Expr> {
        let mut terms = vec![self.not(strings)?];
        loop {
            self.ws()?;
            if self.word() != Some("and") {
                break;
            }
            self.i += 3;
            terms.push(self.not(strings)?);
        }
        Ok(if terms.len() == 1 {
            terms.remove(0)
        } else {
            Expr::And(terms)
        })
    }

    fn not(&mut self, strings: &[Str]) -> R<Expr> {
        self.ws()?;
        if self.word() == Some("not") {
            self.i += 3;
            self.enter()?;
            let inner = self.not(strings)?;
            self.leave();
            return Ok(Expr::Not(Box::new(inner)));
        }
        self.cmp(strings)
    }

    fn cmp(&mut self, strings: &[Str]) -> R<Expr> {
        let lhs = self.sum(strings)?;
        self.ws()?;
        let op = match (self.peek(), self.peek_at(1)) {
            (Some(b'='), Some(b'=')) => Some((Op::Eq, 2)),
            (Some(b'!'), Some(b'=')) => Some((Op::Ne, 2)),
            (Some(b'<'), Some(b'=')) => Some((Op::Le, 2)),
            (Some(b'>'), Some(b'=')) => Some((Op::Ge, 2)),
            (Some(b'<'), _) => Some((Op::Lt, 1)),
            (Some(b'>'), _) => Some((Op::Gt, 1)),
            _ => None,
        };
        let Some((op, len)) = op else {
            return Ok(lhs);
        };
        self.i += len;
        let rhs = self.sum(strings)?;
        Ok(Expr::Cmp(op, Box::new(lhs), Box::new(rhs)))
    }

    fn sum(&mut self, strings: &[Str]) -> R<Expr> {
        let first = self.primary(strings)?;
        let mut terms = Vec::new();
        loop {
            self.ws()?;
            let neg = match self.peek() {
                Some(b'+') => false,
                Some(b'-') => true,
                _ => break,
            };
            self.i += 1;
            terms.push((neg, self.primary(strings)?));
        }
        if terms.is_empty() {
            return Ok(first);
        }
        terms.insert(0, (false, first));
        Ok(Expr::Sum(terms))
    }

    fn primary(&mut self, strings: &[Str]) -> R<Expr> {
        self.ws()?;
        match self.peek() {
            None => Err("the condition ends too soon".to_string()),
            Some(b'(') => {
                self.enter()?;
                self.i += 1;
                let inner = self.or(strings)?;
                self.ws()?;
                self.expect(")")?;
                self.leave();
                Ok(inner)
            }
            Some(b'$') => {
                self.i += 1;
                let id = self.take_word_chars();
                if self.peek() == Some(b'*') {
                    return Err(format!(
                        "the wildcard `${id}*` is only supported inside `of`"
                    ));
                }
                Ok(Expr::Str(find_string(strings, &id)?))
            }
            Some(b'#') => {
                self.i += 1;
                let id = self.take_word_chars();
                Ok(Expr::Count(find_string(strings, &id)?))
            }
            Some(c) if c.is_ascii_digit() || c == b'-' => {
                let n = self.int()?;
                self.ws()?;
                if self.word() == Some("of") {
                    self.i += 2;
                    let n = usize::try_from(n)
                        .map_err(|_| "a negative count before `of`".to_string())?;
                    return self.of(Quant::N(n), strings);
                }
                Ok(Expr::Int(n))
            }
            Some(_) => {
                let Some(w) = self.take_word() else {
                    return Err(format!("unsupported syntax `{}`", self.tok()));
                };
                self.word_primary(w, strings)
            }
        }
    }

    fn word_primary(&mut self, w: &str, strings: &[Str]) -> R<Expr> {
        match w {
            "true" => return Ok(Expr::Bool(true)),
            "false" => return Ok(Expr::Bool(false)),
            "filesize" => return Ok(Expr::FileSize),
            "any" | "all" | "none" => {
                self.ws()?;
                if self.take_word() != Some("of") {
                    return Err(format!("unsupported syntax: `{w}` without `of`"));
                }
                let q = match w {
                    "any" => Quant::Any,
                    "all" => Quant::All,
                    _ => Quant::None,
                };
                return self.of(q, strings);
            }
            "ecosystem" => {
                self.ws()?;
                let eq = if self.eat("==") {
                    true
                } else if self.eat("!=") {
                    false
                } else {
                    return Err(
                        "`ecosystem` is only supported with `==` or `!=` and a string".to_string(),
                    );
                };
                self.ws()?;
                let name = String::from_utf8_lossy(&self.text()?).into_owned();
                return Ok(Expr::Eco { eq, name });
            }
            "for" | "them" | "of" | "at" | "in" | "matches" | "contains" | "import" | "defined" => {
                return Err(format!("unsupported syntax `{w}`"));
            }
            _ => {}
        }
        self.ws()?;
        if self.peek() == Some(b'(') {
            return Err(format!("unsupported function `{w}`"));
        }
        Ok(Expr::Rule(w.to_string()))
    }

    /// A decimal or `0x` integer with an optional `KB` or `MB` suffix. The cursor is on a digit or `-`.
    fn int(&mut self) -> R<i64> {
        let neg = self.eat("-");
        let start = self.i;
        let n = if self.b[self.i..].starts_with(b"0x") {
            self.i += 2;
            let digits = self.i;
            while self.peek().is_some_and(|c| c.is_ascii_hexdigit()) {
                self.i += 1;
            }
            i64::from_str_radix(&self.src[digits..self.i], 16)
        } else {
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.i += 1;
            }
            self.src[start..self.i].parse::<i64>()
        };
        let mut n = n.map_err(|_| "a number is missing or too large".to_string())?;
        for (suffix, factor) in [("KB", 1 << 10), ("MB", 1 << 20)] {
            if self.eat(suffix) {
                n = n
                    .checked_mul(factor)
                    .ok_or_else(|| "a number is too large".to_string())?;
                break;
            }
        }
        Ok(if neg { -n } else { n })
    }

    /// The set after `of`: `them` or `( $a, $b*, ... )`.
    fn of(&mut self, quant: Quant, strings: &[Str]) -> R<Expr> {
        self.ws()?;
        let mut set: Vec<usize> = Vec::new();
        if self.word() == Some("them") {
            self.i += 4;
            set.extend(0..strings.len());
        } else {
            self.expect("(")?;
            loop {
                self.ws()?;
                if self.peek() != Some(b'$') {
                    return Err(format!(
                        "unsupported syntax `{}` in an `of` set",
                        self.tok()
                    ));
                }
                self.i += 1;
                let id = self.take_word_chars();
                if self.eat("*") {
                    let before = set.len();
                    set.extend(
                        strings
                            .iter()
                            .enumerate()
                            .filter(|(_, s)| s.id.starts_with(&id))
                            .map(|(n, _)| n),
                    );
                    if set.len() == before {
                        return Err(format!("no string matches `${id}*`"));
                    }
                } else {
                    set.push(find_string(strings, &id)?);
                }
                self.ws()?;
                if !self.eat(",") {
                    break;
                }
            }
            self.expect(")")?;
        }
        set.sort_unstable();
        set.dedup();
        if set.is_empty() {
            return Err("an `of` set has no strings".to_string());
        }
        Ok(Expr::Of(quant, set))
    }
}

fn find_string(strings: &[Str], id: &str) -> R<usize> {
    strings
        .iter()
        .position(|s| s.id == id)
        .ok_or_else(|| format!("undefined string ${id}"))
}
