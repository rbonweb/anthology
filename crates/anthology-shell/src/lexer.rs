//! The structural shell lexer: `segments()` (SH1, fix-wave H). A port of guard_rules.py's `_Lexer`,
//! `_find_backtick` and `_segments` as fix-wave H left them (heredoc delimiter words, terminators, `$'...'`
//! decoding, arithmetic, `${...}` and backtick unescaping; owner direction 2026-11-21: those fixes are Rust
//! requirements).
//!
//! Python indexes code points; this port indexes bytes of the `&str`. Every decision is taken at an ASCII
//! byte (always a char boundary), the code point after a backslash is skipped whole, and every character
//! copied into a word is copied whole, so each Python `i += 1`/`i += 2`/`find`/slice has an exact byte
//! equivalent and no slice can fall inside a character.
//!
//! Work is bounded by one budget for the whole analysis (`Work`): every scan charges the bytes it visited,
//! as Python charges characters (`_charge`). Past it the result is `incomplete` and the guard blocks.

pub(crate) mod pystr;
mod pystr_tables;
pub(crate) mod scan;
pub(crate) mod words;

use std::collections::{HashMap, HashSet};

use crate::heredoc::{find_heredoc_end, join_continuations};
use crate::lenient::lenient_segments;
use crate::limits::{Limits, MAX_DEPTH, SEGMENT_COST, extra_limit, work_limit};
use crate::shell_args::Budget;
use scan::{
    MAX_UNCLOSED, body_substitutions, bracket_pairs, scan_paren, scan_paren_ex, substitution,
};
use words::{
    HeredocWord, Memo, ansi_c, backtick_end, brace_end, heredoc_after, input_operator,
    skip_continuations, unescape_backticks,
};

/// One redirection: its operator (`>`, `2>` is `>` with the fd dropped, `&>`, `<<<`, ...) and target word.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct Redirect {
    pub op: String,
    pub target: String,
}

/// The identity of one pipeline (`_Group`): segments joined by `|` share it.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct GroupId(pub u32);

/// One simple command (`_Seg`): its words and its redirections.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct Segment {
    pub tokens: Vec<String>,
    pub redirs: Vec<Redirect>,
    pub group: GroupId,
    /// Rust-only: lexed from the text of a `$((...))` (an arithmetic expression read as commands, as Python
    /// does), so a `*` program word there is multiplication, not a glob (`opaque`).
    pub(crate) arith: bool,
    /// Rust-only: an unquoted `{` was among its words, so bash brace-expands them (SH-FW re-review F-1).
    pub(crate) braces: bool,
    /// Rust-only: the first command after a `|` that had no command before it in its pipeline, the head
    /// being a subshell (`( echo X ) | sh`, SH-FW re-review I-2a).
    pub(crate) after_compound: bool,
    /// Rust-only: a word holds a `{`, `}` or `,` that was quoted, escaped or expanded, so its brace reading
    /// is not bash's (SH-seq review C-1).
    pub(crate) quoted_braces: bool,
    /// Rust-only (G0-FP): a `(` ended it, as in `f()` or `f ()`, so a one-word segment may define a function
    /// (collect.rs `defined_names`). The lenient split, which cannot tell, sets it on every segment.
    pub(crate) defines: bool,
}

/// A pipeline's literal stdin text (heredoc bodies, here-strings).
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct Group {
    pub id: GroupId,
    pub texts: Vec<String>,
}

/// `segments()`'s result. `groups` holds exactly the groups some segment references, ascending by id.
/// `lenient`: the text had an unbalanced quote or substitution, so the whitespace fallback produced the
/// segments. `incomplete`: the work budget ran out (fail closed: the guard blocks, "too complex").
#[derive(Clone, Debug, Default, Eq, PartialEq, Hash)]
pub struct Lexed {
    pub segments: Vec<Segment>,
    pub groups: Vec<Group>,
    pub lenient: bool,
    pub incomplete: bool,
}

/// What the lexer could not see into, for SH2's `Opaque` reasons.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct LexInfo {
    /// A nested construct, or a heredoc body's substitution, was skipped at `MAX_DEPTH`.
    pub(crate) depth_capped: bool,
    /// A nested lexer fell back to the lenient split.
    pub(crate) nested_lenient: bool,
    /// SH-F2: a here-string or heredoc went to a pipeline no segment belongs to (`( sh ) <<< X`: the text
    /// after the `)`). Only for a text lexed without the lenient split.
    pub(crate) orphan_texts: bool,
    /// PE-4: the text of each arithmetic context lexed (`$((...))`, a `((...))` command, `$[...]`), where bash
    /// evaluates a variable's value as an expression, for the names it evaluates (`evaluates_values`).
    pub(crate) arith_texts: Vec<String>,
    /// PE-2, PE-3: a word runs code the lexer does not model (a zsh glob qualifier, `${x@P}`, `${(e)x}`,
    /// `${~x}`, `$~x`). Lexing goes on; the analysis is too complex at its end.
    pub(crate) unproven: bool,
    /// Rust-only: an unquoted `(` or `)` was met, anywhere in the text. The lexer ends a segment there, but zsh
    /// reads `(-o|-x)` in a word list as a glob group that expands to one of its words, so the segments before
    /// and after a paren may be one command (`docker exec c psql -c SQL (-o|-x) '|cmd'`).
    pub(crate) parens: bool,
}

/// `_TooComplex`: the work budget ran out.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TooComplex;

/// Why a lexer stopped.
pub(crate) enum LexError {
    Unbalanced,
    TooComplex,
}

impl From<TooComplex> for LexError {
    fn from(_: TooComplex) -> LexError {
        LexError::TooComplex
    }
}

/// How a heredoc body inside `$(...)` is read (`_STATE.reading`, see `heredoc::comsub_cut`): one reading per
/// pass over the command; the other two run only when a line that matters was seen.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub(crate) enum Reading {
    /// bash 5: a delimiter line with a `)` after it ends the body.
    #[default]
    Bash5,
    /// bash 3.2: only the exact terminator ends it.
    Exact,
    /// Any parenthesis after the delimiter ends it.
    Wide,
}

/// The work budgets and the reading of the pass in progress (`_STATE`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Work {
    /// Python's budget, spent where `_charge` spends it (`charge`).
    pub(crate) budget: Budget,
    /// The budget of Rust's own work (`charge_extra`, and the shell option rescans).
    pub(crate) extra: Budget,
    pub(crate) reading: Reading,
    /// A delimiter line with a parenthesis after it was seen inside `$(...)`: the other readings matter.
    pub(crate) paren_lines: bool,
    /// Rust-only: the work in progress is beyond what the reference does (input it drops), so `charge`
    /// spends the Rust-only budget (SH-FW re-review F-2).
    pub(crate) redirect: bool,
}

impl Work {
    pub(crate) fn new(budget: usize, extra: usize, reading: Reading) -> Work {
        Work {
            budget: Budget(budget),
            extra: Budget(extra),
            reading,
            paren_lines: false,
            redirect: false,
        }
    }

    /// `_charge`: spend `units` of Python's budget.
    pub(crate) fn charge(&mut self, units: usize) -> Result<(), TooComplex> {
        if self.redirect {
            return self.charge_extra(units);
        }
        if self.budget.spend(units) {
            Ok(())
        } else {
            Err(TooComplex)
        }
    }

    /// Spend `units` of the Rust-only budget.
    pub(crate) fn charge_extra(&mut self, units: usize) -> Result<(), TooComplex> {
        if self.extra.spend(units) {
            Ok(())
        } else {
            Err(TooComplex)
        }
    }
}

/// State shared by one top-level lexer and every lexer nested in it: the group arena, the budget and flags.
pub(crate) struct Shared {
    groups: Vec<Vec<String>>,
    pub(crate) work: Work,
    pub(crate) info: LexInfo,
}

impl Shared {
    pub(crate) fn new_group(&mut self) -> GroupId {
        self.groups.push(Vec::new());
        GroupId((self.groups.len() - 1) as u32)
    }

    fn push_text(&mut self, group: GroupId, text: String) {
        if let Some(texts) = self.groups.get_mut(group.0 as usize) {
            texts.push(text);
        }
    }
}

/// The byte at `i`, if any.
pub(crate) fn byte_at(text: &str, i: usize) -> Option<u8> {
    text.as_bytes().get(i).copied()
}

/// The character starting at byte `i` (a char boundary), if any.
pub(crate) fn char_at(text: &str, i: usize) -> Option<char> {
    text.get(i..)?.chars().next()
}

/// Python's `i += 2` at a backslash: past it and the whole code point after it.
pub(crate) fn skip_escape(text: &str, i: usize) -> usize {
    match char_at(text, i + 1) {
        Some(c) => i + 1 + c.len_utf8(),
        None => i + 2,
    }
}

/// `text.find(needle, from)` for an ASCII needle.
pub(crate) fn find_from(text: &str, from: usize, needle: u8) -> Option<usize> {
    let bytes = text.as_bytes().get(from..)?;
    bytes.iter().position(|&b| b == needle).map(|k| from + k)
}

/// The index just past the character at `k` (Python's `k + 1`).
fn after_char(text: &str, k: usize) -> usize {
    k + char_at(text, k).map_or(1, char::len_utf8)
}

/// `_ARRAY_ASSIGNMENT.match(word)`: `[A-Za-z_][A-Za-z0-9_]*\+?=\Z`.
fn is_array_assignment(word: &str) -> bool {
    let Some(name) = word.strip_suffix('=') else {
        return false;
    };
    let name = name.strip_suffix('+').unwrap_or(name);
    let mut bytes = name.bytes();
    matches!(bytes.next(), Some(b) if b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// PE-1: the byte after a `$` that makes it a parameter expansion (`$x`, `$1`, `$@`, `$#`, `$?`, `$$`, `$!`,
/// `$-`, and zsh's `$=x`, `$^x`, `$+x`).
fn param_start(c: u8) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            b'_' | b'@' | b'*' | b'#' | b'?' | b'$' | b'!' | b'-' | b'=' | b'^' | b'+'
        )
}

/// PE-3: a `${...}` (`part`, from the `$`) whose expansion runs code held in a value: bash's prompt
/// expansion (`${x@P}`, which runs a `$(...)` in the value), zsh's `(e)` flag (`${(e)x}`, also among other
/// flags: `${(eL)x}`) and zsh's `~` (`${~x}`, GLOB_SUBST: a glob qualifier in the value runs). Every `${`
/// nested in `part` counts too.
fn runs_value(part: &str) -> bool {
    if part.contains("@P") {
        return true;
    }
    let b = part.as_bytes();
    (0..b.len().saturating_sub(1))
        .filter(|&k| b[k] == b'$' && b[k + 1] == b'{')
        .any(|k| {
            let mut j = k + 2;
            if b.get(j) == Some(&b'(') {
                let close = (j + 1..b.len()).find(|&m| b[m] == b')').unwrap_or(b.len());
                if b[j + 1..close].contains(&b'e') {
                    return true;
                }
                j = close + 1;
            }
            while let Some(&c) = b.get(j).filter(|c| b"^=~+#!".contains(c)) {
                if c == b'~' {
                    return true;
                }
                j += 1;
            }
            false
        })
}

/// PE-3: an unquoted heredoc body expands its `${...}` as a double-quoted word does, so `runs_value` holds
/// for each of them (`${(e)x}`, `${x@P}` in the body run the value's code).
fn body_runs_value(body: &str) -> bool {
    let b = body.as_bytes();
    body.match_indices("${").any(|(k, _)| {
        let end = crate::collect::param_close(b, k).map_or(b.len(), |e| e + 1);
        runs_value(&body[k..end])
    })
}

/// PE-2: an unquoted `${...}` (`part`) whose words hold a `(` right after a word character, a quote or a `)`
/// (`${x:-a.rs(e:'cmd':)}`): zsh globs the default word, so that is a glob qualifier. Not the flags of `${(e)x}`,
/// not a `$(` and not a subscript flag `[(i)x]`.
fn default_qualifier(part: &str) -> bool {
    let b = part.as_bytes();
    (3..b.len()).any(|k| {
        b[k] == b'('
            && (b[k - 1].is_ascii_alphanumeric()
                || matches!(b[k - 1], b'_' | b'.' | b'\'' | b'"' | b')'))
    })
}

/// Carve-out (a): the directories a program word may start with, as `"$NAME/..."` or `"$NAME"/...`. Not
/// `PWD`: the working directory carries over from an earlier call, and its name can hold anything.
pub(crate) const PREFIX_NAMES: &[&str] = &[
    "HOME",
    "TMPDIR",
    "CLAUDE_PLUGIN_ROOT",
    "CLAUDE_PROJECT_DIR",
    "VIRTUAL_ENV",
];

/// Carve-out (a): the `$` at `j`, inside double quotes, is the standard form of a `PREFIX_NAMES` variable
/// (`$NAME`, no braces, no modifier, no subscript) directly followed by a `/`, inside the quotes or right
/// after the closing one. `Some(NAME)`.
fn standard_prefix(text: &str, j: usize) -> Option<&'static str> {
    let b = text.as_bytes();
    let after = (j + 1..b.len())
        .find(|&k| !(b[k].is_ascii_alphanumeric() || b[k] == b'_'))
        .unwrap_or(b.len());
    let name = &text[j + 1..after];
    let slash = match b.get(after) {
        Some(b'/') => true,
        Some(b'"') => b.get(after + 1) == Some(&b'/'),
        _ => false,
    };
    PREFIX_NAMES
        .iter()
        .find(|n| **n == name)
        .copied()
        .filter(|_| slash)
}

/// `_find_backtick`: the closing backtick at or after `i`, skipping escapes.
pub(crate) fn find_backtick(text: &str, i: usize, work: &mut Work) -> Result<usize, LexError> {
    backtick_end(text, i, work)?.ok_or(LexError::Unbalanced)
}

/// The word a process substitution stands for in its command (bash passes `/dev/fd/N`; the number is not
/// modelled, and no rule reads it).
const PROCSUB_PATH: &str = "/dev/fd/63";

/// A heredoc registered on the current line: (delimiter readings, group, quoted, `<<-`).
type Pending = (Vec<String>, GroupId, bool, bool);

/// Rust-only (SH-FW final F-8): a word of a segment that held a `$(...)` or a backtick. The word itself is ""
/// (or the text around it), so the lexer keeps the substitution here: `index` is the word's place in the
/// segment's words, `body` the text of its first substitution, `sole` that the word is nothing else.
///
/// A word that held an unquoted or double-quoted parameter expansion (`$x`, `${x}`, `"$1"`, `$@`) is a
/// holder too, with `param` set (PE-1): its value is only known at run time. `subst`: the word held a
/// `$(...)` or a backtick (`body` is then that text); a holder with only `param` has an empty body.
///
/// `glob`: the word holds an unquoted `*`, `?` or `[` (a holder with only `glob` is for the program word).
/// `prefix` (carve-out a): the word's one expansion is a standard directory prefix at its start, and the rest
/// is literal (`"$HOME/.cargo/bin/cargo"`, `"$CLAUDE_PROJECT_DIR"/bin/x`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Holder {
    pub(crate) index: usize,
    pub(crate) body: String,
    pub(crate) sole: bool,
    pub(crate) subst: bool,
    pub(crate) param: bool,
    pub(crate) glob: bool,
    pub(crate) prefix: Option<&'static str>,
}

struct Lexer<'t, 'a> {
    text: &'t str,
    depth: usize,
    sh: &'a mut Shared,
    /// The segments, each with its `Holder`s (`Lexer::holders`).
    out: Vec<(Segment, Vec<Holder>, bool)>,
    /// The current group, created on first use (Python creates one per segment break; only identity matters).
    group: Option<GroupId>,
    tokens: Vec<String>,
    redirs: Vec<Redirect>,
    cur: String,
    has_word: bool,
    /// A redirection operator waiting for its target word.
    pending: Option<&'static str>,
    heredocs: Vec<Pending>,
    /// A heredoc never closed: bash reads the rest as its body.
    swallowed: bool,
    /// A heredoc body in this text was cut mid-line, at a body line that starts with the delimiter and holds a
    /// parenthesis (`EOF is (here)`): the reading of `$(...)` that bash 5 takes. What follows is read as code in
    /// that reading only; the reading zsh shares (`Reading::Exact`) cuts nowhere and lexes the text again.
    cut: bool,
    /// Heredocs whose terminator was searched for in vain.
    unclosed: usize,
    /// False: an unclosed heredoc's body is not scanned.
    full: bool,
    /// True: substitutions inside are not lexed.
    shallow: bool,
    /// The text of a `$(...)`.
    comsub: bool,
    /// Substitution indexes an extra scan has lexed already.
    scanned: HashSet<usize>,
    /// (start, end) spans the unclosed-body scan has lexed, ascending.
    covered: Vec<(usize, usize)>,
    /// `(` depth.
    parens: isize,
    /// The `(` depth an open `((...))` command starts at.
    arith: Option<isize>,
    /// A `((` ran to the end: later ones are not looked at.
    arith_unclosed: bool,
    /// The word being built contains a quote or escape.
    quoted: bool,
    /// A `|`/`|&` with no command after it yet: a segment break there keeps the pipeline (Rust stricter,
    /// SH-FW review I-2; the oracle applies it to the reference too).
    piped: bool,
    /// An unquoted `{` was added to this segment's words (`Segment::braces`).
    braces: bool,
    /// Unquoted `{`, `}` and `,` added to the word being built (`Segment::quoted_braces`).
    marks: usize,
    /// A word of this segment held more `{`, `}` and `,` than were added unquoted.
    quoted_braces: bool,
    /// SH-F3: a word of the segment being built held a `$(` or a backtick (not `<(`/`>(`, not `$((`).
    held: bool,
    /// A `|` came with no command before it in its pipeline: the next segment is `Segment::after_compound`.
    compound: bool,
    /// Elements of the `name=( ... )` being lexed.
    array: Option<Vec<String>>,
    array_word: String,
    array_start: usize,
    memo: Memo,
    /// `_bracket_pairs(text)`, made at the first `$[`.
    brackets: Option<HashMap<usize, usize>>,
    /// The widest span `scan_expanded` took, without and with `dq`.
    expanded: [Option<(usize, usize)>; 2],
    /// A segment was over the Rust-only budget (`SEGMENT_COST`): it was not kept, and `run` fails (M-1).
    overflow: bool,
    /// Bodies of the substitutions in the word being built (`Holder`).
    subs: Vec<String>,
    /// The holders of the segment being built (`Holder`, F-8).
    holders: Vec<Holder>,
    /// PE-1: the word being built holds an unquoted or double-quoted parameter expansion (`Holder::param`).
    param: bool,
    /// The parameter expansions in the word being built (`Holder::prefix`).
    expansions: usize,
    /// The standard directory prefixes among them, each with whether it starts the word.
    prefixes: Vec<(&'static str, bool)>,
    /// The word being built holds an unquoted `*`, `?` or `[` (`Holder::glob`).
    glob: bool,
    /// The word being built holds an unquoted `{`, `}` or `,` (no `Holder::prefix` then).
    bare_braces: bool,
    /// The text of a `$((...))`: its own segments are arithmetic and run no program word
    /// (`Segment::arith`, no `Holder`). The commands of a substitution inside it are lexed by
    /// their own lexer, so they keep their program words.
    arith_text: bool,
}

impl<'t, 'a> Lexer<'t, 'a> {
    fn new(
        text: &'t str,
        depth: usize,
        full: bool,
        shallow: bool,
        comsub: bool,
        sh: &'a mut Shared,
    ) -> Self {
        Lexer {
            text,
            depth,
            sh,
            out: Vec::new(),
            group: None,
            tokens: Vec::new(),
            redirs: Vec::new(),
            cur: String::new(),
            has_word: false,
            pending: None,
            heredocs: Vec::new(),
            swallowed: false,
            cut: false,
            unclosed: 0,
            full,
            shallow,
            comsub,
            scanned: HashSet::new(),
            covered: Vec::new(),
            parens: 0,
            arith: None,
            arith_unclosed: false,
            quoted: false,
            piped: false,
            braces: false,
            marks: 0,
            quoted_braces: false,
            held: false,
            compound: false,
            array: None,
            array_word: String::new(),
            array_start: 0,
            memo: Memo::default(),
            brackets: None,
            expanded: [None, None],
            overflow: false,
            subs: Vec::new(),
            holders: Vec::new(),
            param: false,
            expansions: 0,
            prefixes: Vec::new(),
            glob: false,
            bare_braces: false,
            arith_text: false,
        }
    }

    fn group(&mut self) -> GroupId {
        match self.group {
            Some(group) => group,
            None => {
                let group = self.sh.new_group();
                self.group = Some(group);
                group
            }
        }
    }

    fn add(&mut self, chars: &str) {
        self.cur.push_str(chars);
        self.has_word = true;
    }

    fn end_word(&mut self) {
        let subs = std::mem::take(&mut self.subs);
        let param = std::mem::take(&mut self.param);
        let expansions = std::mem::take(&mut self.expansions);
        let prefixes = std::mem::take(&mut self.prefixes);
        let glob = std::mem::take(&mut self.glob);
        let bare_braces = std::mem::take(&mut self.bare_braces);
        // SH-F3 `held`: a word of the segment holds a `$(` or a backtick, whatever its role (a token, a redirect
        // target, a here-string, an array element)
        self.held |= !subs.is_empty();
        if !self.has_word {
            return;
        }
        // carve-out (a): one standard prefix starting a word that is otherwise literal (no glob, no brace)
        let prefix = match prefixes.as_slice() {
            [(name, true)] if expansions == 1 && subs.is_empty() && !glob && !bare_braces => {
                Some(*name)
            }
            _ => None,
        };
        let word = std::mem::take(&mut self.cur);
        self.has_word = false;
        self.quoted = false;
        let marks = word
            .bytes()
            .filter(|b| matches!(b, b'{' | b'}' | b','))
            .count();
        // a word with no `{` holds no brace expression (re-review: `-m "a, b"` is fine)
        self.quoted_braces |= marks > std::mem::take(&mut self.marks) && word.contains('{');
        if let Some(array) = self.array.as_mut() {
            array.push(word);
            return;
        }
        match self.pending.take() {
            None => {
                if let Some(body) = subs.first() {
                    let sole = subs.len() == 1 && word.is_empty();
                    self.holders.push(Holder {
                        index: self.tokens.len(),
                        body: body.clone(),
                        sole,
                        subst: true,
                        param,
                        glob,
                        prefix: None,
                    });
                } else if param || glob {
                    self.holders.push(Holder {
                        index: self.tokens.len(),
                        body: String::new(),
                        sole: false,
                        subst: false,
                        param,
                        glob,
                        prefix,
                    });
                }
                self.tokens.push(word)
            }
            Some("<<<") => {
                let group = self.group();
                self.sh.push_text(group, word);
            }
            Some(op) => self.redirs.push(Redirect {
                op: op.to_string(),
                target: word,
            }),
        }
    }

    /// One more kept segment: `SEGMENT_COST` of the Rust-only budget. Out of budget, `overflow` is set and no
    /// segment is kept from then on (`run` fails, so nothing is dropped silently).
    fn charge_segment(&mut self) -> bool {
        if self.overflow || self.sh.work.charge_extra(SEGMENT_COST).is_err() {
            self.overflow = true;
        }
        !self.overflow
    }

    /// End the segment; `new_group`: the pipeline ends too, unless no command followed a `|` yet (a newline,
    /// heredoc bodies, comments or blank lines after a `|` do not end it).
    fn end_seg(&mut self, mut new_group: bool) {
        let empty = !self.has_word && self.tokens.is_empty() && self.redirs.is_empty();
        if new_group && self.piped && empty {
            new_group = false;
        } else {
            self.compound |= !new_group && empty && !self.piped;
            self.piped = !new_group;
        }
        self.end_word();
        self.pending = None;
        let braces = std::mem::take(&mut self.braces);
        let quoted_braces = std::mem::take(&mut self.quoted_braces);
        // an arithmetic segment runs no program word: its substitutions are lexed and analyzed as commands anyway
        let mut holders = std::mem::take(&mut self.holders);
        if self.arith.is_some() || self.arith_text {
            holders.clear();
        }
        // SH-F3: the segment held a `$(` or a backtick (`end_word` derives it from the words' substitutions)
        let held = std::mem::take(&mut self.held);
        if (!self.tokens.is_empty() || !self.redirs.is_empty()) && self.charge_segment() {
            let group = self.group();
            let segment = Segment {
                tokens: std::mem::take(&mut self.tokens),
                redirs: std::mem::take(&mut self.redirs),
                group,
                arith: self.arith_text,
                braces,
                after_compound: std::mem::take(&mut self.compound),
                quoted_braces,
                defines: false,
            };
            self.out.push((segment, holders, held));
        }
        if new_group {
            self.group = None;
            self.compound = false;
        }
    }

    /// At the `(` of `name=(`: lex the elements as words until the closing `)`.
    fn open_array(&mut self, i: usize) -> usize {
        self.array_word = std::mem::take(&mut self.cur);
        self.has_word = false;
        self.quoted = false;
        self.array = Some(Vec::new());
        self.array_start = i + 1;
        i + 1
    }

    /// `<(cmd)` or `>(cmd)` at `i`: bash runs `cmd` and passes its path as one word. The body is lexed as a
    /// nested command, and the word continues the one being built (`x<(a)y` is one word, `2<(a)` keeps the 2).
    fn procsub(&mut self, i: usize) -> Result<usize, LexError> {
        let (inner, j) = scan_paren(self.text, i + 2, &mut self.memo, &mut self.sh.work)?;
        self.nested(inner, Some(i), true)?;
        self.add(PROCSUB_PATH);
        Ok(j)
    }

    /// A metacharacter inside an array literal: words end; `<(..)`/`>(..)` still run.
    fn array_char(&mut self, i: usize) -> Result<usize, LexError> {
        let text = self.text;
        let c = text.as_bytes()[i];
        if (c == b'<' || c == b'>') && byte_at(text, i + 1) == Some(b'(') {
            return self.procsub(i);
        }
        self.end_word();
        if c == b')' {
            self.close_array(i);
            return Ok(i + 1);
        }
        if c == b'\n' {
            return self.take_heredocs(i + 1);
        }
        Ok(i + 1)
    }

    /// The elements become the arguments of an `echo`, and `name=( ... )` stays one assignment word.
    fn close_array(&mut self, i: usize) {
        let elements = self.array.take().unwrap_or_default();
        if !elements.is_empty() && self.charge_segment() {
            let group = self.sh.new_group();
            let mut tokens = Vec::with_capacity(elements.len() + 1);
            tokens.push("echo".to_string());
            tokens.extend(elements);
            let segment = Segment {
                tokens,
                redirs: Vec::new(),
                group,
                arith: self.arith_text,
                braces: false,
                after_compound: false,
                quoted_braces: false,
                defines: false,
            };
            self.out.push((segment, Vec::new(), false));
        }
        let body = self.text.get(self.array_start..i).unwrap_or("");
        let word = format!("{}({body})", self.array_word);
        self.add(&word);
    }

    /// PE-4: an arithmetic context with this text was lexed (`LexInfo::arith_texts`).
    fn note_arith(&mut self, text: &str) -> Result<(), TooComplex> {
        self.sh.work.charge_extra(text.len() + 1)?;
        self.sh.info.arith_texts.push(text.to_string());
        Ok(())
    }

    /// Lex a substitution's text. `pos` is where it starts in this text (for the unclosed-body bookkeeping);
    /// `comsub`: it is the text of a `$(...)` / `<(...)`.
    fn nested(&mut self, inner: &str, pos: Option<usize>, comsub: bool) -> Result<(), TooComplex> {
        self.nested_text(inner, pos, comsub, false)
    }

    /// [`Lexer::nested`]; `arith`: `inner` is the text of a `$((...))` (`Lexer::arith_text`).
    fn nested_text(
        &mut self,
        inner: &str,
        pos: Option<usize>,
        comsub: bool,
        arith: bool,
    ) -> Result<(), TooComplex> {
        self.has_word = true;
        if arith {
            self.note_arith(inner)?;
        }
        if self.depth >= MAX_DEPTH {
            if !inner.is_empty() {
                self.sh.info.depth_capped = true;
            }
            return Ok(());
        }
        if self.shallow || pos.is_some_and(|p| self.scanned.contains(&p)) {
            return Ok(());
        }
        let shallow = pos.is_some_and(|p| {
            let k = self.covered.partition_point(|&(start, _)| start <= p);
            k > 0 && self.covered[k - 1].0 < p && p < self.covered[k - 1].1
        });
        let full = self.full && !self.swallowed;
        let segments = segments_in(inner, self.depth + 1, full, shallow, comsub, arith, self.sh)?;
        self.out.extend(segments);
        Ok(())
    }

    /// Text bash expands like a double-quoted string (arithmetic, `${...}`): its `$(...)` and backticks run
    /// whatever the quotes around them, so they are lexed too. `record` marks them so the command lexing of
    /// the same text does not lex them twice.
    fn scan_expanded(
        &mut self,
        start: usize,
        end: usize,
        dq: bool,
        record: bool,
    ) -> Result<(), TooComplex> {
        let slot = usize::from(dq);
        if let Some((s, e)) = self.expanded[slot]
            && s <= start
            && end <= e
        {
            return Ok(());
        }
        if self.expanded[slot].is_none_or(|(_, e)| end > e) {
            self.expanded[slot] = Some((start, end));
        }
        let has_word = self.has_word;
        let text = self.text;
        let part = text.get(start..end).unwrap_or("");
        for (pos, _, inner, comsub) in body_substitutions(part, dq, &mut self.sh.work)? {
            // a `$(` that is not a command substitution is a `$((...))` (PE-2: its text is arithmetic)
            let arith = !comsub && part[pos..].starts_with("$(");
            self.nested_text(&inner, None, comsub, arith)?;
            if record {
                self.scanned.insert(start + pos);
            }
        }
        self.has_word = has_word;
        Ok(())
    }

    /// The backtick substitution `text[i+1..j]`: lexed as written and as bash runs it.
    fn backtick(&mut self, i: usize, j: usize, dq: bool) -> Result<(), TooComplex> {
        let text = self.text;
        let raw = &text[i + 1..j];
        self.subs.push(raw.to_string()); // F-8: the word holds a substitution
        self.nested(raw, Some(i), false)?;
        if let Some(unescaped) = unescape_backticks(raw, dq, &mut self.sh.work)? {
            self.nested(&unescaped, Some(i), false)?;
        }
        Ok(())
    }

    /// The backslash-backtick at `i`, in arithmetic text. bash reads it as two characters, but zsh evaluates a
    /// subscript there (`(( a[\`cmd\`] ))`) and runs the command between this pair and the next one, so that text
    /// is lexed as a command, as the plain backtick form is. No closing pair: the analysis is too complex.
    /// Returns where the text goes on.
    fn escaped_backtick(&mut self, i: usize) -> Result<usize, TooComplex> {
        let text = self.text;
        let Some(close) = text.get(i + 2..).and_then(|rest| rest.find("\\`")) else {
            self.sh.info.unproven = true;
            return Ok(i + 2);
        };
        let body = &text[i + 2..i + 2 + close];
        self.subs.push(body.to_string()); // the word holds a substitution (F-8)
        self.nested(body, Some(i), false)?;
        Ok(i + 2 + close + 2)
    }

    fn run(mut self) -> Result<Vec<(Segment, Vec<Holder>, bool)>, LexError> {
        let text = self.text;
        let bytes = text.as_bytes();
        let n = text.len();
        let mut i = 0;
        while i < n {
            let c = bytes[i];
            match c {
                b'\\'
                    if byte_at(text, i + 1) == Some(b'`')
                        && (self.arith.is_some() || self.arith_text) =>
                {
                    i = self.escaped_backtick(i)?;
                }
                b'\\' => match char_at(text, i + 1) {
                    Some(next) => {
                        if next != '\n' {
                            // backslash-newline adds nothing
                            self.quoted = true;
                            let mut buf = [0u8; 4];
                            self.add(next.encode_utf8(&mut buf));
                        }
                        i += 1 + next.len_utf8();
                    }
                    None => i += 2,
                },
                b'\'' => {
                    self.quoted = true;
                    let j = find_from(text, i + 1, b'\'').ok_or(LexError::Unbalanced)?;
                    self.add(&text[i + 1..j]);
                    i = j + 1;
                }
                b'"' => {
                    self.quoted = true;
                    i = self.double_quoted(i)?;
                }
                b'`' => {
                    let j = find_backtick(text, i + 1, &mut self.sh.work)?;
                    self.backtick(i, j, false)?;
                    i = j + 1;
                }
                b'$' => i = self.dollar(i)?,
                b' ' | b'\t' | b'\r' => {
                    self.end_word();
                    i += 1;
                }
                b'\n' | b';' | b'&' | b'|' | b'<' | b'>' | b'(' | b')' if self.array.is_some() => {
                    i = self.array_char(i)?;
                }
                b'\n' => {
                    self.end_seg(true);
                    i = self.take_heredocs(i + 1)?;
                }
                b'#' if !self.has_word => i = find_from(text, i, b'\n').unwrap_or(n),
                b'(' if self.has_word && is_array_assignment(&self.cur) => {
                    // `names=(a b)` is an array, not a subshell
                    i = self.open_array(i);
                }
                b';' | b'(' | b')' => {
                    // PE-2: read on as before, and the analysis is too complex at its end; also after a
                    // group (`(a|b)(e:'cmd':)`). PE-1: a zsh anonymous function `() { ... } args` gets
                    // positional parameters the analysis does not put in place
                    let after_group = i > 0 && bytes[i - 1] == b')';
                    self.sh.info.unproven |=
                        c == b'(' && (self.has_word || after_group) && self.glob_qualifier(i);
                    self.sh.info.unproven |= c == b'(' && self.glob_group(i);
                    self.sh.info.unproven |= c == b'('
                        && !self.has_word
                        && self.tokens.is_empty()
                        && self.array.is_none()
                        && self.arith.is_none()
                        && !self.arith_text
                        && text[i + 1..]
                            .trim_start_matches([' ', '\t'])
                            .starts_with(')');
                    self.sh.info.parens |= c != b';';
                    self.paren(c, i)?;
                    let before = self.out.len();
                    self.end_seg(true);
                    if c == b'(' && self.out.len() > before {
                        // `f()`: the segment just ended may define a function (`Segment::defines`)
                        if let Some((segment, _, _)) = self.out.last_mut() {
                            segment.defines = true;
                        }
                    }
                    i += 1;
                }
                b'&' if byte_at(text, skip_continuations(text, i + 1)) != Some(b'>') => {
                    self.end_seg(true);
                    i += 1;
                }
                b'|' => {
                    let next = byte_at(text, i + 1);
                    let double = next == Some(b'|');
                    self.end_seg(double);
                    i += if double || next == Some(b'&') { 2 } else { 1 };
                }
                b'<' | b'>' if byte_at(text, i + 1) == Some(b'(') => i = self.procsub(i)?,
                b'<' | b'>' | b'&' => i = self.redirect(i)?,
                _ => {
                    let ch = char_at(text, i).unwrap_or('\u{fffd}');
                    self.braces |= ch == '{';
                    self.marks += usize::from(matches!(ch, '{' | '}' | ','));
                    self.bare_braces |= matches!(ch, '{' | '}' | ',');
                    self.glob |= matches!(ch, '*' | '?' | '[');
                    let mut buf = [0u8; 4];
                    self.add(ch.encode_utf8(&mut buf));
                    i += ch.len_utf8();
                }
            }
        }
        if self.array.is_some() {
            // an unclosed `name=(`: keep its elements
            self.end_word();
            self.close_array(n);
        }
        self.end_seg(true);
        if self.overflow {
            return Err(LexError::TooComplex);
        }
        Ok(self.out)
    }

    /// PE-2: the `(` at `i` comes right after a character of the word being built, so zsh reads it as a glob
    /// qualifier or pattern group (`*(e:'cmd':)`, `a.rs(+cmd)`, `"x"(#q+cmd)`), which can run code even with no
    /// glob character in the word. Not in arithmetic text (`a*(b)`), not inside `[[ ... ]]` (a pattern or
    /// regex there is matched, never globbed) and not `f()` / `f( )`, a function definition.
    fn glob_qualifier(&self, i: usize) -> bool {
        if self.arith.is_some() || self.arith_text || self.tokens.first().is_some_and(|t| t == "[[")
        {
            return false;
        }
        let rest = self.text.get(i + 1..).unwrap_or("");
        !rest.trim_start_matches([' ', '\t']).starts_with(')')
    }

    /// The `(` at `i` comes after a blank that follows words of the segment, and no grammar word makes it open a
    /// subshell or a pattern list. zsh reads `(a|b)` there as a glob group, one word that expands to any number
    /// (`docker exec c (flock|x) psql ...` runs `flock` when a file `flock` exists), so the words that follow it
    /// are not the ones the analysis reads; the lexer splits the segment at the paren all the same. Not after
    /// the grammar words that start a command (`if (cmd)`, `then (cmd)`, `! (cmd)`, `{ (cmd)`), not in a `case`
    /// pattern list (`case x in (a|b)`), not the `((` of `for ((;;))`, and not where [`Lexer::glob_qualifier`]
    /// says no. Not in the text of a heredoc body that never closed either (`swallowed`), nor after a body that
    /// a `$(...)` reading cut (`cut`): the lexer reads that text as commands too, for the rules that block, but
    /// no zsh reading runs prose such as `EOF is (here)`, and the exact reading lexes the text again.
    fn glob_group(&self, i: usize) -> bool {
        /// Words after which a `(` opens a subshell. Not `time`, which a quoted spelling makes a program.
        const STARTS: &[&str] = &[
            "if", "then", "else", "elif", "while", "until", "do", "!", "{",
        ];
        let words = &self.tokens;
        // `function name` names a definition: what follows it starts the body
        let body = match words.first() {
            Some(first) if first == "function" => words.get(2..).unwrap_or_default(),
            _ => words,
        };
        !self.has_word
            && !words.is_empty()
            && !self.swallowed
            && !self.cut
            && self.glob_qualifier(i)
            && !body.iter().all(|w| STARTS.contains(&w.as_str()))
            && !(words[0] == "case" && words.last().is_some_and(|w| w == "in"))
            && !(words.len() == 1 && words[0] == "for" && byte_at(self.text, i + 1) == Some(b'('))
    }

    /// Track `(` depth. A `((` with no word before it that closes with `))` is an arithmetic command: its
    /// `<<` is a shift, and its `$(...)` run even inside single quotes. Any other `((` is two subshells.
    fn paren(&mut self, c: u8, i: usize) -> Result<(), TooComplex> {
        if c == b'(' {
            if self.arith.is_none()
                && !self.arith_unclosed
                && !self.has_word
                && byte_at(self.text, i + 1) == Some(b'(')
            {
                let s = scan_paren_ex(self.text, i + 1, true, &mut self.memo, &mut self.sh.work)?;
                if s.closes {
                    self.arith = Some(self.parens);
                    let end = s.end.unwrap_or(self.text.len());
                    let text = self.text;
                    let inner = text.get(i + 2..end.saturating_sub(2)).unwrap_or("");
                    self.note_arith(inner)?;
                    self.scan_expanded(i + 2, end.saturating_sub(2), false, true)?;
                } else {
                    self.arith_unclosed = s.end.is_some_and(|e| e >= self.text.len());
                }
            }
            self.parens += 1;
        } else if c == b')' {
            self.parens -= 1;
            if let Some(a) = self.arith
                && self.parens <= a + 1
            {
                self.arith = None;
                // the segment ends here, inside the arithmetic: its words run no program (F-8). The words still
                // held their substitutions (F-3)
                self.held |= !self.subs.is_empty();
                self.holders.clear();
                self.subs.clear();
                self.param = false;
            }
        }
        Ok(())
    }

    /// A parameter expansion at `j`, inside double quotes: the word holds one (`Holder::param`), and a
    /// standard directory prefix is noted with whether nothing came before it (`Holder::prefix`).
    fn note_expansion(&mut self, j: usize) {
        if let Some(name) = standard_prefix(self.text, j) {
            let first = self.cur.is_empty() && self.expansions == 0 && self.subs.is_empty();
            self.prefixes.push((name, first));
        }
        self.param = true;
        self.expansions += 1;
    }

    fn double_quoted(&mut self, i: usize) -> Result<usize, LexError> {
        let text = self.text;
        let bytes = text.as_bytes();
        let n = text.len();
        self.has_word = true;
        let mut j = i + 1;
        while j < n {
            match bytes[j] {
                b'"' => return Ok(j + 1),
                b'\\' => match byte_at(text, j + 1) {
                    Some(b'\n') => j += 2,
                    Some(next @ (b'\\' | b'$' | b'"' | b'`')) => {
                        self.cur.push(next as char);
                        j += 2;
                    }
                    _ => {
                        self.cur.push('\\');
                        j += 1;
                    }
                },
                b'`' => {
                    let k = find_backtick(text, j + 1, &mut self.sh.work)?;
                    self.backtick(j, k, true)?;
                    j = k + 1;
                }
                b'$' if byte_at(text, j + 1) == Some(b'(') => {
                    let (inner, k, arith) =
                        substitution(text, j, &mut self.memo, &mut self.sh.work)?;
                    if !arith {
                        self.subs.push(inner.to_string()); // F-8: the word holds a substitution
                    }
                    // as unquoted: a `$((...))` text is arithmetic (PE-2)
                    self.nested_text(&inner, Some(j), !arith, arith)?;
                    j = k;
                }
                b'$' if byte_at(text, j + 1) == Some(b'{') => {
                    let k = self.brace_end(j)?;
                    self.scan_expanded(j + 2, k, true, false)?;
                    let after = after_char(text, k);
                    let part = text.get(j..after).unwrap_or("");
                    self.sh.info.unproven |= runs_value(part); // PE-3
                    self.note_expansion(j);
                    // as unquoted: `${...}` is never brace-expanded (re-review over-block fix)
                    self.marks += part
                        .bytes()
                        .filter(|b| matches!(b, b'{' | b'}' | b','))
                        .count();
                    self.cur.push_str(part);
                    j = after;
                }
                b'$' if byte_at(text, j + 1) == Some(b'[') => {
                    let close = self.bracket_end(j)?;
                    self.note_arith(text.get(j + 2..close.unwrap_or(n)).unwrap_or(""))?;
                    if let Some(k) = close {
                        self.scan_expanded(j + 2, k, true, true)?;
                    }
                    self.cur.push('$');
                    j += 1;
                }
                b'$' if byte_at(text, j + 1).is_some_and(param_start) => {
                    self.note_expansion(j);
                    self.cur.push('$');
                    j += 1;
                }
                _ => {
                    let ch = char_at(text, j).unwrap_or('\u{fffd}');
                    self.cur.push(ch);
                    j += ch.len_utf8();
                }
            }
        }
        Err(LexError::Unbalanced)
    }

    /// The index of the `}` closing the `${` at `i`; when it never closes, the first `}` (or the last
    /// character), as before fix-wave H.
    fn brace_end(&mut self, i: usize) -> Result<usize, TooComplex> {
        let text = self.text;
        if let Some(k) = brace_end(text, i + 2, Some(&mut self.memo), &mut self.sh.work)? {
            return Ok(k);
        }
        Ok(match find_from(text, i + 2, b'}') {
            Some(k) => k,
            // the last character's start (Python's `len(text) - 1`)
            None => text.char_indices().next_back().map_or(0, |(k, _)| k),
        })
    }

    /// The index of the `]` closing the `$[` at `i`.
    fn bracket_end(&mut self, i: usize) -> Result<Option<usize>, TooComplex> {
        if self.brackets.is_none() {
            self.brackets = Some(bracket_pairs(self.text, &mut self.sh.work)?);
        }
        Ok(self
            .brackets
            .as_ref()
            .and_then(|pairs| pairs.get(&(i + 1)).copied()))
    }

    fn dollar(&mut self, i: usize) -> Result<usize, LexError> {
        let text = self.text;
        match byte_at(text, i + 1) {
            Some(b'(') => {
                let (inner, j, arith) = substitution(text, i, &mut self.memo, &mut self.sh.work)?;
                if !arith {
                    self.subs.push(inner.to_string()); // F-8: the word holds a substitution
                }
                // an arithmetic segment runs no program word; the commands of a `$(...)` in it do (DT-4)
                self.nested_text(&inner, Some(i), !arith, arith)?;
                return Ok(j);
            }
            Some(b'{') => {
                let k = self.brace_end(i)?;
                // bash runs a `<(...)` in an unquoted `${...}`; that is not modelled here, so it is too complex
                if text
                    .get(i + 2..k)
                    .is_some_and(|body| body.contains("<(") || body.contains(">("))
                {
                    return Err(LexError::TooComplex);
                }
                self.scan_expanded(i + 2, k, false, false)?;
                let after = after_char(text, k);
                let part = text.get(i..after).unwrap_or("");
                // PE-3, and PE-2: a glob qualifier in its unquoted default word
                self.sh.info.unproven |= runs_value(part) || default_qualifier(part);
                // `${...}` is not brace-expanded, and not quoted either
                self.marks += part
                    .bytes()
                    .filter(|b| matches!(b, b'{' | b'}' | b','))
                    .count();
                self.add(part);
                self.param = true;
                self.expansions += 1;
                return Ok(after);
            }
            Some(b'[') => {
                // `$[...]`: old arithmetic syntax
                let close = self.bracket_end(i)?;
                self.note_arith(text.get(i + 2..close.unwrap_or(text.len())).unwrap_or(""))?;
                if let Some(k) = close {
                    self.scan_expanded(i + 2, k, false, true)?;
                }
            }
            // PE-3: zsh's `$~x` turns on GLOB_SUBST, so the value's glob qualifiers run code
            Some(b'~') => self.sh.info.unproven = true,
            Some(c) if param_start(c) => {
                self.param = true;
                self.expansions += 1;
            }
            Some(b'\'') => {
                self.quoted = true;
                let (decoded, end) =
                    ansi_c(text, i + 2, &mut self.sh.work)?.ok_or(LexError::Unbalanced)?;
                self.add(&decoded);
                return Ok(end);
            }
            Some(b'"') => return Ok(i + 1),
            _ => {}
        }
        self.add("$");
        Ok(i + 1)
    }

    fn redirect(&mut self, i: usize) -> Result<usize, LexError> {
        let text = self.text;
        if self.has_word
            && self.pending.is_none()
            && !self.quoted
            && !self.cur.is_empty()
            && self.cur.bytes().all(|b| b.is_ascii_digit())
        {
            // `2>file`: the fd number is not a word
            self.cur.clear();
            self.has_word = false;
        } else {
            self.end_word(); // `"2"<<`, `2>&1<<`: a word, a target
        }
        let (op, j): (&'static str, usize) = if byte_at(text, i) == Some(b'<') {
            input_operator(text, i)
        } else {
            // `>`, `>>`, `>|`, `>&`, `&>`, `&>>`
            let mut j = skip_continuations(text, i + 1);
            let amp = byte_at(text, i) == Some(b'&');
            if amp {
                j = skip_continuations(text, j + 1);
            }
            let next = byte_at(text, j);
            let op = match (amp, next) {
                (false, Some(b'>')) => ">>",
                (false, Some(b'|')) => ">|",
                (false, Some(b'&')) => ">&",
                (false, _) => ">",
                (true, Some(b'>')) => "&>>",
                (true, Some(b'|')) => "&>|",
                (true, Some(b'&')) => "&>&",
                (true, _) => "&>",
            };
            if matches!(next, Some(b'>' | b'|' | b'&')) {
                j = skip_continuations(text, j + 1);
            }
            (op, j)
        };
        if op == "<<" || op == "<<-" {
            if self.arith.is_some() {
                return Ok(j); // `((`: a shift
            }
            return self.heredoc(op, j);
        }
        if byte_at(text, j) != Some(b'(') {
            // a `(` after `>>`, `&>` and the like is a syntax error in bash; `<(`/`>(` never reach here (`procsub`)
            self.pending = Some(op);
        }
        Ok(j)
    }

    /// After `<<` / `<<-`: register the heredoc its delimiter word opens. Whether the body is literal depends
    /// on that word alone, never on the word before the operator.
    fn heredoc(&mut self, op: &str, j: usize) -> Result<usize, LexError> {
        match heredoc_after(self.text, j, &mut self.sh.work)? {
            HeredocWord::None => Ok(j),
            HeredocWord::Unclosed => Err(LexError::Unbalanced), // bash reads the rest into the word
            HeredocWord::Word {
                delimiters,
                quoted,
                subs,
                end,
            } => {
                for inner in subs {
                    // bash does not run these; lexing them only blocks more
                    self.nested(inner, None, false)?;
                }
                self.has_word = false;
                let group = self.group();
                self.heredocs.push((delimiters, group, quoted, op == "<<-"));
                Ok(end)
            }
        }
    }

    /// At the start of the line after heredoc operators: capture each body as bash reads it. A body that is
    /// never closed runs to the end of the input: it is scanned, and the lines after it are still lexed as
    /// commands too (fail closed). After `MAX_UNCLOSED` bodies that never close, later heredocs are not
    /// looked for.
    fn take_heredocs(&mut self, mut i: usize) -> Result<usize, LexError> {
        let text = self.text;
        let n = text.len();
        for (delimiters, group, quoted, strip_tabs) in std::mem::take(&mut self.heredocs) {
            if self.unclosed >= MAX_UNCLOSED {
                break;
            }
            let found = find_heredoc_end(
                text,
                i,
                &delimiters,
                strip_tabs,
                !quoted,
                self.comsub,
                &mut self.sh.work,
            )?;
            let (body_end, after) = match found {
                Some((body_end, after)) => (body_end, Some(after)),
                None => {
                    self.unclosed += 1;
                    if self.swallowed || !self.full {
                        self.swallowed = true;
                        continue;
                    }
                    self.swallowed = true;
                    (n, None)
                }
            };
            let raw = &text[i..body_end];
            let body = if quoted {
                raw.to_string()
            } else {
                join_continuations(raw, &mut self.sh.work)?
            };
            self.sh.push_text(group, body.clone());
            if !quoted {
                // PE-3: the body's `${...}` expand as a double-quoted word's do
                self.sh.info.unproven |= body_runs_value(&body);
                if self.depth < MAX_DEPTH && !self.shallow {
                    // bash runs these
                    for (pos, end, inner, comsub) in
                        body_substitutions(&body, false, &mut self.sh.work)?
                    {
                        // a `$(` that is not a command substitution is a `$((...))` (PE-2)
                        let arith = !comsub && body[pos..].starts_with("$(");
                        if arith {
                            self.note_arith(&inner)?;
                        }
                        let segments = segments_in(
                            &inner,
                            self.depth + 1,
                            self.full,
                            false,
                            comsub,
                            arith,
                            self.sh,
                        )?;
                        self.out.extend(segments);
                        if after.is_none() && body_end - i == body.len() {
                            // the rest is lexed again: see `nested`
                            self.scanned.insert(i + pos);
                            self.covered.push((i + pos, i + end));
                        }
                    }
                } else if self.depth >= MAX_DEPTH && (body.contains("$(") || body.contains('`')) {
                    self.sh.info.depth_capped = true;
                }
            }
            if let Some(after) = after {
                i = after;
                self.cut |= i < n && text.as_bytes()[i - 1] != b'\n';
            }
        }
        Ok(i)
    }
}

/// `_segments(text, depth, full, shallow, comsub)` inside one top-level call: the lexer, else the lenient
/// fallback. `arith`: `text` is the text of a `$((...))` (`Lexer::arith_text`); the lenient fallback's
/// segments are all marked arithmetic then (they carry no `Holder` either way).
pub(crate) fn segments_in(
    text: &str,
    depth: usize,
    full: bool,
    shallow: bool,
    comsub: bool,
    arith: bool,
    sh: &mut Shared,
) -> Result<Vec<(Segment, Vec<Holder>, bool)>, TooComplex> {
    sh.work.charge(text.len() + 1)?;
    let mut lexer = Lexer::new(text, depth, full, shallow, comsub, sh);
    lexer.arith_text = arith;
    match lexer.run() {
        Ok(segments) => Ok(segments),
        Err(LexError::TooComplex) => Err(TooComplex),
        Err(LexError::Unbalanced) => {
            sh.info.nested_lenient = true;
            lenient_segments(text, sh).map(|segments| {
                segments
                    .into_iter()
                    .map(|mut s| {
                        s.arith |= arith;
                        (s, Vec::new(), false)
                    })
                    .collect()
            })
        }
    }
}

/// `lex`'s result: the segments, what could not be seen into, and whether a heredoc line inside `$(...)` made
/// the other readings matter. `holders[k]` belongs to `lexed.segments[k]` (Rust-only, F-8).
pub(crate) struct LexOut {
    pub(crate) lexed: Lexed,
    pub(crate) info: LexInfo,
    pub(crate) holders: Vec<Vec<Holder>>,
    /// SH-F3: per segment of `lexed.segments`, whether it held a `$(` or a backtick.
    pub(crate) held: Vec<bool>,
}

/// `_segments(text, depth)` at the top level, spending from `work` (`analyze` threads one budget through the
/// whole analysis). Over budget: no segments and `incomplete` (Python raises `_TooComplex`, and the guard
/// blocks).
///
/// The flags are sticky: work whose output is thrown away (a nested lexer, or the whole lexer when the top
/// level falls back to lenient) still leaves `depth_capped` and `nested_lenient` set.
pub(crate) fn lex_in(text: &str, depth: usize, work: &mut Work) -> LexOut {
    let mut sh = Shared {
        groups: Vec::new(),
        work: *work,
        info: LexInfo::default(),
    };
    let result = match sh.work.charge(text.len() + 1) {
        Err(TooComplex) => Err(TooComplex),
        Ok(()) => match Lexer::new(text, depth, true, false, false, &mut sh).run() {
            Ok(segments) => Ok((segments, false)),
            Err(LexError::TooComplex) => Err(TooComplex),
            Err(LexError::Unbalanced) => {
                sh.info.nested_lenient = true;
                lenient_segments(text, &mut sh).map(|segments| {
                    let segments = segments
                        .into_iter()
                        .map(|s| (s, Vec::new(), false))
                        .collect();
                    (segments, true)
                })
            }
        },
    };
    *work = sh.work;
    let (pairs, lenient, incomplete) = match result {
        Ok((pairs, lenient)) => (pairs, lenient, false),
        Err(TooComplex) => (Vec::new(), false, true),
    };
    let mut segments = Vec::with_capacity(pairs.len());
    let mut holders = Vec::with_capacity(pairs.len());
    let mut held = Vec::with_capacity(pairs.len());
    for (segment, segment_holders, segment_held) in pairs {
        segments.push(segment);
        holders.push(segment_holders);
        held.push(segment_held);
    }
    let mut ids: Vec<u32> = segments.iter().map(|s| s.group.0).collect();
    ids.sort_unstable();
    ids.dedup();
    let mut arena = std::mem::take(&mut sh.groups);
    sh.info.orphan_texts = !lenient
        && !sh.info.nested_lenient
        && arena
            .iter()
            .enumerate()
            .any(|(k, texts)| !texts.is_empty() && ids.binary_search(&(k as u32)).is_err());
    let groups = ids
        .into_iter()
        .map(|id| Group {
            id: GroupId(id),
            texts: arena
                .get_mut(id as usize)
                .map(std::mem::take)
                .unwrap_or_default(),
        })
        .collect();
    LexOut {
        lexed: Lexed {
            segments,
            groups,
            lenient,
            incomplete,
        },
        info: sh.info,
        holders,
        held,
    }
}

/// `segments()` with explicit limits (kept for the published API; the lexer no longer uses a regex).
pub fn segments_with(text: &str, depth: usize, limits: &Limits) -> Lexed {
    let _ = limits;
    let mut work = Work::new(work_limit(text), extra_limit(text), Reading::Bash5);
    lex_in(text, depth, &mut work).lexed
}

/// Split a command string into simple commands (guard_rules.py `_segments(text, depth)`). Nested `$(...)`,
/// backticks, `( ... )`, `${...}`, arithmetic and unquoted heredoc-body substitutions are lexed recursively
/// (to `MAX_DEPTH`), and their segments come before the segment that contains them (execution order).
pub fn segments(text: &str, depth: usize) -> Lexed {
    segments_with(text, depth, &Limits::default())
}
