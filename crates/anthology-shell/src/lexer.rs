//! The structural shell lexer: `segments()` (SH1). A port of guard_rules.py's `_Lexer`, `_scan_paren`,
//! `_find_backtick` and `_segments`.
//!
//! Python indexes code points; this port indexes bytes of the `&str`. Every decision is taken at an ASCII
//! byte (always a char boundary), the code point after a backslash is skipped whole, and every character
//! copied into a word is copied whole, so each Python `i += 1`/`i += 2`/`find`/slice has an exact byte
//! equivalent and no slice can fall inside a character.

pub(crate) mod pystr;
mod pystr_tables;

use std::collections::HashSet;
use std::sync::LazyLock;

use crate::heredoc::{body_substitutions, find_heredoc_end, is_heredoc_delimiter};
use crate::lenient::lenient_segments;
use crate::limits::{DEFAULT_BACKTRACK_LIMIT, LEX_BUDGET, Limits, MAX_DEPTH};
use crate::shell_args::Budget;

/// One redirection: its operator (`>`, `2>` is `>` with the fd dropped, `&>`, `<<`, ...) and target word.
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
}

/// A pipeline's literal stdin text (heredoc bodies, here-strings).
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct Group {
    pub id: GroupId,
    pub texts: Vec<String>,
}

/// `segments()`'s result. `groups` holds exactly the groups some segment references, ascending by id.
/// `lenient`: the text had an unbalanced quote or substitution, so the whitespace fallback produced the
/// segments. `incomplete`: a regex backtrack limit was hit (fail closed: the guard blocks, design §9.5).
#[derive(Clone, Debug, Default, Eq, PartialEq, Hash)]
pub struct Lexed {
    pub segments: Vec<Segment>,
    pub groups: Vec<Group>,
    pub lenient: bool,
    pub incomplete: bool,
}

/// What the lexer could not see into, for SH2's `Opaque` reasons.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct LexInfo {
    /// A nested construct, or a heredoc body's substitution, was skipped at `MAX_DEPTH`.
    pub(crate) depth_capped: bool,
    /// A nested lexer fell back to the lenient split.
    pub(crate) nested_lenient: bool,
    /// The heredoc regex hit its backtrack limit (`incomplete` is set too; `incomplete` alone may also be the
    /// work budget).
    pub(crate) regex_limited: bool,
}

/// `_Unbalanced`: an unterminated quote or substitution.
pub(crate) struct Unbalanced;

const HEREDOC_START: &str = r#"^<<(?!<)-?[ \t]*(['"]?)([A-Za-z_][A-Za-z0-9_.-]*)\1"#;

fn heredoc_regex(limit: usize) -> Option<fancy_regex::Regex> {
    fancy_regex::RegexBuilder::new(HEREDOC_START)
        .backtrack_limit(limit)
        .build()
        .ok()
}

static HEREDOC_DEFAULT: LazyLock<Option<fancy_regex::Regex>> =
    LazyLock::new(|| heredoc_regex(DEFAULT_BACKTRACK_LIMIT));

/// State shared by one top-level lexer and every lexer nested in it: the group arena, the regex and flags.
pub(crate) struct Shared<'r> {
    re: Option<&'r fancy_regex::Regex>,
    groups: Vec<Vec<String>>,
    pub(crate) incomplete: bool,
    /// Work left for the lenient fallback (see `shell_args::Budget`).
    pub(crate) budget: Budget,
    pub(crate) info: LexInfo,
}

impl Shared<'_> {
    pub(crate) fn new_group(&mut self) -> GroupId {
        self.groups.push(Vec::new());
        GroupId((self.groups.len() - 1) as u32)
    }

    fn push_text(&mut self, group: GroupId, text: String) {
        if let Some(texts) = self.groups.get_mut(group.0 as usize) {
            texts.push(text);
        }
    }

    /// `_HEREDOC_START.match(text, j)`: (delimiter, index just past the match). A backtrack-limit error (or a
    /// regex that failed to build, which cannot happen for this constant pattern) is "no match" and sets
    /// `incomplete`.
    pub(crate) fn heredoc_start<'t>(
        &mut self,
        text: &'t str,
        j: usize,
    ) -> Option<(&'t str, usize)> {
        let Some(re) = self.re else {
            self.incomplete = true;
            self.info.regex_limited = true;
            return None;
        };
        let rest = text.get(j..)?;
        match re.captures(rest) {
            Ok(Some(caps)) => {
                let whole = caps.get(0)?;
                let name = caps.get(2)?;
                Some((&rest[name.start()..name.end()], j + whole.end()))
            }
            Ok(None) => None,
            Err(_) => {
                self.incomplete = true;
                self.info.regex_limited = true;
                None
            }
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

/// `_scan_paren`: (inner, index after `)`) for the parenthesis opened just before `text[i]`. Quotes, escapes
/// and heredoc bodies are skipped over; an unclosed one runs to the end of the text.
pub(crate) fn scan_paren<'t>(text: &'t str, i: usize, sh: &mut Shared<'_>) -> (&'t str, usize) {
    let bytes = text.as_bytes();
    let n = text.len();
    let mut depth = 1usize;
    let mut j = i;
    let mut delimiters: Vec<&'t str> = Vec::new();
    let mut unterminated: HashSet<&'t str> = HashSet::new();
    while j < n {
        let c = bytes[j];
        if c == b'\\' {
            j = skip_escape(text, j);
            continue;
        }
        if c == b'<'
            && let Some((name, end)) = sh.heredoc_start(text, j)
        {
            if !unterminated.contains(name) {
                delimiters.push(name);
            }
            j = end;
            continue;
        }
        if c == b'\n' && !delimiters.is_empty() {
            let mut pos = j + 1;
            for delimiter in delimiters.drain(..) {
                if unterminated.contains(delimiter) {
                    // a repeat on this line of one just searched in vain: the search from the same
                    // `pos` fails again (Python repeats it; the result is the same)
                    continue;
                }
                match find_heredoc_end(text, pos, delimiter) {
                    Some((_, after)) => pos = after,
                    None => {
                        unterminated.insert(delimiter);
                    }
                }
            }
            j = pos; // Python: `j = pos - 1`, then the loop's `j += 1`
            continue;
        }
        if c == b'\'' {
            j = find_from(text, j + 1, b'\'').map_or(n, |k| k + 1);
            continue;
        }
        if c == b'"' {
            j += 1;
            while j < n && bytes[j] != b'"' {
                j = if bytes[j] == b'\\' {
                    skip_escape(text, j)
                } else {
                    j + 1
                };
            }
            j += 1;
            continue;
        }
        if c == b'(' {
            depth += 1;
        } else if c == b')' {
            depth -= 1;
            if depth == 0 {
                return (&text[i..j], j + 1);
            }
        }
        j += 1;
    }
    (text.get(i..).unwrap_or(""), n)
}

/// `_find_backtick`: the closing backtick at or after `i`, skipping escapes.
pub(crate) fn find_backtick(text: &str, i: usize) -> Result<usize, Unbalanced> {
    let bytes = text.as_bytes();
    let mut j = i;
    while j < text.len() {
        match bytes[j] {
            b'\\' => j = skip_escape(text, j),
            b'`' => return Ok(j),
            _ => j += 1,
        }
    }
    Err(Unbalanced)
}

struct Lexer<'t, 'a, 'r> {
    text: &'t str,
    depth: usize,
    sh: &'a mut Shared<'r>,
    out: Vec<Segment>,
    /// The current group, created on first use (Python creates one per segment break; only identity matters).
    group: Option<GroupId>,
    tokens: Vec<String>,
    redirs: Vec<Redirect>,
    cur: String,
    has_word: bool,
    /// A redirection operator waiting for its target word.
    pending: Option<&'t str>,
    /// (delimiter, group, quoted) registered on the current line.
    heredocs: Vec<(String, GroupId, bool)>,
    /// Delimiters whose terminator was not found.
    unterminated: HashSet<String>,
    /// The word being built contains a quote or escape.
    quoted: bool,
    /// Elements of the `name=( ... )` being lexed.
    array: Option<Vec<String>>,
    array_word: String,
    array_start: usize,
}

impl<'t, 'a, 'r> Lexer<'t, 'a, 'r> {
    fn new(text: &'t str, depth: usize, sh: &'a mut Shared<'r>) -> Self {
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
            unterminated: HashSet::new(),
            quoted: false,
            array: None,
            array_word: String::new(),
            array_start: 0,
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
        if !self.has_word {
            return;
        }
        let word = std::mem::take(&mut self.cur);
        self.has_word = false;
        let quoted = std::mem::replace(&mut self.quoted, false);
        if let Some(array) = self.array.as_mut() {
            array.push(word);
            return;
        }
        match self.pending.take() {
            None => self.tokens.push(word),
            Some("<<" | "<<-") => {
                let group = self.group();
                self.heredocs.push((word, group, quoted));
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

    fn end_seg(&mut self, new_group: bool) {
        self.end_word();
        self.pending = None;
        if !self.tokens.is_empty() || !self.redirs.is_empty() {
            let group = self.group();
            self.out.push(Segment {
                tokens: std::mem::take(&mut self.tokens),
                redirs: std::mem::take(&mut self.redirs),
                group,
            });
        }
        if new_group {
            self.group = None;
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

    /// A metacharacter inside an array literal: words end; `<(..)`/`>(..)` still run.
    fn array_char(&mut self, i: usize) -> usize {
        let text = self.text;
        let c = text.as_bytes()[i];
        if (c == b'<' || c == b'>') && byte_at(text, i + 1) == Some(b'(') {
            let (inner, j) = scan_paren(text, i + 2, self.sh);
            self.nested(inner);
            return j;
        }
        self.end_word();
        if c == b')' {
            self.close_array(i);
            return i + 1;
        }
        if c == b'\n' {
            return self.take_heredocs(i + 1);
        }
        i + 1
    }

    /// The elements become the arguments of an `echo`, and `name=( ... )` stays one assignment word.
    fn close_array(&mut self, i: usize) {
        let elements = self.array.take().unwrap_or_default();
        if !elements.is_empty() {
            let group = self.sh.new_group();
            let mut tokens = Vec::with_capacity(elements.len() + 1);
            tokens.push("echo".to_string());
            tokens.extend(elements);
            self.out.push(Segment {
                tokens,
                redirs: Vec::new(),
                group,
            });
        }
        let body = self.text.get(self.array_start..i).unwrap_or("");
        let word = format!("{}({body})", self.array_word);
        self.add(&word);
    }

    fn nested(&mut self, inner: &str) {
        if self.depth < MAX_DEPTH {
            let segments = segments_inner(inner, self.depth + 1, self.sh);
            self.out.extend(segments);
        } else if !inner.is_empty() {
            self.sh.info.depth_capped = true;
        }
        self.has_word = true;
    }

    fn run(mut self) -> Result<Vec<Segment>, Unbalanced> {
        let text = self.text;
        let bytes = text.as_bytes();
        let n = text.len();
        let mut i = 0;
        while i < n {
            let c = bytes[i];
            match c {
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
                    let j = find_from(text, i + 1, b'\'').ok_or(Unbalanced)?;
                    self.add(&text[i + 1..j]);
                    i = j + 1;
                }
                b'"' => {
                    self.quoted = true;
                    i = self.double_quoted(i)?;
                }
                b'`' => {
                    let j = find_backtick(text, i + 1)?;
                    self.nested(&text[i + 1..j]);
                    i = j + 1;
                }
                b'$' => i = self.dollar(i)?,
                b' ' | b'\t' | b'\r' => {
                    self.end_word();
                    i += 1;
                }
                b'\n' | b';' | b'&' | b'|' | b'<' | b'>' | b'(' | b')' if self.array.is_some() => {
                    i = self.array_char(i);
                }
                b'\n' => {
                    self.end_seg(true);
                    i = self.take_heredocs(i + 1);
                }
                b'#' if !self.has_word => i = find_from(text, i, b'\n').unwrap_or(n),
                b'(' if self.has_word && is_array_assignment(&self.cur) => {
                    // `names=(a b)` is an array, not a subshell
                    i = self.open_array(i);
                }
                b';' | b'(' | b')' => {
                    self.end_seg(true);
                    i += 1;
                }
                b'&' if byte_at(text, i + 1) != Some(b'>') => {
                    self.end_seg(true);
                    i += 1;
                }
                b'|' => {
                    let next = byte_at(text, i + 1);
                    let double = next == Some(b'|');
                    self.end_seg(double);
                    i += if double || next == Some(b'&') { 2 } else { 1 };
                }
                b'<' | b'>' | b'&' => i = self.redirect(i),
                _ => match char_at(text, i) {
                    Some(ch) => {
                        let mut buf = [0u8; 4];
                        self.add(ch.encode_utf8(&mut buf));
                        i += ch.len_utf8();
                    }
                    None => i += 1, // unreachable: `i` is always a char boundary
                },
            }
        }
        if self.array.is_some() {
            // an unclosed `name=(`: keep its elements
            self.end_word();
            self.close_array(n);
        }
        self.end_seg(true);
        Ok(self.out)
    }

    fn double_quoted(&mut self, i: usize) -> Result<usize, Unbalanced> {
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
                    let k = find_backtick(text, j + 1)?;
                    self.nested(&text[j + 1..k]);
                    j = k + 1;
                }
                b'$' if byte_at(text, j + 1) == Some(b'(') => {
                    let (inner, after) = scan_paren(text, j + 2, self.sh);
                    self.nested(inner);
                    j = after;
                }
                b'$' if byte_at(text, j + 1) == Some(b'{') => {
                    let end = find_from(text, j + 2, b'}').map_or(n, |k| k + 1);
                    self.cur.push_str(&text[j..end]);
                    j = end;
                }
                _ => match char_at(text, j) {
                    Some(ch) => {
                        self.cur.push(ch);
                        j += ch.len_utf8();
                    }
                    None => j += 1,
                },
            }
        }
        Err(Unbalanced)
    }

    fn dollar(&mut self, i: usize) -> Result<usize, Unbalanced> {
        let text = self.text;
        let bytes = text.as_bytes();
        let n = text.len();
        match byte_at(text, i + 1) {
            Some(b'(') => {
                let (inner, j) = scan_paren(text, i + 2, self.sh);
                self.nested(inner);
                Ok(j)
            }
            Some(b'{') => {
                let end = find_from(text, i + 2, b'}').map_or(n, |k| k + 1);
                self.add(&text[i..end]);
                Ok(end)
            }
            Some(b'\'') => {
                self.quoted = true;
                let mut j = i + 2;
                let mut buf = String::new();
                while j < n && bytes[j] != b'\'' {
                    if bytes[j] == b'\\' && j + 1 < n {
                        j += 1;
                    }
                    match char_at(text, j) {
                        Some(ch) => {
                            buf.push(ch);
                            j += ch.len_utf8();
                        }
                        None => j += 1,
                    }
                }
                if j >= n {
                    return Err(Unbalanced);
                }
                self.add(&buf);
                Ok(j + 1)
            }
            Some(b'"') => Ok(i + 1),
            _ => {
                self.add("$");
                Ok(i + 1)
            }
        }
    }

    fn redirect(&mut self, i: usize) -> usize {
        let text = self.text;
        if self.has_word && pystr::is_digit_str(&self.cur) {
            // `2>file`: the fd number is not a word (`quoted` is left as Python leaves it)
            self.cur.clear();
            self.has_word = false;
        } else {
            self.end_word();
        }
        let mut j = if byte_at(text, i) == Some(b'&') {
            i + 1
        } else {
            i
        };
        if byte_at(text, j) == Some(b'>') {
            j += 1;
            if matches!(byte_at(text, j), Some(b'>' | b'|' | b'&')) {
                j += 1;
            }
        } else {
            j += 1;
            match byte_at(text, j) {
                Some(b'<') => {
                    j += 1;
                    if matches!(byte_at(text, j), Some(b'<' | b'-')) {
                        j += 1;
                    }
                }
                Some(b'>' | b'&') => j += 1,
                _ => {}
            }
        }
        if byte_at(text, j) != Some(b'(') {
            // `<(cmd)` is a substitution, not a file
            self.pending = text.get(i..j);
        }
        j
    }

    /// At the start of the line after a heredoc operator: capture each body (if closed).
    fn take_heredocs(&mut self, mut i: usize) -> usize {
        let text = self.text;
        for (delimiter, group, quoted) in std::mem::take(&mut self.heredocs) {
            if self.unterminated.contains(&delimiter) || !is_heredoc_delimiter(&delimiter) {
                continue;
            }
            let Some((body_end, after)) = find_heredoc_end(text, i, &delimiter) else {
                // later lines cannot close it either
                self.unterminated.insert(delimiter);
                continue;
            };
            let body = &text[i..body_end];
            self.sh.push_text(group, body.to_string());
            if !quoted {
                // an unquoted body runs its `$(...)`
                if self.depth < MAX_DEPTH {
                    for inner in body_substitutions(body, self.sh) {
                        let segments = segments_inner(inner, self.depth + 1, self.sh);
                        self.out.extend(segments);
                    }
                } else if body.contains("$(") || body.contains('`') {
                    self.sh.info.depth_capped = true;
                }
            }
            i = after;
        }
        i
    }
}

/// `_segments(text, depth)` inside one top-level call: the lexer, else the lenient fallback.
pub(crate) fn segments_inner(text: &str, depth: usize, sh: &mut Shared<'_>) -> Vec<Segment> {
    match Lexer::new(text, depth, sh).run() {
        Ok(segments) => segments,
        Err(Unbalanced) => {
            sh.info.nested_lenient = true;
            lenient_segments(text, sh)
        }
    }
}

/// `_segments` with its flags: `(Lexed, LexInfo)`. `LexInfo.nested_lenient` is true for a top-level
/// fallback too.
///
/// The flags are sticky: work whose output is thrown away (a nested lexer, or the whole lexer when the top
/// level falls back to lenient) still leaves `incomplete`, `depth_capped` and `nested_lenient` set. So Rust
/// may report `incomplete` (and the guard block) where Python allowed: a D-7 case, never the reverse.
pub(crate) fn lex(text: &str, depth: usize, limits: &Limits) -> (Lexed, LexInfo) {
    lex_in(text, depth, limits, &mut Budget(LEX_BUDGET))
}

/// `lex` spending from a caller's budget (`analyze` threads one budget through the whole analysis).
pub(crate) fn lex_in(
    text: &str,
    depth: usize,
    limits: &Limits,
    budget: &mut Budget,
) -> (Lexed, LexInfo) {
    let built;
    let re = if limits.backtrack_limit == DEFAULT_BACKTRACK_LIMIT {
        HEREDOC_DEFAULT.as_ref()
    } else {
        built = heredoc_regex(limits.backtrack_limit);
        built.as_ref()
    };
    let mut sh = Shared {
        re,
        groups: Vec::new(),
        incomplete: false,
        info: LexInfo::default(),
        budget: *budget,
    };
    let (segments, lenient) = match Lexer::new(text, depth, &mut sh).run() {
        Ok(segments) => (segments, false),
        Err(Unbalanced) => {
            sh.info.nested_lenient = true;
            (lenient_segments(text, &mut sh), true)
        }
    };
    let mut ids: Vec<u32> = segments.iter().map(|s| s.group.0).collect();
    ids.sort_unstable();
    ids.dedup();
    let mut arena = std::mem::take(&mut sh.groups);
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
    let lexed = Lexed {
        segments,
        groups,
        lenient,
        incomplete: sh.incomplete,
    };
    *budget = sh.budget;
    (lexed, sh.info)
}

/// `segments()` with explicit limits (a test lowers `backtrack_limit`).
pub fn segments_with(text: &str, depth: usize, limits: &Limits) -> Lexed {
    lex(text, depth, limits).0
}

/// Split a command string into simple commands (guard_rules.py `_segments(text, depth)`). Nested `$(...)`,
/// backticks, `( ... )` and unquoted heredoc-body substitutions are lexed recursively (to `MAX_DEPTH`), and
/// their segments come before the segment that contains them (execution order).
pub fn segments(text: &str, depth: usize) -> Lexed {
    segments_with(text, depth, &Limits::default())
}
