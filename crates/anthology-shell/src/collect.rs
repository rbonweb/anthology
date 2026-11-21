//! Entry collection (SH2, fix-waves H and I): guard_rules.py `_Context`, `_add_command`, `_add_alternates`,
//! `_collect_find_exec`, `_collect_fd_exec`, `_reads_stdin_as_shell`, `_reads_fd_as_shell`, `_feed_stdin`,
//! `_process_substitutions`, `_literal_output`, `_echo_text`, `_first_arg`, `_collect`, `_collect_once`, and the
//! path helpers `_expand` and `_normalize` (which the tracked `cd` directory needs; the guard reuses them, so
//! there is one implementation, G-2).
//!
//! One work budget covers the whole analysis, every reading pass included (`Work`): Python's
//! (`limits::work_limit`), spent where `_charge` spends it, and Rust's own (`limits::extra_limit`), spent on the
//! words this module stores, the texts it analyzes and the lines it builds. Over either, the analysis stops
//! and is `incomplete` (Python raises `_TooComplex`; the guard blocks, "too complex to check safely").
//!
//! Beyond Python (each sets `incomplete` or is case-scoped in the difftest):
//! - `find -exec find -exec ...` nesting is walked with an explicit stack (Python recurses and raises
//!   `RecursionError` near 500 levels) and stops past `FIND_EXEC_CAP` levels (D-1);
//! - every `fd -x`/`--exec` group ended by `;` is a command, not only the first (D-8, FW-I review R1-M2);
//! - alternate readings are all kept in `Analysis.alternates` for G1 (lead ruling Q3=A), where Python checks
//!   G1 as it goes and keeps the first that hits;
//! - SH-FW review, part B: an fd cluster ending in `x`/`X` (`-Hx`) starts an exec group (I-1); more than
//!   `MAX_FED` fed lines is too complex (I-3); a command's brace expressions give an alternate reading with
//!   them expanded (C-3); echo/printf text fed to a shell or to xargs/parallel is also fed decoded (C-4); a
//!   `[`-holding argument of `let`, `declare`, `read`, `printf -v`, `test -v`, `[[` ... runs its
//!   substitutions (C-5).
//! - SH-FW re-review: brace readings only for a segment with an unquoted `{` (`Segment::braces`, F-1), and a
//!   brace reading that changes the program word is too complex (C-3a); every fed line is read, past the
//!   reference's `MAX_FED` on the Rust-only budget (`Work::redirect`, F-2); printf's text with a leading `--`
//!   dropped and with its format filled (`printf::printf_fill`, C-4a); `printf -v<name>` attached and
//!   `unset` (C-5a); a pipeline headed by a compound command or a function call (`( echo X ) | sh`,
//!   `if ...; fi | sh`, `f | sh`) is fed the literal output of the commands before it in the text (I-2a).
//! - Fail-closed families (controller ruling after the narrow verification; `fail_closed` and the group loop
//!   of `collect_once`): a brace sequence `{x..y}` or a brace expression in a word holding whitespace (a
//!   quoted item); a `[...]` subscript holding `$(`, a backtick or `$'` in any word; an assignment, export or
//!   prefix of `TAR_OPTIONS` or `GIT_CONFIG_*` that is not the modelled prefix of `tar` / `git`, a git alias
//!   given twice or through `GIT_CONFIG_*`, `git --config-env`; a shell reading stdin from a producer that
//!   is not literal (`curl | sh`, `f | tr | sh`), from a compound command with a redirect or here-string, or
//!   from a compound with no literal output; a here-string that is only a substitution (`sh <<< "$(f)"`);
//!   `>(` with a shell; a `<(` with no literal output given to a shell; `coproc`; a shell running a file an
//!   earlier command wrote. Each is too complex (the guard blocks).

mod printf;

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use crate::analyze::Entry;
use crate::lexer::scan::{body_substitutions, scan_paren};
use crate::lexer::words::{Memo, ansi_c_decode};
use crate::lexer::{
    Group, GroupId, LexOut, Reading, Redirect, TooComplex, Work, find_from, lex_in, pystr,
    skip_escape,
};
use crate::limits::{DEFAULT_BACKTRACK_LIMIT, ENTRY_COST, Limits, MAX_CWD_DEPTH, MAX_DEPTH};
use crate::opaque::{Opaque, TEST_BRACKETS, is_dynamic, mark};
use crate::prefixes::hidden::runs_option_command;
use crate::prefixes::{
    MAX_FED, STDIN_ARG_RUNNERS, StdinRun, Stripped, UNPARSED_RUNNERS, is_assignment,
    is_interpreter, parallel_lines, pm_runs_own_verb, strip_prefixes,
};
use crate::shell_args::{
    Budget, SHELLS, ShellMode, WORD, embedded_shells_in, prog_name, script_starts_in, shell_parse,
};

const DOCKER: &[&str] = &["docker", "docker-compose"];
const CHDIR_PROGRAMS: &[&str] = &["tar", "git", "make"];
const FIND_EXEC: &[&str] = &["-exec", "-execdir", "-ok", "-okdir"];
const FD_PROGRAMS: &[&str] = &["fd", "fdfind"];
const FD_EXEC: &[&str] = &["-x", "--exec", "-X", "--exec-batch"];

/// `find -exec find -exec ...` levels walked before giving up (Python raises `RecursionError` well below).
pub(crate) const FIND_EXEC_CAP: usize = 1000;

/// A regex backtrack limit was hit (GC-16): the caller blocks.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct LimitExceeded;

/// `_HOME_VAR` and `_TILDE_USER` (fancy-regex, GC-16: both have a look-ahead).
pub(crate) struct PathRegexes {
    home_var: Option<fancy_regex::Regex>,
    tilde_user: Option<fancy_regex::Regex>,
}

impl PathRegexes {
    fn new(limit: usize) -> PathRegexes {
        let build = |pattern: &str| {
            fancy_regex::RegexBuilder::new(pattern)
                .backtrack_limit(limit)
                .build()
                .ok()
        };
        PathRegexes {
            home_var: build(r"\$\{HOME\}|\$HOME(?![A-Za-z0-9_])"),
            tilde_user: build(r"^~[A-Za-z0-9_.-]+(?=/|\z)"),
        }
    }
}

static PATH_DEFAULT: LazyLock<PathRegexes> =
    LazyLock::new(|| PathRegexes::new(DEFAULT_BACKTRACK_LIMIT));

/// Run `f` with the path regexes for `limits`.
pub(crate) fn with_path_regexes<T>(limits: &Limits, f: impl FnOnce(&PathRegexes) -> T) -> T {
    if limits.backtrack_limit == DEFAULT_BACKTRACK_LIMIT {
        f(&PATH_DEFAULT)
    } else {
        f(&PathRegexes::new(limits.backtrack_limit))
    }
}

fn expand_in(path: &str, home: &str, re: &PathRegexes) -> Result<String, LimitExceeded> {
    let mut path = path.replace('\\', "/");
    if path == "~" || path.starts_with("~/") {
        path = format!("{home}{}", &path[1..]);
    } else if re
        .tilde_user
        .as_ref()
        .ok_or(LimitExceeded)?
        .is_match(&path)
        .map_err(|_| LimitExceeded)?
    {
        path = format!("/{path}"); // another user's home: absolute
    }
    let home_var = re.home_var.as_ref().ok_or(LimitExceeded)?;
    let mut out = String::with_capacity(path.len());
    let mut last = 0;
    for found in home_var.find_iter(&path) {
        let found = found.map_err(|_| LimitExceeded)?;
        out.push_str(&path[last..found.start()]);
        out.push_str(home);
        last = found.end();
    }
    out.push_str(&path[last..]);
    Ok(out)
}

/// `posixpath.normpath` for a non-empty path.
fn normpath(path: &str) -> String {
    let initial = if path.starts_with("//") && !path.starts_with("///") {
        2
    } else if path.starts_with('/') {
        1
    } else {
        0
    };
    let mut comps: Vec<&str> = Vec::new();
    for comp in path.split('/') {
        if comp.is_empty() || comp == "." {
            continue;
        }
        if comp != ".." || (initial == 0 && comps.is_empty()) || comps.last() == Some(&"..") {
            comps.push(comp);
        } else if !comps.is_empty() {
            comps.pop();
        }
    }
    let joined = format!("{}{}", "/".repeat(initial), comps.join("/"));
    if joined.is_empty() {
        ".".to_string()
    } else {
        joined
    }
}

fn normalize_in(
    path: &str,
    home: &str,
    cwd: Option<&str>,
    re: &PathRegexes,
) -> Result<String, LimitExceeded> {
    let mut path = expand_in(path, home, re)?;
    if let Some(cwd) = cwd.filter(|c| !c.is_empty())
        && !path.starts_with('/')
    {
        path = format!("{cwd}/{path}");
    }
    Ok(if path.is_empty() {
        String::new()
    } else {
        pystr::lower(&normpath(&path))
    })
}

/// `_expand(path, home)`: backslashes to `/`, `~` and `~/` to `home`, `~user/...` to `/~user/...`, and every
/// `$HOME`/`${HOME}` to `home`.
pub fn expand_home(path: &str, home: &str) -> Result<String, LimitExceeded> {
    expand_with(path, home, &Limits::default())
}

/// `expand_home` with explicit limits.
pub fn expand_with(path: &str, home: &str, limits: &Limits) -> Result<String, LimitExceeded> {
    with_path_regexes(limits, |re| expand_in(path, home, re))
}

/// `_normalize(path, home, cwd)`: expanded, normalized (`posixpath.normpath`), lowercase path; a relative
/// path resolves against a non-empty `cwd`. An empty expansion stays empty.
pub fn normalize_path(path: &str, home: &str, cwd: Option<&str>) -> Result<String, LimitExceeded> {
    normalize_with(path, home, cwd, &Limits::default())
}

/// `normalize_path` with explicit limits.
pub fn normalize_with(
    path: &str,
    home: &str,
    cwd: Option<&str>,
    limits: &Limits,
) -> Result<String, LimitExceeded> {
    with_path_regexes(limits, |re| normalize_in(path, home, cwd, re))
}

/// `_echo_args_text`: an `echo`/`printf`'s words as the text it writes (leading `-n`/`-e`/`-E` words dropped,
/// `\n` a newline).
fn echo_text(args: &[String]) -> String {
    echo_words(args).join(" ").replace("\\n", "\n")
}

/// The words an `echo`/`printf` writes: leading `-n`/`-e`/`-E` words dropped.
fn echo_words(args: &[String]) -> &[String] {
    let flag = |a: &String| {
        a.strip_prefix('-')
            .is_some_and(|r| !r.is_empty() && r.bytes().all(|b| matches!(b, b'n' | b'e' | b'E')))
    };
    let start = args.iter().position(|a| !flag(a)).unwrap_or(args.len());
    &args[start..]
}

/// `\0nnn` (echo -e, printf's `%b`) as `$'...'`'s `\nnn`.
fn echo_octal(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let (mut last, mut j) = (0, 0);
    while j + 1 < b.len() {
        if b[j] != b'\\' {
            j += 1;
            continue;
        }
        if b[j + 1] == b'0' && b.get(j + 2).is_some_and(|c| (b'0'..=b'7').contains(c)) {
            out.push_str(&text[last..=j]);
            last = j + 2;
        }
        j += 2;
    }
    out.push_str(&text[last..]);
    out
}

/// SH-FW review C-4 (Rust stricter, D-8): what an `echo`/`printf` writes with its escapes decoded, as bash 5's
/// `$'...'` reads them and with `\0nnn` read as echo -e and printf's `%b` read it; for printf also its
/// arguments after the format alone (`printf '%b' X`, `printf '%s' X`), and (re-review C-4a) its words
/// without a leading `--` and its format filled from its arguments. Each reading once, in that order.
fn echo_decoded(prog: &str, args: &[String], work: &mut Work) -> Result<Vec<String>, TooComplex> {
    let words = echo_words(args);
    let mut sources = vec![words.join(" ")];
    if prog == "printf" {
        let rest = match words.first() {
            Some(w) if w == "--" => &words[1..],
            _ => words,
        };
        if rest.len() > 1 {
            sources.push(rest[1..].join(" "));
        }
        // SH-FW re-review C-4a: the words without the `--`, and the text the format writes filled
        if rest.len() < words.len() {
            sources.push(rest.join(" "));
        }
        if let Some((format, values)) = rest.split_first() {
            sources.push(printf::printf_fill(format, values, work)?);
        }
    }
    let mut out: Vec<String> = Vec::new();
    for source in sources {
        for decoded in [
            ansi_c_decode(&source, false, work)?,
            ansi_c_decode(&echo_octal(&source), false, work)?,
        ] {
            if !out.contains(&decoded) {
                out.push(decoded);
            }
        }
    }
    Ok(out)
}

/// SH-FW review C-5 (Rust stricter, D-8): programs that evaluate an arithmetic subscript in their arguments
/// (`let 'a[$(cmd)]=1'`), and those that do once a `-v` is among them (`printf -v`, `test -v`).
const SUBSCRIPT_PROGRAMS: &[&str] = &[
    "let", "declare", "typeset", "local", "readonly", "export", "read", "[[", "unset",
];
const SUBSCRIPT_V_PROGRAMS: &[&str] = &["printf", "test", "["];

/// Words that the brace expressions of one command may give (`{a,b}{c,d}` gives 4), and the brace levels
/// one word may nest; past either it is too complex.
const MAX_BRACE_WORDS: usize = 4096;
const MAX_BRACE_DEPTH: usize = 64;

/// SH-seq re-review R-1: the index after the `}` closing the `${` at `j` (`word[j]` is `$`), or the word's
/// end. Bash does not brace-expand inside `${...}`, so the brace scanners skip it whole.
fn param_end(b: &[u8], j: usize) -> usize {
    // quotes and escapes inside `${...}` hide their braces (#89 review F-4), as in the lexer's `brace_scan`
    let mut depth = 0usize;
    let mut k = j + 1;
    while k < b.len() {
        match b[k] {
            b'\\' => k += 1,
            q @ (b'\'' | b'"') => {
                k += 1;
                while k < b.len() && b[k] != q {
                    k += usize::from(q == b'"' && b[k] == b'\\') + 1;
                }
            }
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return k + 1;
                }
            }
            _ => {}
        }
        k += 1;
    }
    b.len()
}

/// `word[j]` starts a `${`.
fn param_at(b: &[u8], j: usize) -> bool {
    b[j] == b'$' && b.get(j + 1) == Some(&b'{')
}

/// The first brace expression of `word`, as (open, close, commas): a `{` (not `${`) with its matching `}` and
/// a comma at its own level between them. One whose text holds a quote character is not one: past quote
/// removal, that was quoted text (a JSON body).
fn brace_at(word: &str) -> Option<(usize, usize, Vec<usize>)> {
    struct Open {
        at: usize,
        param: bool,
        quote: bool,
        commas: Vec<usize>,
    }
    let b = word.as_bytes();
    let mut stack: Vec<Open> = Vec::new();
    let mut first: Option<(usize, usize, Vec<usize>)> = None;
    let mut skip = 0;
    for (j, &c) in b.iter().enumerate() {
        if j < skip {
            continue;
        }
        if param_at(b, j) {
            skip = param_end(b, j);
            continue;
        }
        match c {
            b'{' => stack.push(Open {
                at: j,
                param: j > 0 && b[j - 1] == b'$',
                quote: false,
                commas: Vec::new(),
            }),
            b',' => {
                if let Some(top) = stack.last_mut() {
                    top.commas.push(j);
                }
            }
            b'"' | b'\'' => {
                if let Some(top) = stack.last_mut() {
                    top.quote = true;
                }
            }
            b'}' => {
                if let Some(open) = stack.pop() {
                    if let Some(parent) = stack.last_mut() {
                        parent.quote |= open.quote;
                    }
                    if !open.param
                        && !open.quote
                        && !open.commas.is_empty()
                        && first.as_ref().is_none_or(|(at, _, _)| open.at < *at)
                    {
                        first = Some((open.at, j, open.commas));
                    }
                }
            }
            _ => {}
        }
    }
    first
}

/// Bash's brace expansion of `word`, appended to `out` in bash's order, on the Rust-only budget. Too complex
/// past `MAX_BRACE_WORDS` words in `out`, or `MAX_BRACE_DEPTH` levels deep.
fn expand_braces(
    word: &str,
    level: usize,
    out: &mut Vec<String>,
    work: &mut Work,
) -> Result<(), TooComplex> {
    work.charge_extra(WORD + 2 * word.len())?;
    let comma = brace_at(word);
    let sequence = brace_sequences(word).into_iter().find_map(|(open, close)| {
        int_sequence(&word[open + 1..close]).map(|seq| (open, close, seq))
    });
    if let Some((open, close, seq)) = sequence
        && comma.as_ref().is_none_or(|(at, _, _)| open < *at)
    {
        if level >= MAX_BRACE_DEPTH {
            return Err(TooComplex);
        }
        work.charge_extra(seq.count * (WORD + seq.len))?;
        let (pre, post) = (&word[..open], &word[close + 1..]);
        for number in seq.words() {
            expand_braces(&format!("{pre}{number}{post}"), level + 1, out, work)?;
        }
        return Ok(());
    }
    let Some((open, close, commas)) = comma else {
        if out.len() >= MAX_BRACE_WORDS {
            return Err(TooComplex);
        }
        out.push(word.to_string());
        return Ok(());
    };
    if level >= MAX_BRACE_DEPTH {
        return Err(TooComplex);
    }
    let (pre, post) = (&word[..open], &word[close + 1..]);
    let mut start = open + 1;
    for end in commas.into_iter().chain([close]) {
        let alternative = format!("{pre}{}{post}", &word[start..end]);
        expand_braces(&alternative, level + 1, out, work)?;
        start = end + 1;
    }
    Ok(())
}

/// SH-FW review C-3 (Rust stricter, D-8): `tokens` with every brace expression expanded and the empty words
/// that gives dropped, or `None` when no token has one. Only for a segment with an unquoted `{` (re-review
/// F-1); its tokens are past quote removal, so a quoted `{a,b}` beside it is expanded too (the reading is an
/// alternate, checked against G1 only).
fn brace_reading(tokens: &[String], work: &mut Work) -> Result<Option<Vec<String>>, TooComplex> {
    let mut words = Vec::new();
    let mut reading = Vec::new();
    let mut found = false;
    for token in tokens {
        if !token.contains('{') {
            reading.push(token.clone());
            continue;
        }
        work.charge_extra(token.len())?;
        if brace_at(token).is_none() && brace_sequences(token).is_empty() {
            reading.push(token.clone());
            continue;
        }
        found = true;
        let before = words.len();
        expand_braces(token, 0, &mut words, work)?;
        reading.extend(words[before..].iter().filter(|w| !w.is_empty()).cloned());
    }
    Ok(found.then_some(reading))
}

/// C-3a: a program that a brace-built command word must not become unseen: it writes text a pipe may run,
/// reads stdin as commands, or runs its arguments (a peeled wrapper counts too).
fn brace_risky(prog: &str) -> bool {
    SHELLS.contains(&prog)
        || is_interpreter(prog)
        || matches!(
            prog,
            "echo"
                | "printf"
                | "cat"
                | "tee"
                | "xargs"
                | "parallel"
                | "eval"
                | "source"
                | "."
                | "exec"
        )
}

/// Programs whose `NAME=VALUE` arguments set variables for later commands.
const EXPORTERS: &[&str] = &["export", "declare", "typeset", "readonly", "local"];

/// Variables whose value is a command line git, a pager or rsync runs (`prefixes`' `COMMAND_VARIABLES`).
const COMMAND_ENV: &[&str] = &[
    "GIT_SSH_COMMAND",
    "GIT_SSH",
    "GIT_PAGER",
    "GIT_EDITOR",
    "GIT_SEQUENCE_EDITOR",
    "GIT_EXTERNAL_DIFF",
    "GIT_ASKPASS",
    "SSH_ASKPASS",
    "GIT_PROXY_COMMAND",
    "PAGER",
    "EDITOR",
    "VISUAL",
    "RSYNC_RSH",
];

/// Fail closed (controller ruling): words whose bash expansion the analysis does not model. `braces`: the
/// segment had an unquoted `{`; `quoted_braces`: one of its words held a quoted or escaped `{`, `}` or `,`.
fn fail_closed(tokens: &[String], braces: bool, quoted_braces: bool) -> bool {
    if braces && mixed_braces(tokens, quoted_braces) {
        return true;
    }
    tokens.iter().any(|t| {
        (braces
            && t.contains('{')
            && (letter_sequence(t) || brace_item_space(t) || sequence_nested_comma(t)))
            || subscript_runs(t)
    })
}

/// SH-seq review C-1: the brace reading expands every brace past quote removal, so it is bash's only when
/// nothing brace-like was quoted, and (interim rule) when an integer sequence is the only brace expression of
/// the command. Otherwise neither reading need be bash's: too complex.
fn mixed_braces(tokens: &[String], quoted_braces: bool) -> bool {
    let count: usize = tokens.iter().map(|t| brace_expressions(t)).sum();
    count > 0
        && (quoted_braces || (count > 1 && tokens.iter().any(|t| !brace_sequences(t).is_empty())))
}

/// The brace expressions of `word` (not `${`): braces holding a comma at their own level, or a `..`.
fn brace_expressions(word: &str) -> usize {
    let b = word.as_bytes();
    let mut stack: Vec<(usize, bool)> = Vec::new(); // (open, a comma at its level)
    let mut count = 0;
    let mut skip = 0;
    for (j, &c) in b.iter().enumerate() {
        if j < skip {
            continue;
        }
        if param_at(b, j) {
            skip = param_end(b, j);
            continue;
        }
        match c {
            b'{' => stack.push((j, false)),
            b',' => {
                if let Some(top) = stack.last_mut() {
                    top.1 = true;
                }
            }
            b'}' => {
                if let Some((open, comma)) = stack.pop()
                    && (open == 0 || b[open - 1] != b'$')
                    && (comma || word[open + 1..j].contains(".."))
                {
                    count += 1;
                }
            }
            _ => {}
        }
    }
    count
}

/// SH-seq review I-1: a `..` brace holding a comma brace (`{-1..{1,}}`): bash reads the comma first, then no
/// sequence; this analysis would read a sequence. Too complex.
fn sequence_nested_comma(word: &str) -> bool {
    let b = word.as_bytes();
    let mut stack: Vec<(usize, bool, bool)> = Vec::new(); // (open, comma at its level, a comma deeper)
    let mut skip = 0;
    for (j, &c) in b.iter().enumerate() {
        if j < skip {
            continue;
        }
        if param_at(b, j) {
            skip = param_end(b, j);
            continue;
        }
        match c {
            b'{' => stack.push((j, false, false)),
            b',' => {
                if let Some(top) = stack.last_mut() {
                    top.1 = true;
                }
            }
            b'}' => {
                if let Some((open, comma, deeper)) = stack.pop() {
                    if deeper && !comma && word[open + 1..j].contains("..") {
                        return true;
                    }
                    if let Some(parent) = stack.last_mut() {
                        parent.2 |= comma || deeper;
                    }
                }
            }
            _ => {}
        }
    }
    false
}

/// A brace expression whose items hold whitespace (a quoted item, `{'a b',c}`); whitespace elsewhere in the
/// word (`Application\ Support/{a,b}`) is fine.
fn brace_item_space(word: &str) -> bool {
    brace_at(word).is_some_and(|(open, close, _)| word[open..=close].contains(char::is_whitespace))
}

/// The brace sequences `{x..y}` of `word` (not `${`), as (open, close), in order.
fn brace_sequences(word: &str) -> Vec<(usize, usize)> {
    let b = word.as_bytes();
    let mut out = Vec::new();
    let mut open = None;
    let mut skip = 0;
    for (j, &c) in b.iter().enumerate() {
        if j < skip {
            continue;
        }
        if param_at(b, j) {
            skip = param_end(b, j);
            continue;
        }
        match c {
            b'{' => open = (j == 0 || b[j - 1] != b'$').then_some(j),
            b',' => open = None,
            b'}' => {
                if let Some(at) = open.take()
                    && word[at + 1..j].contains("..")
                {
                    out.push((at, j));
                }
            }
            _ => {}
        }
    }
    out
}

/// A brace sequence other than an integer one (`{a..e}`, `{1..x}`): too complex (controller ruling).
fn letter_sequence(word: &str) -> bool {
    brace_sequences(word)
        .into_iter()
        .any(|(open, close)| int_sequence(&word[open + 1..close]).is_none())
}

/// An integer sequence (`1..5`, `01..10`, `0..20..5`, `5..1`): its bounds, read without building its words
/// (SH-seq review M-1).
struct IntSequence {
    start: i64,
    end: i64,
    step: u64,
    width: usize,
    count: usize,
    /// The longer endpoint's length: no word is longer.
    len: usize,
}

impl IntSequence {
    /// Its words, as bash 5 writes them: zero-padded to the wider endpoint when either has a leading zero.
    fn words(&self) -> Vec<String> {
        let (start, end, step, width) = (self.start, self.end, self.step, self.width);
        let mut out = Vec::with_capacity(self.count);
        let mut n = start;
        for _ in 0..self.count {
            out.push(if n < 0 {
                format!("-{:0w$}", n.unsigned_abs(), w = width.saturating_sub(1))
            } else {
                format!("{n:0width$}")
            });
            n = if start <= end {
                n.saturating_add(step as i64)
            } else {
                n.saturating_sub(step as i64)
            };
        }
        out
    }
}

/// `inner` read as an integer sequence; the step's sign is ignored. `None` when it is not one, or would give
/// more than `MAX_BRACE_WORDS` words.
fn int_sequence(inner: &str) -> Option<IntSequence> {
    let parts: Vec<&str> = inner.split("..").collect();
    let int = |p: &str| -> Option<i64> {
        let digits = p.strip_prefix('-').unwrap_or(p);
        if digits.is_empty() || digits.len() > 18 || !digits.bytes().all(|d| d.is_ascii_digit()) {
            return None;
        }
        p.parse().ok()
    };
    let (start, end, step) = match parts.as_slice() {
        [a, b] => (int(a)?, int(b)?, 1),
        [a, b, s] => (int(a)?, int(b)?, int(s)?.unsigned_abs().max(1)),
        _ => return None,
    };
    let padded = |p: &str| {
        let d = p.strip_prefix('-').unwrap_or(p);
        d.len() > 1 && d.starts_with('0')
    };
    let width = if padded(parts[0]) || padded(parts[1]) {
        parts[0].len().max(parts[1].len())
    } else {
        0
    };
    let count = start.abs_diff(end) / step + 1;
    if count > MAX_BRACE_WORDS as u64 {
        return None;
    }
    Some(IntSequence {
        start,
        end,
        step,
        width,
        count: count as usize,
        len: parts[0].len().max(parts[1].len()),
    })
}

/// A `[...]` subscript holding `$(`, a backtick or `$'`: bash evaluates it (arithmetic), running the
/// substitution even where it was quoted.
fn subscript_runs(word: &str) -> bool {
    // one pass: a substitution start with a `[` before it and no `]` between
    let b = word.as_bytes();
    let mut open = false;
    for (j, &c) in b.iter().enumerate() {
        match c {
            b'[' => open = true,
            b']' => open = false,
            b'`' if open => return true,
            b'$' if open && matches!(b.get(j + 1), Some(b'(' | b'\'')) => return true,
            _ => {}
        }
    }
    false
}

/// A text that is only a command substitution (`$(f)`, `` `f` ``): what it gives is a command's output.
fn only_substitution(text: &str) -> bool {
    let t = pystr::strip_ws(text);
    t.is_empty()
        || (t.starts_with("$(") && t.ends_with(')'))
        || (t.len() > 1 && t.starts_with('`') && t.ends_with('`'))
}

/// Fail closed: `TAR_OPTIONS` / `GIT_CONFIG_*` set (in `prefix`, the words before the program, or as an
/// export's argument) anywhere but as the modelled prefix of `tar` / `git`, a git alias from `GIT_CONFIG_*`
/// or given twice, and `git --config-env`.
fn option_env_unmodelled(prefix: &[String], prog: &str, args: &[String]) -> bool {
    let mut aliases: HashSet<String> = HashSet::new();
    let mut twice = false;
    for k in 0..args.len().saturating_sub(1) {
        if prog == "git"
            && args[k] == "-c"
            && let Some((key, _)) = args[k + 1].split_once('=')
            && pystr::lower(key).starts_with("alias.")
        {
            twice |= !aliases.insert(pystr::lower(key));
        }
    }
    if twice || (prog == "git" && args.iter().any(|a| a.starts_with("--config-env"))) {
        return true;
    }
    let exported = if EXPORTERS.contains(&prog) { args } else { &[] };
    prefix.iter().chain(exported).any(|word| {
        let word = word.as_str();
        let Some((name, value)) = word.split_once('=').filter(|_| is_assignment(word)) else {
            return false;
        };
        if name == "TAR_OPTIONS" {
            return !matches!(prog, "tar" | "gtar" | "bsdtar");
        }
        if name.starts_with("GIT_CONFIG_") {
            return prog != "git" || pystr::lower(value).starts_with("alias.");
        }
        false
    })
}

/// `_first_arg`: the first argument that is not an option.
fn first_arg(args: &[String]) -> Option<&str> {
    args.iter()
        .find(|a| !a.starts_with('-'))
        .map(String::as_str)
}

/// `_STDIN_PATH`: a script file that is the shell's stdin.
fn is_stdin_path(path: &str) -> bool {
    matches!(path, "/dev/stdin" | "/dev/fd/0" | "/proc/self/fd/0")
}

/// SH-seq I-2: a text that is only a substitution of a stdin reader: `$(cat)`, `` `cat -` ``, `$(</dev/stdin)`,
/// `$(cat /dev/stdin)` (`cat` with no file operand, or a stdin redirect alone).
fn reads_stdin_substitution(text: &str) -> bool {
    let t = pystr::strip_ws(text);
    let inner = match t.strip_prefix("$(").and_then(|r| r.strip_suffix(')')) {
        Some(body) => body,
        None => match t.strip_prefix('`').and_then(|r| r.strip_suffix('`')) {
            Some(body) => body,
            None => return false,
        },
    };
    let inner = pystr::strip_ws(inner);
    if let Some(path) = inner.strip_prefix('<') {
        return is_stdin_path(pystr::strip_ws(path));
    }
    let words = pystr::split_ws(inner);
    words.first() == Some(&"cat")
        && words[1..]
            .iter()
            .all(|w| *w == "-" || (w.starts_with('-') && w.len() > 1) || is_stdin_path(w))
}

/// `_FD_PATH`: a numbered file descriptor.
fn is_fd_path(path: &str) -> bool {
    ["/dev/fd/", "/proc/self/fd/"].iter().any(|prefix| {
        path.strip_prefix(prefix)
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    })
}

/// The commands `find -exec`/`-execdir`/`-ok`/`-okdir` (each ended by `;` or `+`) or fd's `-x`/`--exec`/`-X`/
/// `--exec-batch` (ended by `;`) run. Rust stricter (FW-I review R1-M2): every fd group, not only the first;
/// and (SH-FW review I-1, D-8) a cluster of fd's no-value flags ending in `x`/`X` (`-Hx`) starts one too.
fn exec_payloads(prog: &str, args: &[String]) -> Vec<Vec<String>> {
    let fd_exec = |arg: &str| {
        FD_EXEC.contains(&arg)
            || arg
                .strip_prefix('-')
                .filter(|l| !l.starts_with('-'))
                .and_then(|l| l.strip_suffix(['x', 'X']))
                .is_some_and(|l| l.bytes().all(|b| b"HIsigFalLpu01q".contains(&b)))
    };
    let mut out = Vec::new();
    let mut k = 0;
    if prog == "find" {
        while k < args.len() {
            if FIND_EXEC.contains(&args[k].as_str()) {
                let mut end = k + 1;
                while end < args.len() && args[end] != ";" && args[end] != "+" {
                    end += 1;
                }
                if end > k + 1 {
                    out.push(args[k + 1..end].to_vec());
                }
                k = end;
            }
            k += 1;
        }
    } else if FD_PROGRAMS.contains(&prog) {
        while k < args.len() {
            if fd_exec(&args[k]) {
                let end = (k + 1..args.len())
                    .find(|&j| args[j] == ";")
                    .unwrap_or(args.len());
                if end > k + 1 {
                    out.push(args[k + 1..end].to_vec());
                }
                k = end;
            }
            k += 1;
        }
    }
    out
}

/// One frame of the `find -exec` / `fd -x` walk: its payloads, the next one, and what they inherit.
struct ExecFrame {
    payloads: Vec<Vec<String>>,
    next: usize,
    group: GroupId,
    exempt: bool,
    depth: usize,
}

/// `_Context.seen`'s key: (text, exempt, cwd at the start).
type SeenKey = (String, bool, Option<String>);

/// `_Context` plus the analysis-wide state: every group, the alternates, the flags and the work budget.
pub(crate) struct Ctx<'a> {
    home: &'a str,
    re: &'a PathRegexes,
    cwd: Option<String>,
    pub(crate) entries: Vec<Entry>,
    /// Other readings of wrapped commands (`_add_alternates`), for G1 only; each differs from its entry.
    pub(crate) alternates: Vec<Entry>,
    pub(crate) groups: Vec<Group>,
    pub(crate) incomplete: bool,
    pub(crate) work: Work,
    halted: bool,
    /// `_Context.seen`: (depth, cwd at the end) for every text analyzed in this pass.
    seen: HashMap<SeenKey, (usize, Option<String>)>,
    /// `_Entry.stdin_runs`, by entry index.
    stdin_runs: HashMap<usize, Vec<StdinRun>>,
    /// Entries that are a shell launcher peeled with no program (`sudo -s`): a shell reading stdin (SH-seq I-2).
    launchers: HashSet<usize>,
    /// The segment `add_one` is about to add came from a `$((...))` (`Segment::arith`).
    arith: bool,
    /// The commands `add_one` adds come from a segment with an unquoted `{` (`Segment::braces`); scoped to
    /// one `collect` call.
    braces: bool,
    /// The same segment's `Segment::quoted_braces`.
    quoted_braces: bool,
}

impl<'a> Ctx<'a> {
    pub(crate) fn new(home: &'a str, re: &'a PathRegexes, work: Work) -> Ctx<'a> {
        Ctx {
            home,
            re,
            cwd: None,
            entries: Vec::new(),
            alternates: Vec::new(),
            groups: Vec::new(),
            incomplete: false,
            work,
            halted: false,
            seen: HashMap::new(),
            stdin_runs: HashMap::new(),
            launchers: HashSet::new(),
            arith: false,
            braces: false,
            quoted_braces: false,
        }
    }

    /// The work budget ran out: nothing more is analyzed, and the result is incomplete.
    pub(crate) fn halted(&self) -> bool {
        self.halted
    }

    /// Start another pass over the command with `reading` (Python's `_check` runs `_analyze` afresh): the
    /// `cd` directory and the memo restart; the budget, the entries and the groups carry on.
    pub(crate) fn next_pass(&mut self, reading: Reading) {
        self.cwd = None;
        self.seen.clear();
        self.work.reading = reading;
    }

    fn halt(&mut self) {
        self.halted = true;
        self.incomplete = true;
    }

    /// Spend `n` of the Rust-only budget.
    fn spend(&mut self, n: usize) -> bool {
        if self.halted || self.work.charge_extra(n).is_err() {
            self.halt();
            return false;
        }
        true
    }

    /// `Some` of an `Ok`, else stop the analysis.
    fn take<T>(&mut self, result: Result<T, TooComplex>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(TooComplex) => {
                self.halt();
                None
            }
        }
    }

    /// `f`, written against Python's budget, run on the Rust-only one (work the reference does not do).
    fn on_extra<T>(&mut self, f: impl FnOnce(&mut Work) -> Result<T, TooComplex>) -> Option<T> {
        let mut work = self.work;
        work.budget = self.work.extra;
        let result = f(&mut work);
        let spent_extra = self.work.extra.0 - work.extra.0;
        self.work.extra = Budget(work.budget.0.saturating_sub(spent_extra));
        self.work.paren_lines |= work.paren_lines;
        self.take(result)
    }

    /// `f` on the Rust-only budget: work past input the reference drops (re-review F-2).
    fn redirected<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        let outer = std::mem::replace(&mut self.work.redirect, true);
        let result = f(self);
        self.work.redirect = outer;
        result
    }

    fn normalize(&mut self, path: &str, cwd: Option<&str>) -> String {
        match normalize_in(path, self.home, cwd, self.re) {
            Ok(path) => path,
            Err(LimitExceeded) => {
                self.incomplete = true;
                String::new()
            }
        }
    }

    fn group_texts(&self, group: GroupId) -> Vec<String> {
        self.groups
            .get(group.0 as usize)
            .map(|g| g.texts.clone())
            .unwrap_or_default()
    }

    /// SH-FW re-review I-2a: the pipeline whose first command is `head` is fed by a compound command or a
    /// function call: `head` came right after a `)` (`( echo X ) | sh`), is a closing word alone (`fi | sh`,
    /// `{ ...; } | sh`), or calls a name an earlier command of the text defined (`defined`, see
    /// `defined_names`).
    fn compound_head(
        &self,
        head: usize,
        defined: &HashMap<String, usize>,
        after_compound: &HashSet<usize>,
    ) -> bool {
        let entry = &self.entries[head];
        after_compound.contains(&head)
            || entry.raw.is_empty() // a redirect-only segment: `( ... ) 2>&1 | sh`
            || (entry.raw.len() == 1 && matches!(entry.raw[0].as_str(), "}" | "fi" | "done" | "esac"))
            || defined.get(&entry.prog).is_some_and(|&at| at < head)
    }

    /// Fail closed: entry `e`, before a stdin shell in its pipeline, writes only literal text the analysis
    /// reads: echo/printf, `cat` with no file, a closing word or redirect-only segment, a defined function.
    fn literal_producer(&self, e: usize, defined: &HashMap<String, usize>) -> bool {
        let entry = &self.entries[e];
        match entry.prog.as_str() {
            "echo" | "printf" | "" => true,
            "cat" => entry.args.iter().all(|a| a.starts_with('-') && a.len() > 1),
            name => {
                (entry.raw.len() == 1
                    && matches!(entry.raw[0].as_str(), "}" | "fi" | "done" | "esac"))
                    || defined.get(name).is_some_and(|&at| at < e)
            }
        }
    }

    /// Fail closed: a shell or `source` of `consumers` runs a file an earlier command of the text wrote
    /// (`printf X > a.sh; sh a.sh`).
    fn runs_written_file(&self, entries: &[usize], consumers: &[usize]) -> bool {
        if consumers.is_empty() {
            return false;
        }
        let written: HashSet<&str> = entries
            .iter()
            .flat_map(|&e| self.entries[e].redirs.iter())
            .filter(|r| r.op.starts_with('>') || r.op.starts_with("&>"))
            .map(|r| r.target.as_str())
            .collect();
        !written.is_empty()
            && consumers.iter().any(|&c| {
                self.entries[c]
                    .args
                    .iter()
                    .any(|a| written.contains(a.as_str()))
            })
    }

    /// The names the commands `entries` may define as functions, each with its first entry: `f()` lexes as
    /// the command `f`, `function f {` as `function f ...`. `None` once the analysis has stopped.
    fn defined_names(&mut self, entries: &[usize]) -> Option<HashMap<String, usize>> {
        let mut defined = HashMap::new();
        for &e in entries {
            let raw = &self.entries[e].raw;
            let name = match raw.as_slice() {
                [name] => name,
                [function, name, ..] if function == "function" => name,
                _ => continue,
            };
            if !defined.contains_key(name) {
                let name = name.clone();
                if !self.spend(WORD + name.len()) {
                    return None;
                }
                defined.insert(name, e);
            }
        }
        Some(defined)
    }

    /// `_collect(text, ctx, exempt, depth)`, memoized on (text, exempt, cwd) as Python's is. Returns true when
    /// `text` was skipped at `MAX_DEPTH` (the caller marks the entry it belongs to `DepthCap`).
    pub(crate) fn collect(&mut self, text: &str, exempt: bool, depth: usize) -> bool {
        if text.is_empty() || self.halted {
            return false;
        }
        if depth > MAX_DEPTH {
            return true;
        }
        let key: SeenKey = (text.to_string(), exempt, self.cwd.clone());
        if let Some((seen_depth, cwd)) = self.seen.get(&key)
            && *seen_depth <= depth
        {
            // analyzed already: same entries, same cwd
            self.cwd = cwd.clone();
            return false;
        }
        if !self.spend(1 + 2 * text.len()) {
            return false;
        }
        self.seen.insert(key.clone(), (depth, self.cwd.clone()));
        let (braces, quoted_braces) = (self.braces, self.quoted_braces);
        self.collect_once(text, exempt, depth);
        (self.braces, self.quoted_braces) = (braces, quoted_braces);
        self.seen.insert(key, (depth, self.cwd.clone()));
        false
    }

    /// `_collect_once`.
    fn collect_once(&mut self, text: &str, exempt: bool, depth: usize) {
        let LexOut { lexed, info } = lex_in(text, 0, &mut self.work);
        if lexed.incomplete {
            self.halt(); // Python raises `_TooComplex`
            return;
        }
        let mut from_text = Vec::new();
        if lexed.lenient || info.nested_lenient {
            from_text.push(Opaque::Unbalanced);
        }
        if info.depth_capped {
            from_text.push(Opaque::DepthCap);
        }
        let mut ids: HashMap<GroupId, GroupId> = HashMap::new();
        for group in lexed.groups {
            let id = GroupId(self.groups.len() as u32);
            ids.insert(group.id, id);
            self.groups.push(Group {
                id,
                texts: group.texts,
            });
        }
        let mut new_entries = Vec::new();
        let mut braced: HashSet<usize> = HashSet::new();
        let mut after_compound: HashSet<usize> = HashSet::new();
        for seg in lexed.segments {
            let group = ids.get(&seg.group).copied().unwrap_or(GroupId(u32::MAX));
            self.arith = seg.arith;
            self.braces = seg.braces;
            self.quoted_braces = seg.quoted_braces;
            match self.add_command(seg.tokens, seg.redirs, group, exempt, depth, &from_text) {
                Some(entry) => {
                    new_entries.push(entry);
                    if seg.braces {
                        braced.insert(entry);
                    }
                    if seg.after_compound {
                        after_compound.insert(entry);
                    }
                }
                None => return,
            }
        }
        // Literal text that reaches a shell's stdin is a command (heredocs, here-strings, `echo | sh`), text that
        // reaches an interpreter's stdin is code, and text that reaches xargs or parallel becomes the words or
        // lines of their command. Groups in first-appearance order.
        let mut order: Vec<GroupId> = Vec::new();
        let mut members: HashMap<GroupId, Vec<usize>> = HashMap::new();
        for &e in &new_entries {
            let group = self.entries[e].group;
            members
                .entry(group)
                .or_insert_with(|| {
                    order.push(group);
                    Vec::new()
                })
                .push(e);
        }
        let Some(defined) = self.defined_names(&new_entries) else {
            return;
        };
        // I-2a: `new_entries[..fed_from]` printed into an earlier compound-headed pipeline already
        let mut fed_from = 0;
        for group in order {
            let entries = members.remove(&group).unwrap_or_default();
            let mut shell = None;
            for &e in &entries {
                match self.reads_stdin_as_shell(e) {
                    Some(true) => {
                        shell = Some(e);
                        break;
                    }
                    Some(false) => {}
                    None => return,
                }
            }
            let interpreters: Vec<usize> = entries
                .iter()
                .copied()
                .filter(|&e| is_interpreter(&self.entries[e].prog))
                .collect();
            let feeders: Vec<usize> = entries
                .iter()
                .copied()
                .filter(|e| self.stdin_runs.get(e).is_some_and(|runs| !runs.is_empty()))
                .collect();
            if shell.is_none() && interpreters.is_empty() && feeders.is_empty() {
                continue;
            }
            let mut texts = self.group_texts(group);
            let mut decoded = Vec::new();
            for &e in &entries {
                let entry = &self.entries[e];
                if entry.prog == "echo" || entry.prog == "printf" {
                    texts.push(echo_text(&entry.args));
                    let result = echo_decoded(&entry.prog, &entry.args, &mut self.work);
                    let Some(found) = self.take(result) else {
                        return;
                    };
                    decoded.extend(found);
                }
            }
            // I-2a: a pipeline headed by a compound command or a function call is fed what the commands
            // before it print (a superset: the compound's own are among them)
            let head = entries[0];
            let unread = texts.is_empty();
            let compound = self.compound_head(head, &defined, &after_compound);
            // fail closed: a compound with a redirect or a here-string, or a producer that is not literal
            // text, before a shell or xargs/parallel
            let readers = shell.is_some() || !feeders.is_empty();
            if readers
                && compound
                && (!self.group_texts(group).is_empty() || !self.entries[head].redirs.is_empty())
            {
                self.halt();
                return;
            }
            if let Some(shell) = shell
                && entries
                    .iter()
                    .take_while(|&&e| e != shell)
                    .any(|&e| !self.literal_producer(e, &defined))
            {
                self.halt();
                return;
            }
            if unread && compound {
                let upto = new_entries.partition_point(|&e| e < head);
                for &e in &new_entries[fed_from.min(upto)..upto] {
                    let entry = &self.entries[e];
                    if entry.prog == "echo" || entry.prog == "printf" {
                        texts.push(echo_text(&entry.args));
                        let result = echo_decoded(&entry.prog, &entry.args, &mut self.work);
                        let Some(found) = self.take(result) else {
                            return;
                        };
                        decoded.extend(found);
                    }
                }
                fed_from = fed_from.max(upto);
                if readers && texts.is_empty() {
                    // a compound that prints no literal text (`{ cat <<X ... } | sh`)
                    self.halt();
                    return;
                }
            }
            if shell.is_some() && texts.iter().any(|t| only_substitution(t)) {
                // `sh <<< "$(f)"`: the text is what a command prints (the lexer leaves `""` for it)
                self.halt();
                return;
            }
            // C-4: a shell and xargs/parallel also get the decoded texts; an interpreter's `code` does not
            let mut fed = texts.clone();
            let mut known: HashSet<String> = texts.iter().cloned().collect();
            for text in decoded {
                if known.insert(text.clone()) {
                    fed.push(text);
                }
            }
            if !self.spend(fed.iter().map(|t| 2 * (WORD + t.len())).sum()) {
                return;
            }
            for &e in &interpreters {
                // each is scanned per entry (G3)
                let size: usize = texts.iter().map(String::len).sum();
                if self.work.charge(size).is_err() || !self.spend(WORD * texts.len() + size) {
                    self.halt();
                    return;
                }
                self.entries[e].code.extend(texts.iter().cloned());
            }
            if let Some(shell) = shell {
                // `bash --version` reads no script (`bash -s -- --version 1.2` does)
                let info_only = self.entries[shell]
                    .args
                    .first()
                    .is_some_and(|a| a == "--version" || a == "--help");
                if unread && !info_only {
                    mark(
                        &mut self.entries[shell].opaque,
                        Opaque::StdinShellUnreadable,
                    );
                }
                let exempt = self.entries[shell].exempt;
                for body in &fed {
                    if self.collect(body, exempt, depth + 1) {
                        mark(&mut self.entries[shell].opaque, Opaque::DepthCap);
                    }
                }
            }
            if !feeders.is_empty() {
                let lines = fed_lines(&fed);
                if !self.spend(lines.iter().map(|l| WORD + l.len()).sum()) {
                    return;
                }
                for &e in &feeders {
                    self.braces = braced.contains(&e);
                    self.feed_stdin(e, &lines, depth);
                }
            }
            if self.halted {
                return;
            }
        }
        // A shell or `source` given a process substitution runs what it prints (`sh <(echo X)`,
        // `cat <(echo X) | sh`): any such consumer in the text makes the literal output of every `<(...)` in it
        // a command.
        let consumers: Vec<usize> = new_entries
            .iter()
            .copied()
            .filter(|&e| {
                let prog = self.entries[e].prog.as_str();
                SHELLS.contains(&prog) || prog == "source" || prog == "."
            })
            .collect();
        // fail closed: `> >(sh)`, `coproc` with a shell, a shell running a file the text wrote
        let coproc = new_entries
            .iter()
            .any(|&e| self.entries[e].raw.first().is_some_and(|w| w == "coproc"));
        if (!consumers.is_empty() && (text.contains(">(") || coproc))
            || self.runs_written_file(&new_entries, &consumers)
        {
            self.halt();
            return;
        }
        if text.contains("<(") && !consumers.is_empty() {
            let Some(inners) = self.process_substitutions(text) else {
                return;
            };
            for inner in inners {
                let Some(bodies) = self.literal_output(&inner, depth) else {
                    return;
                };
                if bodies.is_empty() {
                    // fail closed: `sh < <(f)`, `bash <(curl ...)`: what it prints is not literal
                    self.halt();
                    return;
                }
                for body in bodies {
                    if self.collect(&body, exempt, depth + 1) {
                        for &e in &consumers {
                            mark(&mut self.entries[e].opaque, Opaque::DepthCap);
                        }
                    }
                }
            }
        }
        // A shell reading a file descriptor (`sh <&3`, `sh /dev/fd/3`) runs the literal text that a bare `exec`
        // in the same text opened (`exec 3<<<'X'`), or that a redirection on the shell itself opened.
        let fd_shells: HashSet<usize> = new_entries
            .iter()
            .copied()
            .filter(|&e| self.reads_fd_as_shell(e))
            .collect();
        if fd_shells.is_empty() {
            return;
        }
        for &e in &new_entries {
            let entry = &self.entries[e];
            let opener =
                entry.prog.is_empty() && entry.raw.first().is_some_and(|w| prog_name(w) == "exec");
            if !fd_shells.contains(&e) && !opener {
                continue;
            }
            for body in self.group_texts(entry.group) {
                if self.collect(&body, exempt, depth + 1) {
                    mark(&mut self.entries[e].opaque, Opaque::DepthCap);
                }
            }
            if self.halted {
                return;
            }
        }
    }

    /// `_feed_stdin`: literal stdin lines become commands for the xargs or parallel layer of entry `e`.
    fn feed_stdin(&mut self, e: usize, lines: &[String], depth: usize) {
        if lines.is_empty() {
            return;
        }
        let runs = self.stdin_runs.get(&e).cloned().unwrap_or_default();
        let (group, exempt) = (self.entries[e].group, self.entries[e].exempt);
        for run in runs {
            if run.parallel {
                let line = run.template.join(" ");
                let replace = run.replace.as_deref().unwrap_or("{}");
                let result = parallel_lines(&line, replace, &[lines.to_vec()], &mut self.work);
                let Some(found) = self.take(result) else {
                    return;
                };
                for (k, line) in found.iter().enumerate() {
                    // F-2: the reference reads the first `MAX_FED`
                    let capped = if k < MAX_FED {
                        self.collect(line, exempt, depth + 1)
                    } else {
                        self.redirected(|ctx| ctx.collect(line, exempt, depth + 1))
                    };
                    if capped {
                        mark(&mut self.entries[e].opaque, Opaque::DepthCap);
                    }
                }
            } else if let Some(replace) = &run.replace {
                // each word grows by its number of replacement strings times the line
                let count = |t: &String| {
                    if replace.is_empty() {
                        t.chars().count() + 1
                    } else {
                        t.matches(replace.as_str()).count()
                    }
                };
                let base: usize = run.template.iter().map(|t| WORD + 2 * t.len()).sum();
                let found: usize = run.template.iter().map(count).sum();
                for (k, line) in lines.iter().enumerate() {
                    if !self.spend(base + found.saturating_mul(line.len())) {
                        return;
                    }
                    let tokens = run
                        .template
                        .iter()
                        .map(|t| t.replace(replace.as_str(), line))
                        .collect();
                    let added = if k < MAX_FED {
                        self.add_command(tokens, Vec::new(), group, exempt, depth + 1, &[])
                    } else {
                        self.redirected(|ctx| {
                            ctx.add_command(tokens, Vec::new(), group, exempt, depth + 1, &[])
                        })
                    };
                    if added.is_none() {
                        return;
                    }
                }
            } else if !run.template.is_empty() {
                // the reference's command (the first `MAX_FED` lines), then, past them, one with every line
                let words = |lines: &[String]| {
                    let mut tokens = run.template.clone();
                    tokens.extend(
                        lines
                            .iter()
                            .flat_map(|line| pystr::split_ws(line))
                            .map(str::to_string),
                    );
                    tokens
                };
                let tokens = words(&lines[..lines.len().min(MAX_FED)]);
                if self
                    .add_command(tokens, Vec::new(), group, exempt, depth + 1, &[])
                    .is_none()
                {
                    return;
                }
                if lines.len() > MAX_FED {
                    let tokens = words(lines);
                    let added = self.redirected(|ctx| {
                        ctx.add_command(tokens, Vec::new(), group, exempt, depth + 1, &[])
                    });
                    if added.is_none() {
                        return;
                    }
                }
            }
        }
    }

    /// `_process_substitutions`: the inner text of each `<(...)` outside quotes.
    fn process_substitutions(&mut self, text: &str) -> Option<Vec<String>> {
        if !self.spend(text.len()) {
            return None;
        }
        let bytes = text.as_bytes();
        let n = bytes.len();
        let mut found = Vec::new();
        let mut j = 0;
        while j < n {
            match bytes[j] {
                b'\\' => j = skip_escape(text, j),
                b'\'' => j = find_from(text, j + 1, b'\'').map_or(n, |k| k + 1),
                b'"' => {
                    j += 1;
                    while j < n && bytes[j] != b'"' {
                        j = if bytes[j] == b'\\' {
                            skip_escape(text, j)
                        } else {
                            j + 1
                        };
                    }
                    j += 1;
                }
                b'<' if bytes.get(j + 1) == Some(&b'(') => {
                    let result = scan_paren(text, j + 2, &mut Memo::default(), &mut self.work);
                    let (inner, end) = self.take(result)?;
                    if !self.spend(WORD + inner.len()) {
                        return None;
                    }
                    found.push(inner.to_string());
                    j = end;
                }
                _ => j += 1,
            }
        }
        Some(found)
    }

    /// `_literal_output`: literal text the commands in `text` print (echo/printf arguments, here-strings,
    /// heredoc bodies).
    fn literal_output(&mut self, text: &str, depth: usize) -> Option<Vec<String>> {
        let LexOut { lexed, .. } = lex_in(text, depth + 1, &mut self.work);
        if lexed.incomplete {
            self.halt();
            return None;
        }
        let mut by_id: HashMap<GroupId, Vec<String>> =
            lexed.groups.into_iter().map(|g| (g.id, g.texts)).collect();
        let mut texts = Vec::new();
        let mut known: HashSet<String> = HashSet::new();
        for seg in &lexed.segments {
            let result = strip_prefixes(&seg.tokens, None, None, true, &mut self.work);
            let stripped = self.take(result)?;
            if let Some(first) = stripped.cmd.first()
                && matches!(prog_name(first).as_str(), "echo" | "printf")
            {
                let out = echo_text(&stripped.cmd[1..]);
                let result = echo_decoded(&prog_name(first), &stripped.cmd[1..], &mut self.work);
                let decoded = self.take(result)?;
                if !self
                    .spend(WORD + out.len() + decoded.iter().map(|d| WORD + d.len()).sum::<usize>())
                {
                    return None;
                }
                known.insert(out.clone());
                texts.push(out);
                // C-4: a shell runs what it prints, escapes decoded
                for text in decoded {
                    if known.insert(text.clone()) {
                        texts.push(text);
                    }
                }
            }
            if let Some(group) = by_id.remove(&seg.group) {
                known.extend(group.iter().cloned());
                texts.extend(group);
            }
        }
        Some(texts)
    }

    /// `_reads_stdin_as_shell`: a shell fed from stdin (`bash`, `sh -s`, `sh /dev/stdin`, `source /dev/stdin`)
    /// or a docker command that runs one. `None` when the work budget ran out.
    fn reads_stdin_as_shell(&mut self, e: usize) -> Option<bool> {
        let entry = &self.entries[e];
        let prog = entry.prog.as_str();
        // SH-seq I-2: a shell launcher with no program (`sudo -s`, `su`, `script`, `sg grp`), and `newgrp`
        if self.launchers.contains(&e) || prog == "newgrp" {
            return Some(true);
        }
        if SHELLS.contains(&prog) {
            let parsed = shell_parse(prog, &entry.args, 0);
            return Some(match parsed.mode {
                ShellMode::Stdin => true,
                ShellMode::File => entry
                    .args
                    .get(parsed.start)
                    .is_some_and(|f| is_stdin_path(f)),
                // SH-seq I-2: `sh -c "$(cat)"`. The lexer leaves `""` for a double-quoted substitution, so an
                // empty script counts as one; a single-quoted `'$(cat)'` is read as written.
                ShellMode::Command => parsed
                    .scripts
                    .iter()
                    .any(|(_, script)| script.is_empty() || reads_stdin_substitution(script)),
            });
        }
        if prog == "eval" {
            // SH-seq I-2: `eval "$(cat)"`, `eval '$(</dev/stdin)'`: the words are the script
            return Some(
                entry.args.iter().any(String::is_empty)
                    || reads_stdin_substitution(&entry.args.join(" ")),
            );
        }
        if prog == "source" || prog == "." {
            return Some(first_arg(&entry.args).is_some_and(is_stdin_path));
        }
        if DOCKER.contains(&prog) {
            let args = entry.args.clone();
            let Some(shells) = embedded_shells_in(&args, &mut self.work.extra) else {
                self.halt();
                return None;
            };
            return Some(shells.iter().any(|(mode, _)| *mode == ShellMode::Stdin));
        }
        Some(false)
    }

    /// `_reads_fd_as_shell`: a shell (or `source`) whose script is a file descriptor (`sh <&3`, `sh /dev/fd/3`,
    /// `source /dev/fd/3`).
    fn reads_fd_as_shell(&self, e: usize) -> bool {
        let entry = &self.entries[e];
        let prog = entry.prog.as_str();
        if prog == "source" || prog == "." {
            return first_arg(&entry.args).is_some_and(is_fd_path);
        }
        if !SHELLS.contains(&prog) {
            return false;
        }
        let parsed = shell_parse(prog, &entry.args, 0);
        match parsed.mode {
            ShellMode::File => entry.args.get(parsed.start).is_some_and(|f| is_fd_path(f)),
            ShellMode::Stdin => entry.redirs.iter().any(|r| r.op == "<&"),
            _ => false,
        }
    }

    /// `_add_command`, with the `find -exec` / `fd -x` recursion walked iteratively. Returns the entry's
    /// index, or `None` once the analysis has stopped.
    fn add_command(
        &mut self,
        tokens: Vec<String>,
        redirs: Vec<Redirect>,
        group: GroupId,
        exempt: bool,
        depth: usize,
        from_text: &[Opaque],
    ) -> Option<usize> {
        let e = self.add_one(tokens, redirs, group, exempt, depth, from_text)?;
        if self.entries[e].prog == "find" || FD_PROGRAMS.contains(&self.entries[e].prog.as_str()) {
            self.exec_walk(e, depth);
        }
        if self.halted { None } else { Some(e) }
    }

    /// `_collect_find_exec` and `_collect_fd_exec`, depth-first in Python's order, without recursion.
    fn exec_walk(&mut self, e: usize, depth: usize) {
        let entry = &self.entries[e];
        let mut stack = vec![ExecFrame {
            payloads: exec_payloads(&entry.prog, &entry.args),
            next: 0,
            group: entry.group,
            exempt: entry.exempt,
            depth,
        }];
        while let Some(top) = stack.last_mut() {
            if self.halted {
                return;
            }
            if top.next >= top.payloads.len() {
                stack.pop();
                continue;
            }
            let payload = std::mem::take(&mut top.payloads[top.next]);
            top.next += 1;
            let (group, exempt, depth) = (top.group, top.exempt, top.depth + 1);
            if stack.len() > FIND_EXEC_CAP {
                self.incomplete = true;
                mark(&mut self.entries[e].opaque, Opaque::DepthCap);
                return;
            }
            let Some(child) = self.add_one(payload, Vec::new(), group, exempt, depth, &[]) else {
                return;
            };
            let child = &self.entries[child];
            if child.prog == "find" || FD_PROGRAMS.contains(&child.prog.as_str()) {
                let frame = ExecFrame {
                    payloads: exec_payloads(&child.prog, &child.args),
                    next: 0,
                    group: child.group,
                    exempt: child.exempt,
                    depth,
                };
                stack.push(frame);
            }
        }
    }

    /// `_add_command` for one command, except the `find -exec` / `fd -x` payloads.
    fn add_one(
        &mut self,
        tokens: Vec<String>,
        redirs: Vec<Redirect>,
        group: GroupId,
        exempt: bool,
        depth: usize,
        from_text: &[Opaque],
    ) -> Option<usize> {
        let arith = std::mem::take(&mut self.arith);
        let size: usize = tokens.iter().map(|t| WORD + t.len()).sum();
        if !self.spend(1 + 2 * size) {
            return None;
        }
        if fail_closed(&tokens, self.braces, self.quoted_braces) {
            self.halt();
            return None;
        }
        let mut alternates = Vec::new();
        let mut runs = Vec::new();
        let result = strip_prefixes(
            &tokens,
            Some(&mut alternates),
            Some(&mut runs),
            true,
            &mut self.work,
        );
        let Stripped {
            cmd,
            mut scripts,
            extra,
            peeled,
        } = self.take(result)?;
        let prog_path = cmd.first().map(|w| pystr::lower(w)).unwrap_or_default();
        let prog = cmd.first().map(|w| prog_name(w)).unwrap_or_default();
        let args: Vec<String> = cmd.get(1..).map(<[String]>::to_vec).unwrap_or_default();
        let prefix = &tokens[..tokens.len().saturating_sub(cmd.len())];
        // an integer sequence is allowed after the program word only: before it, or as it, it could form one
        if self.braces
            && tokens[..(prefix.len() + 1).min(tokens.len())]
                .iter()
                .any(|t| !brace_sequences(t).is_empty())
        {
            self.halt();
            return None;
        }
        if option_env_unmodelled(prefix, &prog, &args) {
            self.halt();
            return None;
        }
        // an exported command variable runs later, as its prefix form does now
        if EXPORTERS.contains(&prog.as_str()) {
            for arg in &args {
                if let Some((name, value)) = arg.split_once('=')
                    && COMMAND_ENV.contains(&name)
                    && !value.is_empty()
                    && !scripts.iter().any(|s| s == value)
                {
                    scripts.push(value.to_string());
                }
            }
        }
        let interpreter = is_interpreter(&prog);
        let code = if interpreter {
            if !self.spend(args.iter().map(|t| WORD + t.len()).sum::<usize>()) {
                return None;
            }
            args.clone()
        } else {
            Vec::new()
        };
        let mut cwd = self.cwd.clone();
        if CHDIR_PROGRAMS.contains(&prog.as_str()) {
            // `tar -C dir x`, `git -C dir ...`
            for k in 0..args.len().saturating_sub(1) {
                if args[k] == "-C" || args[k] == "--directory" {
                    let base = self.cwd.clone();
                    let path = self.normalize(&args[k + 1], base.as_deref());
                    if !path.is_empty() {
                        cwd = Some(path);
                    }
                }
            }
        }
        // `_add_alternates`: every alternate's scripts join this command's (once each); each alternate whose
        // command differs from this one is kept for G1 (Python checks G1 here and keeps the first that hits).
        // One that starts among the peeled words (before the program the main parse found) means the tables
        // may have misread an option's value: the entry is marked (lead ruling R16).
        let main_at = tokens.len().saturating_sub(cmd.len());
        // C-3: the command with its brace expressions expanded is one more (it marks nothing); one that
        // changes the program word is too complex (C-3a: `{echo,X} | sh` would be another command's input)
        let mut braced = None;
        if self.braces {
            let result = brace_reading(&tokens, &mut self.work);
            if let Some(reading) = self.take(result)? {
                braced = Some(alternates.len());
                alternates.push((main_at, reading, false));
            }
        }
        let mut differs = false;
        for (k, (at, alternate, verbatim)) in alternates.into_iter().enumerate() {
            let other = if verbatim {
                // the shipped reading of a package manager: its words are the command, as they are
                Stripped {
                    cmd: alternate.clone(),
                    ..Stripped::default()
                }
            } else {
                let result = strip_prefixes(&alternate, None, None, false, &mut self.work);
                self.take(result)?
            };
            if braced == Some(k)
                && other.cmd.first() != cmd.first()
                && (other.cmd.len() < alternate.len()
                    || other
                        .cmd
                        .first()
                        .is_some_and(|w| brace_risky(&prog_name(w))))
            {
                self.halt();
                return None;
            }
            for script in other.scripts {
                if !scripts.contains(&script) {
                    scripts.push(script);
                }
            }
            if other.cmd == cmd {
                continue;
            }
            differs |= at < main_at;
            if !self.spend(
                ENTRY_COST + 1 + size + other.cmd.iter().map(|t| WORD + t.len()).sum::<usize>(),
            ) {
                return None;
            }
            let other_prog = other.cmd.first().map(|w| prog_name(w)).unwrap_or_default();
            let other_args: Vec<String> = other
                .cmd
                .get(1..)
                .map(<[String]>::to_vec)
                .unwrap_or_default();
            self.alternates.push(Entry {
                raw: tokens.clone(),
                redirs: Vec::new(),
                prog_path: other
                    .cmd
                    .first()
                    .map(|w| pystr::lower(w))
                    .unwrap_or_default(),
                code: if is_interpreter(&other_prog) {
                    other_args.clone()
                } else {
                    Vec::new()
                },
                exempt: exempt || DOCKER.contains(&other_prog.as_str()),
                prog: other_prog,
                args: other_args,
                cwd: cwd.clone(),
                group,
                opaque: Vec::new(),
            });
        }
        let mut opaque: Vec<Opaque> = Vec::new();
        for &reason in from_text {
            mark(&mut opaque, reason);
        }
        if peeled.placeholder
            || peeled.stdin_args
            || peeled.emptied
            || STDIN_ARG_RUNNERS.contains(&prog.as_str())
            || cmd.first().is_some_and(|w| {
                // in arithmetic, `*` is multiplication: only an expansion makes the word dynamic
                if arith {
                    w.contains(['$', '`'])
                } else {
                    is_dynamic(w) && !TEST_BRACKETS.contains(&w.as_str())
                }
            })
        {
            mark(&mut opaque, Opaque::DynamicCommandWord);
        }
        if peeled.unknown_option
            || differs
            || (UNPARSED_RUNNERS.contains(&prog.as_str()) && !args.is_empty())
            || runs_option_command(&prog, &args)
        {
            mark(&mut opaque, Opaque::UnknownWrapperOption);
        }
        if interpreter && !pm_runs_own_verb(&prog, &args) {
            mark(&mut opaque, Opaque::InterpreterCode);
        }
        if prog == "eval" {
            mark(&mut opaque, Opaque::Eval);
        }
        if (prog == "source" || prog == ".") && !args.is_empty() {
            mark(&mut opaque, Opaque::Source);
        }
        if scripts
            .iter()
            .any(|s| s.is_empty() || s.contains(['$', '`']))
        {
            mark(&mut opaque, Opaque::ShellScriptFromExpansion);
        }
        if SHELLS.contains(&prog.as_str()) {
            let parsed = shell_parse(&prog, &args, 0);
            if parsed.mode == ShellMode::File
                && let Some(file) = args.get(parsed.start)
            {
                // `bash ./install.sh` runs a file the analysis cannot read, as `source` does
                mark(&mut opaque, Opaque::Source);
                if is_dynamic(file) {
                    mark(&mut opaque, Opaque::ShellScriptFromExpansion);
                }
            }
        }
        let entry_exempt = exempt || DOCKER.contains(&prog.as_str());
        // M-1: a kept entry is charged before it is stored (out of budget: incomplete, nothing dropped)
        if !self.spend(ENTRY_COST) {
            return None;
        }
        self.entries.push(Entry {
            raw: tokens,
            redirs,
            prog_path,
            prog: prog.clone(),
            args: args.clone(),
            cwd,
            exempt: entry_exempt,
            group,
            code,
            opaque,
        });
        let e = self.entries.len() - 1;
        if !runs.is_empty() {
            self.stdin_runs.insert(e, runs);
        }
        if peeled.shell_launch {
            self.launchers.insert(e);
        }
        for script in &scripts {
            if self.collect(script, exempt, depth + 1) {
                mark(&mut self.entries[e].opaque, Opaque::DepthCap);
            }
        }
        for script in &extra {
            // F-2: GNU parallel lines past the reference's
            if self.redirected(|ctx| ctx.collect(script, exempt, depth + 1)) {
                mark(&mut self.entries[e].opaque, Opaque::DepthCap);
            }
        }
        // C-5: bash evaluates a subscript as arithmetic, running its substitutions even inside single quotes
        if SUBSCRIPT_PROGRAMS.contains(&prog.as_str())
            || (SUBSCRIPT_V_PROGRAMS.contains(&prog.as_str())
                && args.iter().any(|a| a.starts_with("-v")))
        {
            for arg in args.iter().filter(|a| a.contains('[')) {
                let scripts = self.on_extra(|work| {
                    let found = body_substitutions(arg, true, work)?;
                    Ok(found
                        .into_iter()
                        .map(|s| s.2.into_owned())
                        .collect::<Vec<String>>())
                })?;
                for script in scripts {
                    if self.collect(&script, exempt, depth + 1) {
                        mark(&mut self.entries[e].opaque, Opaque::DepthCap);
                    }
                }
            }
        }
        if DOCKER.contains(&prog.as_str()) {
            // `docker run img sh -euc "..."`
            let Some(starts) = script_starts_in(&args, &mut self.work.extra) else {
                self.halt();
                return None;
            };
            for (_start, script) in starts {
                if self.collect(&script, true, depth + 1) {
                    mark(&mut self.entries[e].opaque, Opaque::DepthCap);
                }
            }
        } else if prog == "cd" || prog == "pushd" {
            match first_arg(&args) {
                None => self.cwd = Some(self.normalize("~", None)),
                Some("-") => {}
                Some(target) => {
                    let base = self.cwd.clone();
                    let new_cwd = self.normalize(target, base.as_deref());
                    if new_cwd.matches('/').count() <= MAX_CWD_DEPTH {
                        // keeps absurd `cd` chains linear
                        self.cwd = Some(new_cwd);
                    }
                }
            }
        }
        if self.halted { None } else { Some(e) }
    }
}

/// `_feed_stdin`'s lines: every non-blank line of `texts`, stripped, all of them (the reference reads the
/// first `MAX_FED`; SH-FW re-review F-2).
fn fed_lines(texts: &[String]) -> Vec<String> {
    let mut lines = Vec::new();
    for text in texts {
        for line in pystr::splitlines(text) {
            let line = pystr::strip_ws(line);
            if !line.is_empty() {
                lines.push(line.to_string());
            }
        }
    }
    lines
}
