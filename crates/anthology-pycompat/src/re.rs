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
//!   pass through (fancy-regex); a possessive repeat `X*+` becomes the atomic `(?>X*)`;
//! - a repeated look-around (Python allows it, fancy-regex does not) is written as sre runs it:
//!   once when the minimum is at least 1, optional (`(?:A|)`, lazy `(?:|A)`) when it is 0, and
//!   never (keeping its groups) when the maximum is 0; a repeated group with nothing in it
//!   (`(?:)*`, `(?x: ){2}`, which fancy-regex refuses) is written once, without the repeat;
//! - every literal is written as `\x{…}`, so no Python literal (`{`, `[` in a class, `&&`, `~~`,
//!   `--`, `\<`) can turn into Rust syntax; a `{` that is not a valid repeat is a literal, as in
//!   Python; `{,n}` is `{0,n}`;
//! - IGNORECASE (the flag or an inline `i`, scoped as Python scopes it) is applied here, not by
//!   the engines: each cased literal and class is written out as the explicit set of what
//!   CPython's sre matches for it (`_sre.unicode_tolower`, `_sre.unicode_iscased`,
//!   `re._casefix`'s extra cases, from the generated tables), because Rust's simple case
//!   folding differs from it;
//! - inline `m s` pass through, `x` (verbose) and `a` (ASCII) are applied here, `u` is a no-op,
//!   and `L` is refused (a str pattern), as Python refuses it.
//!
//! Anything Python reads differently from both engines, or that this does not support exactly,
//! is an `Err`, never a silent difference: `\N{…}`, conditionals `(?(1)…)`, a lone surrogate, a
//! back-reference under IGNORECASE, a case-insensitive class range past U+FFFF, and a capture
//! group inside a repeat whose body can match empty (sre keeps the groups of a last, empty
//! iteration; Rust's engines do not) unless the count is fixed. `\b` and `\B` are sre's: look-
//! arounds over [`W`] (or `[0-9A-Za-z_]` under ASCII), since Rust's own `\b` uses a different,
//! newer Unicode word set; `\B` never matches in an empty string, as in CPython. So a pattern
//! with `\b` needs fancy-regex. One thing Python refuses is accepted: a look-behind whose width
//! is not fixed (fancy-regex runs it); no Python source can hold one.
//!
//! [`compile`] picks `regex` when the translation needs no look-around, back-reference or
//! atomic group, and `fancy-regex` (with its default backtrack limit) otherwise. fancy-regex
//! counts that limit over a whole search, so a long text with no match can exhaust it: a fancy
//! pattern whose only fancy parts are `\b`, `\B` and `$` is therefore scanned by `regex` with
//! those removed (a superset) and each candidate start confirmed by fancy-regex anchored there,
//! with its own budget (where rejected candidates keep costing long superset scans, every start
//! up to the superset's earliest match end is tried instead: the cost of sre's own per-start
//! search, not a rescan per candidate). Other fancy patterns keep the whole-search limit (D-7). Spans are byte
//! offsets into the haystack. [`Regex::find_iter`] walks matches as Python's `finditer` does:
//! an empty match right after a non-empty one is reported (`x*` on `abxd` gives
//! `(0,0) (1,1) (2,3) (3,3) (4,4)`), and after an empty match the next search at the same
//! position takes the first non-empty way of matching there, as sre's `must_advance` retry does
//! (`a??` on `a` gives `(0,0) (0,1) (1,1)`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

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
        let re = fancy_regex::Regex::new(&t.pattern)
            .map(Regex::Fancy)
            .map_err(|e| e.to_string())?;
        // Builds the prefilter now, so a failure is a compile error, not a match error.
        aux(&re).map_err(|e| e.0)?;
        Ok(re)
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
            Regex::Fancy(r) => match &aux(self)?.prefilter {
                Some(_) => Ok(self.find_at(s, 0)?.is_some()),
                None => r.is_match(s).map_err(limit),
            },
        }
    }

    /// `p.search(s, pos)`'s span: the leftmost match starting at or after byte `pos`, with the
    /// whole string as context (look-behind, `\b`, `^`).
    pub fn find_at(&self, s: &str, pos: usize) -> Result<Option<(usize, usize)>, LimitError> {
        match self {
            Regex::Plain(r) => Ok(r.find_at(s, pos).map(|m| (m.start(), m.end()))),
            Regex::Fancy(r) => find_fancy(r, &*aux(self)?, s, pos),
        }
    }

    /// `p.search(s, pos)`'s groups.
    pub fn captures_at(&self, s: &str, pos: usize) -> Result<Option<Groups>, LimitError> {
        match self {
            Regex::Plain(r) => Ok(r
                .captures_at(s, pos)
                .map(|c| c.iter().map(|g| g.map(|m| (m.start(), m.end()))).collect())),
            Regex::Fancy(r) => captures_fancy(r, &*aux(self)?, s, pos),
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
        let aux = aux(self);
        std::iter::from_fn(move || {
            let aux = match &aux {
                Ok(a) => a,
                Err(e) if !walk.done => {
                    walk.done = true;
                    return Some(Err(e.clone()));
                }
                Err(_) => return None,
            };
            walk.next(
                s,
                |pos| {
                    let m = match self {
                        Regex::Plain(r) => Ok(r.find_at(s, pos).map(|m| (m.start(), m.end()))),
                        Regex::Fancy(r) => find_fancy(r, aux, s, pos),
                    };
                    m.map(|m| m.map(|m| (m, m)))
                },
                |pos| nonempty_at(aux, s, pos).map(|g| g.and_then(|g| g[0]).map(|m| (m, m))),
            )
            .map(|r| r.map(|(span, _)| span))
        })
    }

    /// `[m.regs for m in p.finditer(s)]`, in bytes.
    pub fn captures_iter<'a>(
        &'a self,
        s: &'a str,
    ) -> impl Iterator<Item = Result<Groups, LimitError>> + 'a {
        let mut walk = Walk::default();
        let aux = aux(self);
        std::iter::from_fn(move || {
            let aux = match &aux {
                Ok(a) => a,
                Err(e) if !walk.done => {
                    walk.done = true;
                    return Some(Err(e.clone()));
                }
                Err(_) => return None,
            };
            walk.next(
                s,
                |pos| {
                    let c = match self {
                        Regex::Plain(r) => Ok(r.captures_at(s, pos).map(|c| {
                            c.iter()
                                .map(|g| g.map(|m| (m.start(), m.end())))
                                .collect::<Groups>()
                        })),
                        Regex::Fancy(r) => captures_fancy(r, aux, s, pos),
                    };
                    c.map(|c| c.map(|g| (g[0].unwrap_or((pos, pos)), g)))
                },
                |pos| nonempty_at(aux, s, pos).map(|c| c.map(|g| (g[0].unwrap_or((pos, pos)), g))),
            )
            .map(|r| r.map(|(_, g)| g))
        })
    }

    fn as_str(&self) -> &str {
        match self {
            Regex::Plain(r) => r.as_str(),
            Regex::Fancy(r) => r.as_str(),
        }
    }
}

/// What a compiled pattern needs besides its engine, built once per pattern text.
struct Aux {
    /// The pattern text (the cache key).
    text: Box<str>,
    /// `\G(?:P)(?!\G|(?!))`, sre's `must_advance` retry (see [`nonempty_at`]); built when first
    /// needed.
    nonempty: OnceLock<Result<fancy_regex::Regex, LimitError>>,
    /// For a fancy pattern whose only fancy parts are `\b`, `\B` and `$`: the pattern without
    /// them (it matches a superset, on the `regex` engine), and the anchored `\G(?:P)` that
    /// confirms a candidate start. fancy-regex counts its backtrack limit over a whole search, so
    /// a long text with no match would hit it; this way each candidate start has its own
    /// budget, and the linear engine does the scanning.
    prefilter: Option<(regex::Regex, fancy_regex::Regex)>,
}

/// The whole match and groups of a fancy-regex `Captures`.
fn groups_of(c: &fancy_regex::Captures<'_, str>) -> Groups {
    c.iter().map(|g| g.map(|m| (m.start(), m.end()))).collect()
}

/// The pieces the translation writes for `\b`, `\B` and a non-MULTILINE `$`.
fn dropped_for_prefilter() -> &'static [String] {
    static PIECES: OnceLock<Vec<String>> = OnceLock::new();
    PIECES.get_or_init(|| {
        let mut out = Vec::new();
        for ascii in [false, true] {
            for c in ['b', 'B'] {
                out.push(boundary(c, ascii));
            }
        }
        out.push(DOLLAR.to_string());
        out
    })
}

fn build_aux(text: &str, fancy: bool) -> Result<Aux, LimitError> {
    let mut prefilter = None;
    if fancy {
        let mut superset = text.to_string();
        for piece in dropped_for_prefilter() {
            superset = superset.replace(piece.as_str(), "");
        }
        // The `regex` engine refuses whatever fancy part is left (a look-around, a
        // back-reference, an atomic group): then there is no prefilter.
        if superset != text
            && let Ok(pre) = regex::Regex::new(&superset)
        {
            let anchored = fancy_regex::Regex::new(&format!("\\G(?:{text})"))
                .map_err(|e| LimitError(format!("the anchored form: {e}")))?;
            prefilter = Some((pre, anchored));
        }
    }
    Ok(Aux {
        text: text.into(),
        nonempty: OnceLock::new(),
        prefilter,
    })
}

/// The [`Aux`] of `re`, from a process-wide cache (keyed by the pattern's length and a hash of
/// its ends, then compared in full). [`Regex`] is the §3.2 enum of the two engines and has no
/// room for it, so each direct `is_match`/`find_at`/`captures_at` on a fancy pattern pays this
/// lookup (a compare of the pattern text: microseconds for the longest privacy patterns);
/// `find_iter` and `captures_iter` pay it once.
fn aux(re: &Regex) -> Result<Arc<Aux>, LimitError> {
    type Cache = HashMap<(usize, u64), Vec<Arc<Aux>>>;
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    let text = re.as_str();
    let key = {
        use std::hash::{Hash, Hasher};
        let b = text.as_bytes();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        b[..b.len().min(256)].hash(&mut h);
        b[b.len().saturating_sub(256)..].hash(&mut h);
        (b.len(), h.finish())
    };
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    {
        let cache = cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(a) = cache
            .get(&key)
            .and_then(|v| v.iter().find(|a| *a.text == *text))
        {
            return Ok(a.clone());
        }
    }
    let built = Arc::new(build_aux(text, re.is_fancy())?);
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    if cache.len() > 4096 {
        cache.clear();
    }
    cache.entry(key).or_default().push(built.clone());
    Ok(built)
}

/// A candidate start after `c` (one character on).
fn next_start(s: &str, c: usize) -> usize {
    c + s[c..].chars().next().map_or(1, char::len_utf8)
}

fn find_fancy(
    r: &fancy_regex::Regex,
    aux: &Aux,
    s: &str,
    pos: usize,
) -> Result<Option<(usize, usize)>, LimitError> {
    let Some((pre, anchored)) = &aux.prefilter else {
        return r
            .find_from_pos(s, pos)
            .map(|m| m.map(|m| (m.start(), m.end())))
            .map_err(limit);
    };
    prefiltered(pre, s, pos, |i| {
        anchored
            .find_from_pos(s, i)
            .map(|m| m.map(|m| (m.start(), m.end())))
            .map_err(limit)
    })
}

fn captures_fancy(
    r: &fancy_regex::Regex,
    aux: &Aux,
    s: &str,
    pos: usize,
) -> Result<Option<Groups>, LimitError> {
    let Some((pre, anchored)) = &aux.prefilter else {
        return r
            .captures_from_pos(s, pos)
            .map(|c| c.as_ref().map(groups_of))
            .map_err(limit);
    };
    prefiltered(pre, s, pos, |i| {
        anchored
            .captures_from_pos(s, i)
            .map(|c| c.as_ref().map(groups_of))
            .map_err(limit)
    })
}

/// How much text the prefilter may spend on candidates the confirm rejects, beyond four times
/// the distance the search has covered, before [`prefiltered`] stops asking it for whole
/// matches.
const PREFILTER_SLACK: usize = 4096;

/// The first match from `pos`, trying candidate starts in order with `confirm` (the anchored
/// pattern; a full match can only start where the superset `pre` matches). `pre.find_at` scans
/// to the end of the superset's leftmost match, so when a dropped `\b` keeps rejecting long
/// superset matches (`\bsk-[\w-]{16,}` on `ask-ask-…`), asking it again from each next start
/// would be quadratic. Past [`PREFILTER_SLACK`] of such rejected text, the search goes window
/// by window instead: `pre.shortest_match_at` ends at the earliest end `e` of a superset
/// match, so the leftmost one starts at or before `e`, and `confirm` tries every start up to
/// `e`, as sre tries every start. Either way each confirm has its own backtrack budget.
fn prefiltered<T>(
    pre: &regex::Regex,
    s: &str,
    pos: usize,
    confirm: impl Fn(usize) -> Result<Option<T>, LimitError>,
) -> Result<Option<T>, LimitError> {
    let mut at = pos;
    let mut wasted = 0usize;
    while at <= s.len() {
        if wasted <= PREFILTER_SLACK + 4 * (at - pos) {
            let Some(m) = pre.find_at(s, at) else {
                return Ok(None);
            };
            if let Some(found) = confirm(m.start())? {
                return Ok(Some(found));
            }
            wasted += m.end() - m.start();
            at = next_start(s, m.start());
            continue;
        }
        let Some(e) = pre.shortest_match_at(s, at) else {
            return Ok(None);
        };
        let mut i = at;
        loop {
            if let Some(found) = confirm(i)? {
                return Ok(Some(found));
            }
            if i >= e {
                break;
            }
            i = next_start(s, i);
        }
        at = next_start(s, e);
    }
    Ok(None)
}

/// sre's `must_advance` retry: the first match, in the pattern's own priority order, that
/// starts at `pos` and is not empty. `\G(?:P)(?!\G|(?!))` anchors at `pos` and backtracks past
/// every empty way of matching, as sre does. The inner `\G` sits in an alternation because
/// fancy-regex 0.20 treats a `\G` that leads a look-ahead as leading the pattern and gives up
/// the whole search when it fails.
fn nonempty_at(aux: &Aux, s: &str, pos: usize) -> Result<Option<Groups>, LimitError> {
    let re = aux
        .nonempty
        .get_or_init(|| {
            fancy_regex::Regex::new(&format!("\\G(?:{})(?!\\G|(?!))", aux.text))
                .map_err(|e| LimitError(format!("the must-advance form: {e}")))
        })
        .as_ref()
        .map_err(Clone::clone)?;
    let groups = re
        .captures_from_pos(s, pos)
        .map_err(limit)?
        .as_ref()
        .map(groups_of);
    Ok(groups.filter(|g| g[0].is_some_and(|(a, b)| a == pos && b > a)))
}

fn limit(e: fancy_regex::Error) -> LimitError {
    LimitError(e.to_string())
}

/// One search result: the whole match's span and what the caller wants of it.
type Hit<T> = Result<((usize, usize), T), LimitError>;

/// Python's `finditer` walk (`_sre` scanner): after an empty match, the next match must not be
/// empty at the same position: sre backtracks into a lower-priority non-empty match there
/// (`a??` on `a` gives `(0,0) (0,1) (1,1)`), else searches on from the next character.
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
        mut nonempty: impl FnMut(usize) -> Result<Option<((usize, usize), T)>, LimitError>,
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
            found = match nonempty(self.pos) {
                Ok(f) => f,
                Err(e) => {
                    self.done = true;
                    return Some(Err(e));
                }
            };
            if found.is_none() {
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

/// Python's `$` outside MULTILINE: at the end, or before a final newline.
const DOLLAR: &str = "(?=\\x{a}?\\z)";

/// sre's `\b` (`c == 'b'`) or `\B` over its own `\w` (`isalnum() or "_"`, or ASCII), as
/// look-arounds: Rust's `\b` uses a different (and newer) Unicode word set. `\B` never matches
/// in an empty string, as in CPython.
fn boundary(c: char, ascii: bool) -> String {
    let w = if ascii {
        format!("[{ASCII_WORD}]")
    } else {
        W.to_string()
    };
    if c == 'b' {
        format!("(?:(?<={w})(?!{w})|(?<!{w})(?={w}))")
    } else {
        format!("(?:(?<={w})(?={w})|(?<!{w})(?!{w})(?:(?<=[\\s\\S])|(?=[\\s\\S])))")
    }
}
const ASCII_SPACE: &str = "\\x{9}-\\x{d}\\x{20}";
const ASCII_DIGIT: &str = "0-9";

#[derive(Clone, Copy, Default)]
struct Scope {
    ignorecase: bool,
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
    /// Named groups and their numbers.
    names: Vec<(String, usize)>,
    /// The open groups.
    open: Vec<Open>,
    /// Whether the last thing written can take a repeat ("nothing to repeat" otherwise).
    can_repeat: bool,
    /// Where the last atom starts in `out`, and whether fancy-regex sees it as a look-around
    /// (which it refuses to repeat; see `repeat`).
    atom_start: usize,
    atom_look: bool,
    /// Whether the last atom can match empty, and whether it holds a capture group.
    atom_nullable: bool,
    atom_captures: bool,
    /// Whether the last atom is a group with nothing in it (`(?:)`, `(?x: )`, `(?:(?:)|)`):
    /// fancy-regex refuses to repeat one, and repeating it changes nothing, so the repeat is
    /// dropped.
    atom_blank: bool,
    /// Only global flags written so far.
    at_start: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Capture,
    Plain,
    Look,
}

/// An open group.
struct Open {
    capture: Option<usize>,
    kind: Kind,
    /// Where the group starts in `out`.
    start: usize,
    /// Its top-level items so far, whether the last one is a look-around, and whether it has
    /// a top-level `|`.
    items: usize,
    last_look: bool,
    alt: bool,
    /// Whether some finished alternative can match empty; whether the items of the current
    /// one before the last can, and whether the last can; whether a capture is inside.
    nullable_alt: bool,
    nullable_before: bool,
    nullable_last: bool,
    captures: bool,
    /// Whether every item so far is a blank group (see `Parser::atom_blank`).
    blank: bool,
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
            ignorecase: flags.ignorecase,
            multiline: flags.multiline,
            verbose: flags.verbose,
            ascii: flags.ascii,
        }],
        groups: Vec::new(),
        names: Vec::new(),
        open: Vec::new(),
        can_repeat: false,
        atom_start: 0,
        atom_look: false,
        atom_nullable: false,
        atom_captures: false,
        atom_blank: false,
        at_start: true,
    };
    let mut prefix = String::new();
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
        self.scopes.last().copied().unwrap_or_default()
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
        self.atom_start = self.out.len();
        self.atom_look = false;
        self.atom_nullable = false;
        self.atom_captures = false;
        self.atom_blank = false;
        self.item(false, false, false);
        self.out.push_str(text);
    }

    /// A back-reference: an atom that may match empty.
    fn backref(&mut self, text: &str) {
        self.atom(text);
        self.atom_nullable = true;
        if let Some(top) = self.open.last_mut() {
            top.nullable_last = true;
        }
    }

    /// Counts an item of the innermost open group.
    fn item(&mut self, look: bool, nullable: bool, captures: bool) {
        if let Some(top) = self.open.last_mut() {
            top.items += 1;
            top.last_look = look;
            top.nullable_before &= top.nullable_last;
            top.nullable_last = nullable;
            top.captures |= captures;
            top.blank &= self.atom_blank;
        }
    }

    fn open_group(&mut self, text: &str, kind: Kind, capture: Option<usize>, scope: Scope) {
        self.open.push(Open {
            capture,
            kind,
            start: self.out.len(),
            items: 0,
            last_look: false,
            alt: false,
            nullable_alt: false,
            nullable_before: true,
            nullable_last: true,
            captures: false,
            blank: true,
        });
        self.scopes.push(scope);
        self.at_start = false;
        self.can_repeat = false;
        self.out.push_str(text);
    }

    /// A literal character; under IGNORECASE, the class of what CPython's sre matches for it.
    fn literal(&mut self, c: char) {
        let s = self.scope();
        let text = if s.ignorecase {
            fold::literal(c, s.ascii)
        } else if c.is_ascii_alphanumeric() {
            c.to_string()
        } else {
            hex_lit(c)
        };
        self.atom(&text);
    }

    /// A zero-width assertion: it cannot be repeated. `look` when it is written as a look-around
    /// (`$`), which matters to a repeated group around it.
    fn assertion(&mut self, text: &str, look: bool) {
        self.at_start = false;
        self.can_repeat = false;
        self.atom_blank = false;
        self.item(look, true, false);
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
                    if let Some(top) = self.open.last_mut() {
                        top.alt = true;
                        top.nullable_alt |= top.nullable_before && top.nullable_last;
                        top.nullable_before = true;
                        top.nullable_last = true;
                    }
                    self.out.push('|');
                }
                '(' => self.group()?,
                ')' => {
                    let Some(g) = self.open.pop() else {
                        return self.err("unbalanced parenthesis");
                    };
                    if let Some(n) = g.capture {
                        self.groups[n - 1] = true;
                    }
                    self.scopes.pop();
                    // The group as a whole is the atom a repeat applies to.
                    self.out.push(')');
                    self.can_repeat = true;
                    self.atom_start = g.start;
                    self.atom_look = match g.kind {
                        Kind::Look => true,
                        Kind::Plain => g.items == 1 && g.last_look && !g.alt,
                        Kind::Capture => false,
                    };
                    self.atom_nullable = g.kind == Kind::Look
                        || g.nullable_alt
                        || (g.nullable_before && g.nullable_last);
                    self.atom_captures = g.captures || g.kind == Kind::Capture;
                    self.atom_blank = g.blank && g.kind != Kind::Look;
                    let (look, nullable, captures) =
                        (self.atom_look, self.atom_nullable, self.atom_captures);
                    self.item(look, nullable, captures);
                }
                '*' | '+' | '?' => {
                    self.repeat_check()?;
                    let (min, max) = match c {
                        '*' => (0, None),
                        '+' => (1, None),
                        _ => (0, Some(1)),
                    };
                    self.repeat(&c.to_string(), min, max)?;
                }
                '{' => self.brace()?,
                '[' => self.class()?,
                '.' => self.atom("."),
                '^' => self.assertion("^", false),
                '$' => {
                    if self.scope().multiline {
                        self.assertion("$", false);
                    } else {
                        self.fancy = true;
                        self.assertion(DOLLAR, true);
                    }
                }
                '\\' => self.escape()?,
                c => self.literal(c),
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

    /// Writes a repeat (`text` is its Rust form) and its `?` (lazy) or `+` (possessive: the
    /// atomic group of the repeat) suffix.
    ///
    /// fancy-regex refuses to repeat a look-around, which Python allows. A zero-width item
    /// repeated is, in CPython's sre (`MAX_UNTIL` stops when an iteration does not advance):
    /// the item once when `min >= 1`; `(?:A|)` (greedy) or `(?:|A)` (lazy) when `min == 0`;
    /// nothing (`(?:(?!)A|)`, which keeps A's groups) when `max == 0`. That is what is written.
    fn repeat(&mut self, text: &str, min: u64, max: Option<u64>) -> Result<(), String> {
        let lazy = self.eat('?');
        let possessive = !lazy && self.eat('+');
        // CPython's sre runs one more, empty, iteration after the last one that advanced (when
        // `max` allows), and keeps the groups it sets; Rust's engines do not. So a group inside
        // a repeat whose body can match empty is refused, unless the count is fixed.
        if self.atom_captures && self.atom_nullable && max != Some(min) {
            return self
                .err("a capture group inside a repeat that can match empty is not supported");
        }
        if self.atom_blank {
            // Nothing to repeat: the group stays as written, once.
        } else if possessive {
            // `X*+` is the atomic `(?>X*)`.
            self.fancy = true;
            let a = self.out.split_off(self.atom_start);
            self.out.push_str(&format!("(?>{a}{text})"));
        } else if self.atom_look {
            let a = self.out.split_off(self.atom_start);
            let (new, still_look) = if max == Some(0) {
                (format!("(?:(?!){a}|)"), false)
            } else if min >= 1 {
                (a, true)
            } else if lazy {
                (format!("(?:|{a})"), false)
            } else {
                (format!("(?:{a}|)"), false)
            };
            self.out.push_str(&new);
            self.atom_look = still_look;
            if let Some(top) = self.open.last_mut() {
                top.last_look = still_look;
            }
        } else {
            self.out.push_str(text);
            if lazy {
                self.out.push('?');
            }
        }
        if min == 0 {
            self.atom_nullable = true;
            if let Some(top) = self.open.last_mut() {
                top.nullable_last = true;
            }
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
        let (rep, max) = match hi {
            None => (format!("{{{min}}}"), Some(min)),
            Some(h) => match parse(&h)? {
                None => (format!("{{{min},}}"), None),
                Some(max) => {
                    if max < min {
                        return self.err("min repeat greater than max repeat");
                    }
                    (format!("{{{min},{max}}}"), Some(max))
                }
            },
        };
        self.repeat(&rep, min, max)
    }

    fn group(&mut self) -> Result<(), String> {
        let scope = self.scope();
        if !self.eat('?') {
            self.groups.push(false);
            self.open_group("(", Kind::Capture, Some(self.groups.len()), scope);
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
                    if self.names.iter().any(|(n, _)| *n == name) {
                        return self.err("redefinition of group name");
                    }
                    self.groups.push(false);
                    let n = self.groups.len();
                    self.names.push((name.clone(), n));
                    self.open_group(&format!("(?P<{name}>"), Kind::Capture, Some(n), scope);
                    Ok(())
                } else if self.eat('=') {
                    let name = self.name(')')?;
                    let Some(&(_, n)) = self.names.iter().find(|(x, _)| *x == name) else {
                        return self.err("unknown group name");
                    };
                    if !self.groups[n - 1] {
                        return self.err("cannot refer to an open group");
                    }
                    if self.scope().ignorecase {
                        return self.err("a back-reference under IGNORECASE is not supported");
                    }
                    self.fancy = true;
                    self.backref(&format!("\\k<{name}>"));
                    Ok(())
                } else {
                    self.err("unknown extension ?P")
                }
            }
            '=' | '!' => {
                self.fancy = true;
                let text = if c == '=' { "(?=" } else { "(?!" };
                self.open_group(text, Kind::Look, None, scope);
                Ok(())
            }
            '<' => {
                let Some(d) = self.peek().filter(|d| matches!(d, '=' | '!')) else {
                    return self.err("unknown extension ?<");
                };
                self.i += 1;
                self.fancy = true;
                let text = if d == '=' { "(?<=" } else { "(?<!" };
                self.open_group(text, Kind::Look, None, scope);
                Ok(())
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
        self.open_group(text, Kind::Plain, None, scope);
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
        // `re._parser._parse_flags`' checks: one of `a`/`u`, a flag after `-`, and no flag
        // both turned on and off.
        let mut kind: Option<char> = None;
        let mut added: Vec<char> = Vec::new();
        let mut removed: Vec<char> = Vec::new();
        loop {
            let Some(c) = self.peek() else {
                return self.err("missing -, : or )");
            };
            self.i += 1;
            if c.is_ascii_alphabetic() {
                if neg {
                    removed.push(c);
                } else {
                    if matches!(c, 'a' | 'u') {
                        if kind.is_some_and(|k| k != c) {
                            return self
                                .err("bad inline flags: flags 'a', 'u' and 'L' are incompatible");
                        }
                        kind = Some(c);
                    }
                    added.push(c);
                }
            }
            if c == ':' && added.iter().any(|f| removed.contains(f)) {
                return self.err("bad inline flags: flag turned on and off");
            }
            match c {
                '-' if !neg => {
                    neg = true;
                    if !self.peek().is_some_and(|d| d.is_ascii_alphabetic()) {
                        return self.err("missing flag");
                    }
                }
                'L' => return self.err("bad inline flags: cannot use 'L' flag with a str pattern"),
                'a' | 'u' if neg => {
                    return self.err("bad inline flags: cannot turn off flags 'a', 'u' and 'L'");
                }
                'a' => new.ascii = true,
                'u' => new.ascii = false,
                'i' => new.ignorecase = !neg,
                's' => {
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
            'A' => self.assertion("\\A", false),
            'Z' => self.assertion("\\z", false),
            'b' | 'B' => {
                self.fancy = true;
                self.assertion(&boundary(c, ascii), true);
            }
            '0'..='9' => self.digit_escape(c)?,
            _ => {
                let lit = self.char_escape(c)?;
                self.literal(lit);
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
        if self.scope().ignorecase {
            return self.err("a back-reference under IGNORECASE is not supported");
        }
        self.fancy = true;
        // Wrapped so a following digit stays a literal.
        self.backref(&format!("(?:\\{group})"));
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
        self.literal(ch);
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
        let mut items: Vec<ClassItem> = Vec::new();
        let mut any = false;
        let scope = self.scope();
        let ascii = scope.ascii;
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
                    items.push(item);
                    items.push(ClassItem::Lit('-'));
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
                        items.push(ClassItem::Range(a, b));
                    }
                    _ => return self.err("bad character range"),
                }
            } else {
                items.push(item);
            }
        }
        let text = if scope.ignorecase {
            match fold::class(&items, negate, ascii) {
                Ok(t) => t,
                Err(e) => return self.err(&e),
            }
        } else {
            let inner: String = items.iter().map(ClassItem::text).collect();
            format!("[{}{inner}]", if negate { "^" } else { "" })
        };
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
    Range(char, char),
    /// A shorthand class's items, and whether it is negated.
    Set(&'static str, bool),
}

impl ClassItem {
    fn text(&self) -> String {
        match self {
            ClassItem::Lit(c) => hex_lit(*c),
            ClassItem::Range(a, b) => format!("{}-{}", hex_lit(*a), hex_lit(*b)),
            ClassItem::Set(items, false) => (*items).to_string(),
            ClassItem::Set(items, true) => format!("[^{items}]"),
        }
    }
}

/// `re.IGNORECASE` as CPython's `re._compiler` implements it, written out as explicit classes
/// (Rust's `(?i)` folds differently: `İ` and `ı` match `i` in Python, not in Rust).
///
/// - A literal `c` that `_sre.unicode_iscased` rejects matches only itself; otherwise a text
///   character `ch` matches when `lower(ch)` is `lower(c)` or one of its `_EXTRA_CASES`
///   (`lower` = `_sre.unicode_tolower`, the simple lowercase).
/// - A class gathers the lowercase of every literal and range member (plus their extra cases);
///   if any member is cased, `ch` matches when `lower(ch)` is in that set (or in a `\w`/`\s`/
///   `\d` member: those give the same answer for `ch` and `lower(ch)`, which the dump checks);
///   otherwise `ch` itself is tested. A range past U+FFFF is refused (CPython moves it to a
///   slower path with other rules).
/// - Under `(?a)`, only ASCII letters fold, with no extra cases.
mod fold {
    use std::collections::{BTreeSet, HashMap};
    use std::sync::OnceLock;

    use super::{ClassItem, hex_lit};
    use crate::tables;

    fn in_runs(runs: &[(u32, u32)], c: u32) -> bool {
        let i = runs.partition_point(|&(_, hi)| hi < c);
        runs.get(i).is_some_and(|&(lo, _)| lo <= c)
    }

    /// `_sre.unicode_tolower`.
    pub(super) fn lower(c: u32) -> u32 {
        if let Ok(i) = tables::SRE_LOWER.binary_search_by_key(&c, |&(k, _)| k) {
            return tables::SRE_LOWER[i].1;
        }
        match tables::LOWER.binary_search_by_key(&c, |&(k, _)| k) {
            Ok(i) => tables::LOWER[i].1,
            Err(_) => c,
        }
    }

    fn cased(c: u32, ascii: bool) -> bool {
        if ascii {
            c < 0x80 && char::from_u32(c).is_some_and(|c| c.is_ascii_alphabetic())
        } else {
            in_runs(tables::SRE_CASED, c)
        }
    }

    fn fold(c: u32, ascii: bool) -> u32 {
        if ascii {
            if (u32::from(b'A')..=u32::from(b'Z')).contains(&c) {
                c + 32
            } else {
                c
            }
        } else {
            lower(c)
        }
    }

    fn extra(lo: u32) -> &'static [u32] {
        match tables::EXTRA_CASES.binary_search_by_key(&lo, |&(k, _)| k) {
            Ok(i) => tables::EXTRA_CASES[i].1,
            Err(_) => &[],
        }
    }

    /// Every character whose `lower` is `x`, other than `x` itself.
    fn preimages() -> &'static HashMap<u32, Vec<u32>> {
        static MAP: OnceLock<HashMap<u32, Vec<u32>>> = OnceLock::new();
        MAP.get_or_init(|| {
            let mut m: HashMap<u32, Vec<u32>> = HashMap::new();
            let multi = tables::SRE_LOWER.iter().map(|&(k, v)| (k, v));
            for (k, v) in tables::LOWER.iter().copied().chain(multi) {
                if k != v {
                    m.entry(v).or_default().push(k);
                }
            }
            m
        })
    }

    /// The characters `ch` with `fold(ch)` in `set`.
    fn preimage(set: &BTreeSet<u32>, ascii: bool) -> BTreeSet<u32> {
        let mut out = BTreeSet::new();
        for &x in set {
            if fold(x, ascii) == x {
                out.insert(x);
            }
            if ascii {
                if (u32::from(b'a')..=u32::from(b'z')).contains(&x) {
                    out.insert(x - 32);
                }
            } else if let Some(v) = preimages().get(&x) {
                out.extend(v.iter().copied());
            }
        }
        out
    }

    fn runs_text(set: &BTreeSet<u32>) -> String {
        let mut out = String::new();
        let mut it = set.iter().copied().peekable();
        while let Some(a) = it.next() {
            let mut b = a;
            while it.peek() == Some(&(b + 1)) {
                b += 1;
                it.next();
            }
            let (Some(ca), Some(cb)) = (char::from_u32(a), char::from_u32(b)) else {
                continue;
            };
            if a == b {
                out.push_str(&hex_lit(ca));
            } else {
                out.push_str(&format!("{}-{}", hex_lit(ca), hex_lit(cb)));
            }
        }
        out
    }

    /// A literal under IGNORECASE.
    pub(super) fn literal(c: char, ascii: bool) -> String {
        let c = u32::from(c);
        if !cased(c, ascii) {
            return char::from_u32(c).map(hex_lit).unwrap_or_default();
        }
        let lo = fold(c, ascii);
        let mut set = BTreeSet::from([lo]);
        if !ascii {
            set.extend(extra(lo).iter().copied());
        }
        format!("[{}]", runs_text(&preimage(&set, ascii)))
    }

    /// A class under IGNORECASE.
    pub(super) fn class(items: &[ClassItem], negate: bool, ascii: bool) -> Result<String, String> {
        let mut set = BTreeSet::new();
        let mut cats = String::new();
        let mut hascased = false;
        let add = |set: &mut BTreeSet<u32>, c: u32| {
            let lo = fold(c, ascii);
            set.insert(lo);
            if !ascii {
                set.extend(extra(lo).iter().copied());
            }
            lo
        };
        for item in items {
            match item {
                ClassItem::Lit(c) => {
                    let lo = add(&mut set, u32::from(*c));
                    hascased |= cased(lo, ascii);
                }
                ClassItem::Range(a, b) => {
                    let (a, b) = (u32::from(*a), u32::from(*b));
                    if b > 0xFFFF {
                        return Err("a case-insensitive range past U+FFFF is not supported".into());
                    }
                    for c in a..=b {
                        if (0xD800..=0xDFFF).contains(&c) {
                            continue;
                        }
                        add(&mut set, c);
                        hascased |= cased(c, ascii);
                    }
                }
                ClassItem::Set(..) => cats.push_str(&item.text()),
            }
        }
        let members = if hascased { preimage(&set, ascii) } else { set };
        Ok(format!(
            "[{}{}{cats}]",
            if negate { "^" } else { "" },
            runs_text(&members)
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundary_patterns_scan_megabytes_without_hitting_the_limit() {
        // fancy-regex counts its backtrack limit over a whole search; the prefilter gives each
        // candidate start its own budget (review R2-1). A token at the very end must be found.
        let prose = "lorem ipsum dolor sit amet, consectetur adipiscing elit ".repeat(20_000);
        let text = format!("{prose}ghp_{}", "A".repeat(36));
        for p in [
            r"\bgh[pousr]_[A-Za-z0-9]+",
            r"\bxox[baprs]-",
            r"(?i)\bsk-[a-z0-9]{20,}\b",
            r"^\w+$",
        ] {
            let re = compile(p, false).unwrap();
            let spans: Result<Vec<_>, _> = re.find_iter(&text).collect();
            let spans = spans.unwrap_or_else(|e| panic!("{p}: {e}"));
            if p.starts_with(r"\bgh") {
                assert_eq!(spans, [(prose.len(), text.len())]);
            }
            assert!(re.is_match(&text).is_ok());
        }
        // Many empty matches: `\b` over a megabyte, each retry a single anchored attempt.
        let re = compile(r"\b", false).unwrap();
        let start = std::time::Instant::now();
        let n = re.find_iter(&prose[..1_000_000]).count();
        assert_eq!(n, 285_716);
        assert!(start.elapsed().as_secs() < 30, "{:?}", start.elapsed());
    }

    #[test]
    fn a_repeated_blank_group_matches_as_python() {
        // fancy-regex refuses `(?:)*` ("Target of repeat operator is invalid"); Python 3.12:
        type Case<'a> = (&'a str, &'a str, &'a [(usize, usize)]);
        let cases: [Case<'_>; 8] = [
            ("(?:)*a", "ba", &[(1, 2)]),
            ("(?:(?:)|)+?b", "ab", &[(1, 2)]),
            ("(?x: ){2}c", "c", &[(0, 1)]),
            ("(?:){3}", "ab", &[(0, 0), (1, 1), (2, 2)]),
            ("(){2}x", "ax", &[(1, 2)]),
            ("(?:)*+a", "a", &[(0, 1)]),
            ("(?i:)*A", "a", &[]),
            ("a(?:)?", "aa", &[(0, 1), (1, 2)]),
        ];
        for (p, text, want) in cases {
            for fancy in [false, true] {
                // A look-ahead that always holds here forces the fancy engine.
                let p = if fancy {
                    format!("{p}(?!\\x01)")
                } else {
                    p.to_string()
                };
                let re = compile(&p, false).unwrap_or_else(|e| panic!("{p}: {e}"));
                let spans: Vec<_> = re.find_iter(text).map(Result::unwrap).collect();
                assert_eq!(spans, want, "{p}");
            }
        }
        let re = compile("(){2}x", false).unwrap();
        let groups: Vec<_> = re.captures_iter("ax").map(Result::unwrap).collect();
        assert_eq!(groups, [vec![Some((1, 2)), Some((1, 1))]]);
    }

    #[test]
    fn rejected_long_superset_matches_stay_linear() {
        // Review R3-1: the superset (`\b` dropped) matches to the end of the text from every
        // start and the confirm rejects each one; asking `find_at` again from each next start
        // would be quadratic (minutes on a megabyte). Expectations from Python 3.12.
        let asks = "ask-".repeat(250_000);
        let abs = "ab".repeat(80_000);
        let hit = format!("{asks} sk-{}", "x".repeat(20));
        let ab_hit = format!("{abs} bc");
        let ab_end = format!("{abs}c");
        type Case<'a> = (&'a str, &'a str, &'a [(usize, usize)]);
        let cases: [Case<'_>; 5] = [
            (r"\bsk-[A-Za-z0-9_-]{16,}", &asks, &[]),
            (r"\bsk-[A-Za-z0-9_-]{16,}", &hit, &[(1_000_001, 1_000_024)]),
            (r"\bb.*c", &ab_end, &[]),
            (r"\bb.*c", &ab_hit, &[(160_001, 160_003)]),
            (r"(\bb)(.*)c", &ab_hit, &[(160_001, 160_003)]),
        ];
        let start = std::time::Instant::now();
        for (p, text, want) in cases {
            let re = compile(p, false).unwrap();
            let spans: Vec<_> = re.find_iter(text).map(Result::unwrap).collect();
            assert_eq!(spans, want, "{p}");
            let groups: Vec<_> = re.captures_iter(text).map(Result::unwrap).collect();
            assert_eq!(groups.len(), want.len(), "{p}");
            if p.starts_with('(') {
                assert_eq!(groups[0][2], Some((160_002, 160_002)));
            }
        }
        // Seconds in a debug build; the quadratic loop took minutes.
        assert!(start.elapsed().as_secs() < 20, "{:?}", start.elapsed());
    }

    #[test]
    fn walk_reports_an_empty_match_after_a_non_empty_one() {
        let r = compile("x*", false).unwrap();
        let spans: Vec<_> = r.find_iter("abxd").map(Result::unwrap).collect();
        assert_eq!(spans, [(0, 0), (1, 1), (2, 3), (3, 3), (4, 4)]);
    }
}
