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
//!   quoted item); a `[...]` subscript holding a closed `$(...)`, backtick pair or `$'...'` in any word (G0-FP:
//!   not after a `\[`); an assignment, export or
//!   prefix of `TAR_OPTIONS` or `GIT_CONFIG_*` that is not the modelled prefix of `tar` / `git`, a git alias
//!   given twice or through `GIT_CONFIG_*`, `git --config-env`; a shell reading stdin from a producer that
//!   is not literal (`curl | sh`, `f | tr | sh`), from a compound command with a redirect or here-string, or
//!   from a compound with no literal output; a here-string that is only a substitution (`sh <<< "$(f)"`);
//!   `>(` with a shell; a `<(` with no literal output given to a shell; `coproc`; a shell running a file an
//!   earlier command wrote (G0-FP: as its script, not as an argument of the script; `2>&1` writes none).
//!   Each is too complex (the guard blocks).
//! - SH-F2: an xargs/parallel layer fed a `<(...)` (a `<` redirect or `-a`) gets its literal output, as from a
//!   pipe; a `-c` script, eval words or a program word that use a variable `read`/`mapfile` set (or a `for` or
//!   assignment over a substitution) read stdin as commands, and a text that feeds such a setter, or feeds a
//!   compound closer or an orphan pipeline a here-string or heredoc, while it adds a stdin reader, is too
//!   complex (`echo X | while read l; do sh -c "$l"; done`, `{ sh; } <<< X`).
//! - SH-F2 review: every xargs/parallel layer of a text holding `<(` is fed (I-1); a layer added by a script,
//!   a bare `exec` or a function call given a here-string, and a piped or fed function call, count as fed
//!   (I-1, I-2); read names propagate through copies, namerefs and `${!n}`, a shell's positional
//!   parameters are put in place in its script, a reader added before its `read` counts, and a shell or
//!   `source` of a descriptor reads stdin once a name is read (I-2).

mod printf;

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use crate::analyze::Entry;
use crate::lexer::scan::{body_substitutions, scan_paren};
use crate::lexer::words::{Memo, ansi_c_decode};
use crate::lexer::{
    Group, GroupId, Holder, LexOut, Reading, Redirect, TooComplex, Work, find_from, lex_in, pystr,
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
/// SH-F3: the compound commands whose body reads the pipe that feeds them (`echo X | if true; then sh; fi`).
const COMPOUND_WORDS: &[&str] = &["if", "while", "until", "for", "select", "case"];

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
    param_close(b, j).map_or(b.len(), |k| k + 1)
}

/// The index of the `}` closing the `${` at `j` (`b[j]` is `$`), or `None` when it is not closed.
pub(crate) fn param_close(b: &[u8], j: usize) -> Option<usize> {
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
                    return Some(k);
                }
            }
            _ => {}
        }
        k += 1;
    }
    None
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
///
/// G0-FP: bash runs nothing for an unclosed substitution or subscript, and no name ends in a backslash, so the
/// substitution start counts only with its closer (`` ` ``, `)`, `'`) and then a `]` after it, and a `[` right
/// after a backslash opens nothing (`echo '[`'`, `grep '\[`x`\]'`, a regex `` [`] `` stay complete).
fn subscript_runs(word: &str) -> bool {
    let b = word.as_bytes();
    let Some(close) = word.rfind(']') else {
        return false;
    };
    // the last closer of each kind before the last `]`: a start before it is closed, and a `]` follows
    let last = |closer: u8| b[..close].iter().rposition(|&c| c == closer);
    let (tick, paren, quote) = (last(b'`'), last(b')'), last(b'\''));
    // one pass: a substitution start with a `[` before it and no `]` between
    let mut open = false;
    for (j, &c) in b[..close].iter().enumerate() {
        match c {
            b'[' if j == 0 || b[j - 1] != b'\\' => open = true,
            b']' => open = false,
            b'`' if open && tick.is_some_and(|k| k > j) => return true,
            b'$' if open => match b.get(j + 1) {
                Some(b'(') if paren.is_some_and(|k| k > j + 1) => return true,
                Some(b'\'') if quote.is_some_and(|k| k > j + 1) => return true,
                _ => {}
            },
            _ => {}
        }
    }
    false
}

/// `runs_written_file`: `r` writes a file named by its target. `>&N`, `>&N-` and `>&-` (`2>&1`) duplicate or
/// close a descriptor instead (G0-FP).
fn writes_file(r: &Redirect) -> bool {
    let dup = r.op == ">&" && {
        let fd = r.target.strip_suffix('-').unwrap_or(&r.target);
        fd.bytes().all(|c| c.is_ascii_digit())
    };
    (r.op.starts_with('>') || r.op.starts_with("&>")) && !dup
}

/// `runs_written_file`: the words of consumer `entry` that may name a file it runs. A shell given a script file
/// (`sh x.sh 1 2`) and `source x.sh 1 2` run only that file, so the words after it, its positional parameters,
/// do not count (G0-FP); every other consumer's arguments do (`sh -c ... _ x.sh`, `sudo -s`).
fn run_words(entry: &Entry) -> &[String] {
    let args = entry.args.as_slice();
    let prog = entry.prog.as_str();
    if SHELLS.contains(&prog) {
        let parsed = shell_parse(prog, args, 0);
        if parsed.mode == ShellMode::File {
            return &args[..(parsed.start + 1).min(args.len())];
        }
    } else if (prog == "source" || prog == ".") && args.first().is_some_and(|a| !a.starts_with('-'))
    {
        return &args[..1];
    }
    args
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

/// A shell variable name, `[A-Za-z_][A-Za-z0-9_]*`.
fn is_name(word: &str) -> bool {
    let mut chars = word.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// SH-F2: the variables a command sets from what it reads, lowercased: every name among the words of `read`
/// (and `REPLY`) or of `mapfile`/`readarray` (and `MAPFILE`); with a substitution in its segment (`held`), the
/// name of a `for`/`select`; and each assignment whose segment is `held` or whose value uses a name already
/// read (`known`, or one set earlier in the same command: `x=$l`, `export x=$l`). Review I-2: a nameref
/// (`declare -n r=l`, `local -n`, `typeset -n`) sets every name it assigns once any name is read. Option
/// values that look like names count too.
fn setter_names(
    prog: &str,
    args: &[String],
    tokens: &[String],
    held: bool,
    known: &HashSet<String>,
) -> Vec<String> {
    let lower = |w: &str| w.to_ascii_lowercase();
    let mut names: Vec<String> = match prog {
        "read" => std::iter::once("reply".to_string())
            .chain(args.iter().filter(|a| is_name(a)).map(|a| lower(a)))
            .collect(),
        "mapfile" | "readarray" => std::iter::once("mapfile".to_string())
            .chain(args.iter().filter(|a| is_name(a)).map(|a| lower(a)))
            .collect(),
        "for" | "select" if held => args
            .first()
            .filter(|a| is_name(a))
            .map(|a| lower(a))
            .into_iter()
            .collect(),
        _ => Vec::new(),
    };
    let nameref = matches!(prog, "declare" | "local" | "typeset")
        && args
            .iter()
            .any(|a| a.len() > 1 && a.starts_with('-') && a[1..].contains('n'));
    for word in tokens.iter().filter(|w| is_assignment(w)) {
        if let Some((name, value)) = word.split_once('=') {
            let read = !known.is_empty() || !names.is_empty();
            let uses = uses_name(value, read, |n| {
                known.contains(n) || names.iter().any(|m| m == n)
            });
            if held || uses || (nameref && read) {
                names.push(lower(name));
            }
        }
    }
    names
}

/// SH-F2: `text` expands a name `is_read` accepts (lowercased: `$l`, `${l}`, `${l:-x}`, `${#l}`), or, when
/// `any_read` (some name is read), an indirect `${!n}` (review I-2).
fn uses_name(text: &str, any_read: bool, is_read: impl Fn(&str) -> bool) -> bool {
    let b = text.as_bytes();
    (0..b.len()).filter(|&i| b[i] == b'$').any(|i| {
        let mut j = i + 1;
        let braced = b.get(j) == Some(&b'{');
        if braced {
            j += 1;
        }
        let bang = b.get(j) == Some(&b'!');
        if matches!(b.get(j), Some(b'!' | b'#')) {
            j += 1;
        }
        if !b
            .get(j)
            .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_')
        {
            return false;
        }
        if braced && bang && any_read {
            return true;
        }
        let end = (j..b.len())
            .find(|&k| !(b[k].is_ascii_alphanumeric() || b[k] == b'_'))
            .unwrap_or(b.len());
        is_read(&text[j..end].to_ascii_lowercase())
    })
}

/// SH-F4: the commands of the `{ ... }` body of the function whose definition is `run[0]` (`f` of `f() {`, or
/// `function f ...`), from the entries `run` (the definition and the entries after it): each command's words
/// with its leading `{` words dropped, until the `}` that closes the body. Not a `{` body (`f() ( ... )`):
/// nothing.
fn function_body(entries: &[Entry], run: &[usize]) -> Vec<Vec<String>> {
    function_span(entries, run)
        .unwrap_or_default()
        .into_iter()
        .map(|(e, from)| entries[e].raw[from..].to_vec())
        .collect()
}

/// `function_body`'s commands as (entry, index of the command's first word in its `raw`). `None`: the
/// definition has no `{` body (`f() ( ... )`, zsh's `f() cmd`, or nothing after it).
fn function_span(entries: &[Entry], run: &[usize]) -> Option<Vec<(usize, usize)>> {
    let (&first, rest) = run.split_first()?;
    let head = &entries[first].raw;
    let (mut at, mut from, mut tail): (usize, usize, &[usize]) = match head.as_slice() {
        [_] => {
            let (&next, tail) = rest.split_first()?;
            (next, 0, tail)
        }
        [function, _, more @ ..] if function == "function" => {
            if more.is_empty() {
                let (&next, tail) = rest.split_first()?;
                (next, 0, tail)
            } else {
                (first, 2, rest)
            }
        }
        _ => return None,
    };
    if entries[at].raw.get(from).is_none_or(|w| w != "{") {
        return None;
    }
    let mut out = Vec::new();
    let mut depth = 0usize;
    loop {
        let words = &entries[at].raw[from..];
        if words.len() == 1 && words[0] == "}" {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Some(out);
            }
        } else {
            let opened = words.iter().take_while(|w| *w == "{").count();
            depth += opened;
            if words.len() > opened {
                out.push((at, from + opened));
            }
        }
        let Some((&next, more)) = tail.split_first() else {
            return Some(out);
        };
        (at, from, tail) = (next, 0, more);
    }
}

/// SH-F4: a function body's words with a call's positional parameters (`params[0]` is the name) put in place:
/// a word that is `$@`, `$*`, `${@}` or `${*}` becomes the arguments, each word, and any other word gets
/// `positional_script`'s reading.
fn positional_words(words: &[String], params: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for word in words {
        if matches!(word.as_str(), "$@" | "$*" | "${@}" | "${*}") {
            out.extend(params.iter().skip(1).cloned());
        } else {
            out.push(positional_script(word, params));
        }
    }
    out
}

/// SH-F4 review I-1: the body words with every word that holds a positional parameter split on blanks, as
/// bash splits an unquoted `$1` (`f() { $1; }; f "npx x"`). The lexed words no longer show the quotes, so
/// this is a second reading next to `positional_words` (a quoted path with a space stays whole there).
/// `None` when it reads the same.
fn positional_split(words: &[String], params: &[String]) -> Option<Vec<String>> {
    let mut out = Vec::new();
    for word in words {
        if matches!(word.as_str(), "$@" | "$*" | "${@}" | "${*}") {
            out.extend(params.iter().skip(1).cloned());
            continue;
        }
        let value = positional_script(word, params);
        if value == *word {
            out.push(value);
        } else {
            out.extend(pystr::split_ws(&value).into_iter().map(str::to_owned));
        }
    }
    (out != positional_words(words, params)).then_some(out)
}

/// SH-F2 review I-2: `sh -c S $0 $1 ...` read with its positional parameters put in place (`$1`, `${10}`, `$@`,
/// `$*`, `${@}`), so `sh -c 'eval "$1"' _ "$l"` reads as `eval "$l"`. `params[0]` is `$0`. SH-F4: a quoted
/// `"$@"` (or `"$*"`, `"${@}"`) loses its quotes, so its words stay words (`sh -c '"$@"' _ npx x`).
fn positional_script(script: &str, params: &[String]) -> String {
    let b = script.as_bytes();
    let mut out = String::new();
    let (mut last, mut i) = (0, 0);
    while i < b.len() {
        if b[i] != b'$' {
            i += 1;
            continue;
        }
        let found = if b.get(i + 1) == Some(&b'{') {
            let j = i + 2;
            let k = (j..b.len())
                .find(|&k| !b[k].is_ascii_digit())
                .unwrap_or(b.len());
            if k > j && b.get(k) == Some(&b'}') {
                Some((&script[j..k], k + 1))
            } else if matches!(b.get(j), Some(b'@' | b'*')) && b.get(j + 1) == Some(&b'}') {
                Some((&script[j..j + 1], j + 2))
            } else {
                None
            }
        } else if b
            .get(i + 1)
            .is_some_and(|c| c.is_ascii_digit() || matches!(c, b'@' | b'*'))
        {
            Some((&script[i + 1..i + 2], i + 2))
        } else {
            None
        };
        let Some((key, mut end)) = found else {
            i += 1;
            continue;
        };
        let mut at = i;
        if matches!(key, "@" | "*") && i > last && b[i - 1] == b'"' && b.get(end) == Some(&b'"') {
            at = i - 1;
            end += 1;
        }
        let value = match key {
            "@" | "*" => params.get(1..).unwrap_or(&[]).join(" "),
            n => n
                .parse::<usize>()
                .ok()
                .and_then(|n| params.get(n))
                .cloned()
                .unwrap_or_default(),
        };
        out.push_str(&script[last..at]);
        out.push_str(&value);
        last = end;
        i = end;
    }
    out.push_str(&script[last..]);
    out
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

/// `_Context.seen`'s key: (text, exempt, cwd at the start), and (PE-1) whether the analysis put the text's
/// positional parameters in place (`Ctx::positional_known`).
type SeenKey = (String, bool, Option<String>, bool);

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
    /// The words of the segment `add_one` is about to add that hold a substitution (`Holder`, F-8).
    holders: Vec<Holder>,
    /// SH-F3: the segment `add_one` is about to add holds a `$(` or a backtick (`LexOut::held`).
    subst: bool,
    /// SH-F3: the entries whose segment held a `$(` or a backtick.
    held_entries: HashSet<usize>,
    /// SH-F3: how many stdin-reading entries have been added (`reads_stdin_as_shell`), in this pass or
    /// earlier. A text that adds one bumps it; a memoized text that did bumps it again.
    readers: usize,
    /// SH-F3: the entries whose `-c`/`eval` scripts added a stdin reader.
    script_readers: HashSet<usize>,
    /// SH-F3: the texts (keys of `seen`) whose analysis added a stdin reader.
    reads_seen: HashSet<SeenKey>,
    /// SH-F2: the variables a command of this pass set from what it read (`setter_names`), lowercased.
    read_names: HashSet<String>,
    /// SH-F2: the entries that set one.
    setters: HashSet<usize>,
    /// SH-F2 review I-1: how many xargs/parallel layers (`stdin_runs`) have been added.
    feeders: usize,
    /// SH-F2 review I-1: the entries whose scripts or payloads added one.
    script_feeders: HashSet<usize>,
    /// PE-1: the words of the segment `add_one` last added that held a parameter expansion, for the
    /// `find -exec` / `fd -x` payloads `exec_walk` adds from them.
    param_words: Vec<String>,
    /// PE-1, PE-3, PE-5, PE-6: a command the analysis cannot prove was read (a program or script from a
    /// parameter expansion, code from a value, a nested subscript, a renamed program). The analysis goes on,
    /// and is too complex at its end (`Analysis.incomplete`).
    pub(crate) unproven: bool,
    /// PE-1: a positional parameter was a program or in a script (`positional`) where the analysis put the
    /// parameters in place; too complex where the command sets them some other way (`sets_positionals`).
    pub(crate) positional: bool,
    /// PE-1: the entries whose program or script is a positional parameter, each with whether its text's
    /// parameters were put in place (`top_known`); `collect_once` decides them for its own text.
    positional_uses: Vec<(usize, bool)>,
    /// PE-1: the next `collect` is of a shell's `-c` script whose arguments are all literal, so the
    /// analysis also reads it with them in place (`positional_script`).
    positional_known: bool,
    /// PE-1: `positional_known` for the segment `add_one` is about to add (top level of its text).
    top_known: bool,
    /// PE-1: a function whose body's positional program or script was read with each call's arguments in
    /// place, and those calls (and its definition): no other entry of the analysis may run it
    /// (`calls_only_direct`).
    pub(crate) function_calls: Vec<(String, HashSet<usize>)>,
    /// Carve-out (a): the standard directory prefixes read through (`Holder::prefix`, `Holder::paths`);
    /// too complex where the command changes one (`changes_prefixes`).
    pub(crate) prefix_names: HashSet<&'static str>,
    /// PE-4: the text of every arithmetic context lexed (`LexInfo::arith_texts`).
    pub(crate) arith_texts: Vec<String>,
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
            holders: Vec::new(),
            subst: false,
            held_entries: HashSet::new(),
            readers: 0,
            script_readers: HashSet::new(),
            reads_seen: HashSet::new(),
            read_names: HashSet::new(),
            setters: HashSet::new(),
            feeders: 0,
            script_feeders: HashSet::new(),
            param_words: Vec::new(),
            unproven: false,
            positional: false,
            positional_uses: Vec::new(),
            positional_known: false,
            top_known: false,
            function_calls: Vec::new(),
            prefix_names: HashSet::new(),
            arith_texts: Vec::new(),
        }
    }

    /// SH-F3: the entries whose segment held a `$(` or a backtick.
    pub(crate) fn held_entries(&self) -> &HashSet<usize> {
        &self.held_entries
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
        self.reads_seen.clear();
        self.read_names.clear();
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

    /// SH-F2: `group` was given a here-string or a heredoc.
    fn has_texts(&self, group: GroupId) -> bool {
        self.groups
            .get(group.0 as usize)
            .is_some_and(|g| !g.texts.is_empty())
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
            .filter(|r| writes_file(r))
            .map(|r| r.target.as_str())
            .collect();
        !written.is_empty()
            && consumers.iter().any(|&c| {
                run_words(&self.entries[c])
                    .iter()
                    .any(|a| written.contains(a.as_str()))
            })
    }

    /// The names the commands `entries` may define as functions, each with its first entry: `f()` lexes as
    /// the command `f` ended by a `(` (`defining`, `Segment::defines`), `function f {` as `function f ...`.
    /// `None` once the analysis has stopped.
    fn defined_names(
        &mut self,
        entries: &[usize],
        defining: &HashSet<usize>,
    ) -> Option<HashMap<String, usize>> {
        let mut defined = HashMap::new();
        for &e in entries {
            let raw = &self.entries[e].raw;
            let name = match raw.as_slice() {
                [name] if defining.contains(&e) => name,
                [function, name, ..] if function == "function" => name,
                _ => continue,
            };
            // PE-1: the shell calls this one with a missing command's words as its positional parameters
            let handler = matches!(
                name.as_str(),
                "command_not_found_handle" | "command_not_found_handler"
            );
            self.unproven |= handler;
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

    /// SH-F4: for each call in `new_entries` of a function an earlier entry defined (`f() {` or `function f`),
    /// with arguments, the commands of its `{ ... }` body (`function_body`) are added once more with the call's
    /// words in place of `$1`... (`positional_words`). False once the analysis has stopped.
    fn call_bodies(
        &mut self,
        new_entries: &[usize],
        defined: &HashMap<String, usize>,
        depth: usize,
    ) -> bool {
        for (k, &e) in new_entries.iter().enumerate() {
            let entry = &self.entries[e];
            let Some(&at) = defined.get(&entry.prog) else {
                continue;
            };
            if at >= e || entry.args.is_empty() {
                continue;
            }
            let Some(start) = new_entries[..k].iter().position(|&x| x == at) else {
                continue;
            };
            let mut params = vec![entry.prog.clone()];
            params.extend(entry.args.iter().cloned());
            let (group, exempt) = (entry.group, entry.exempt);
            let body = function_body(&self.entries, &new_entries[start..k]);
            for words in body {
                let split = positional_split(&words, &params).filter(|t| !t.is_empty());
                for tokens in std::iter::once(positional_words(&words, &params)).chain(split) {
                    if !self.spend(tokens.iter().map(|t| WORD + t.len()).sum()) {
                        return false;
                    }
                    (self.braces, self.quoted_braces) = (false, false);
                    let Some(added) =
                        self.add_command(tokens, Vec::new(), group, exempt, depth + 1, &[])
                    else {
                        return false;
                    };
                    // PE-1: a call's argument that held a parameter expansion became the program
                    // (`f() { "$@"; }; f "$x"`); these words carry no holders
                    self.unproven |= self.entries[added].prog_path.contains('$');
                }
            }
        }
        !self.halted
    }

    /// PE-1: whether every positional program or script of a text (`uses`: entry, `known`) is one the analysis
    /// put the parameters in place for. One in the `{ ... }` body of a function the text defines once is, and
    /// its direct calls after the definition (literal words, the function name first) are recorded: those are
    /// the calls `call_bodies` read, and no other entry may run it (`calls_only_direct`, at the end). One
    /// outside any function body is where the text is a shell's `-c` script with literal arguments (`known`).
    /// A function body the analysis does not read (`f() ( ... )`, zsh `f() cmd`) leaves none read.
    fn positional_read(
        &mut self,
        uses: &[(usize, bool)],
        new_entries: &[usize],
        defining: &HashSet<usize>,
    ) -> bool {
        let mut bodies: Vec<(usize, String, Vec<usize>)> = Vec::new();
        for (k, &e) in new_entries.iter().enumerate() {
            let name = match self.entries[e].raw.as_slice() {
                [name] if defining.contains(&e) => name.clone(),
                [function, name, ..] if function == "function" => name.clone(),
                _ => continue,
            };
            let Some(span) = function_span(&self.entries, &new_entries[k..]) else {
                return false;
            };
            bodies.push((e, name, span.into_iter().map(|(e, _)| e).collect()));
        }
        for &(e, known) in uses {
            let inner = bodies
                .iter()
                .filter(|(_, _, span)| span.contains(&e))
                .max_by_key(|(at, _, _)| *at);
            let Some((at, name, _)) = inner else {
                if known {
                    continue;
                }
                return false;
            };
            if bodies.iter().filter(|(_, n, _)| n == name).count() > 1 {
                return false;
            }
            let calls: HashSet<usize> = new_entries
                .iter()
                .copied()
                .filter(|&k| {
                    let entry = &self.entries[k];
                    k == *at
                        || (k > *at
                            && entry.prog == *name
                            && entry.raw.iter().find(|w| !is_assignment(w)) == Some(name)
                            && !self.held_entries.contains(&k)
                            && !entry.raw.iter().any(|w| w.contains(['$', '`'])))
                })
                .collect();
            self.function_calls.push((name.clone(), calls));
        }
        true
    }

    /// `_collect(text, ctx, exempt, depth)`, memoized on (text, exempt, cwd) as Python's is. Returns true when
    /// `text` was skipped at `MAX_DEPTH` (the caller marks the entry it belongs to `DepthCap`).
    pub(crate) fn collect(&mut self, text: &str, exempt: bool, depth: usize) -> bool {
        let known = std::mem::take(&mut self.positional_known);
        if text.is_empty() || self.halted {
            return false;
        }
        if depth > MAX_DEPTH {
            return true;
        }
        let key: SeenKey = (text.to_string(), exempt, self.cwd.clone(), known);
        if let Some((seen_depth, cwd)) = self.seen.get(&key)
            && *seen_depth <= depth
        {
            // analyzed already: same entries, same cwd
            self.cwd = cwd.clone();
            if self.reads_seen.contains(&key) {
                self.readers += 1;
            }
            return false;
        }
        if !self.spend(1 + 2 * text.len()) {
            return false;
        }
        self.seen.insert(key.clone(), (depth, self.cwd.clone()));
        let (braces, quoted_braces) = (self.braces, self.quoted_braces);
        let readers = self.readers;
        self.collect_once(text, exempt, depth, known);
        (self.braces, self.quoted_braces) = (braces, quoted_braces);
        if self.readers > readers {
            self.reads_seen.insert(key.clone());
        }
        self.seen.insert(key, (depth, self.cwd.clone()));
        false
    }

    /// `_collect_once`. `known`: the analysis puts the text's positional parameters in place (PE-1).
    fn collect_once(&mut self, text: &str, exempt: bool, depth: usize, known: bool) {
        let readers_at_start = self.readers;
        let feeders_at_start = self.feeders;
        let uses_at_start = self.positional_uses.len();
        let LexOut {
            lexed,
            info,
            holders,
            held,
        } = lex_in(text, 0, &mut self.work);
        if lexed.incomplete {
            self.halt(); // Python raises `_TooComplex`
            return;
        }
        self.unproven |= info.unproven;
        self.arith_texts.extend(info.arith_texts);
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
        let mut defining: HashSet<usize> = HashSet::new();
        for ((seg, seg_holders), subst) in lexed.segments.into_iter().zip(holders).zip(held) {
            let group = ids.get(&seg.group).copied().unwrap_or(GroupId(u32::MAX));
            self.arith = seg.arith;
            self.braces = seg.braces;
            self.quoted_braces = seg.quoted_braces;
            self.holders = seg_holders;
            self.subst = subst;
            self.top_known = known;
            match self.add_command(seg.tokens, seg.redirs, group, exempt, depth, &from_text) {
                Some(entry) => {
                    new_entries.push(entry);
                    if seg.braces {
                        braced.insert(entry);
                    }
                    if seg.after_compound {
                        after_compound.insert(entry);
                    }
                    if seg.defines {
                        defining.insert(entry);
                    }
                }
                None => return,
            }
        }
        let Some(defined) = self.defined_names(&new_entries, &defining) else {
            return;
        };
        // SH-F4 (review-sh-f3 I-1): a call of a function the text defines, with arguments, also runs its body
        // with them in place of its positional parameters (`f() { eval "$1"; }; f "$l"`)
        if !self.call_bodies(&new_entries, &defined, depth) {
            return;
        }
        // PE-1: each positional program or script of this text is read only where the analysis put the
        // parameters in place
        let uses = self
            .positional_uses
            .split_off(uses_at_start.min(self.positional_uses.len()));
        if !uses.is_empty() {
            if self.positional_read(&uses, &new_entries, &defining) {
                self.positional = true;
            } else {
                self.unproven = true;
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
        // SH-F3: a `-c`/`eval` script that holds a stdin reader is fed a pipe or a process substitution
        // (`echo X | bash -c sh`, `sh -c 'eval "$(cat)"' < <(…)`), and a compound body after a pipe holds one
        // (`echo X | if true; then sh; fi`): what is fed reaches that reader. Too complex.
        let piped: Vec<usize> = members
            .values()
            .flat_map(|es| es.iter().skip(1).copied())
            .collect();
        let procsub = text.contains("<(") || text.contains(">(");
        let piped_set: HashSet<usize> = piped.iter().copied().collect();
        // SH-F2 review I-1: a script or payload that added an xargs/parallel layer counts too, and so does a
        // here-string or heredoc given to the command (`bash -c 'xargs npx' <<< X`)
        let fed_script = new_entries.iter().any(|&e| {
            (self.script_readers.contains(&e) || self.script_feeders.contains(&e))
                && (procsub || piped_set.contains(&e) || self.has_texts(self.entries[e].group))
        });
        let fed_compound = piped.iter().any(|&e| {
            self.entries[e]
                .raw
                .first()
                .is_some_and(|w| COMPOUND_WORDS.contains(&w.as_str()))
        });
        // SH-F2: so does a variable read from a pipe or a here-string (`echo X | { read l; eval "$l"; }`), and
        // a compound fed a here-string or a heredoc (`while read l; do sh -c "$l"; done <<< X`, `{ sh; } <<< X`)
        let fed_setter = new_entries.iter().any(|&e| {
            self.setters.contains(&e)
                && (piped_set.contains(&e) || self.has_texts(self.entries[e].group))
        });
        // review I-1, I-2: a bare `exec` given a here-string (unless a shell reads a descriptor: `exec 3<<<X;
        // sh <&3` runs X, read below), and a call of a function the text defines that is piped or given a
        // here-string (`f <<< X`, `echo X | f`), feed what runs after or inside them
        let fd_reader = new_entries.iter().any(|&e| self.reads_fd_as_shell(e));
        let fed_closer = info.orphan_texts
            || new_entries.iter().any(|&e| {
                let entry = &self.entries[e];
                let raw = &entry.raw;
                let opener = !fd_reader
                    && entry.prog.is_empty()
                    && raw.first().is_some_and(|w| prog_name(w) == "exec");
                let call = defined.get(&entry.prog).is_some_and(|&at| at < e);
                ((raw.is_empty()
                    || opener
                    || (raw.len() == 1 && matches!(raw[0].as_str(), "}" | "fi" | "done" | "esac")))
                    && self.has_texts(entry.group))
                    || (call && (piped_set.contains(&e) || self.has_texts(entry.group)))
            });
        // review I-2: a reader added before the name it uses was read (`while :; do eval "$l"; read l; done`)
        if self.readers == readers_at_start && !self.read_names.is_empty() {
            for &e in &new_entries {
                match self.reads_stdin_as_shell(e) {
                    Some(true) => {
                        self.readers += 1;
                        break;
                    }
                    Some(false) => {}
                    None => return,
                }
            }
        }
        let readers_added = self.readers > readers_at_start;
        let feeders_added = self.feeders > feeders_at_start;
        if fed_script
            || ((fed_compound || fed_setter) && readers_added)
            || (fed_closer && (readers_added || feeders_added))
        {
            self.halt();
            return;
        }
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
        // SH-F3: and every stdin reader (`> >(su)`, `< <(sudo -s)`, `> >(eval "$(cat)")`)
        let mut consumers: Vec<usize> = Vec::new();
        for &e in &new_entries {
            let named = {
                let prog = self.entries[e].prog.as_str();
                SHELLS.contains(&prog) || prog == "source" || prog == "."
            };
            if named || self.reads_stdin_as_shell(e) == Some(true) {
                consumers.push(e);
            }
        }
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
        // SH-F2: every xargs or parallel layer of a text holding a process substitution is fed the literal
        // output of every `<(...)` in it, as a pipe feeds it (`xargs npx < <(echo X)`, `xargs -a <(echo X) npx`;
        // review I-1: through a compound, a function, `cat` or `exec` too)
        if text.contains("<(") {
            let fed_from_procsub: Vec<usize> = new_entries
                .iter()
                .copied()
                .filter(|e| self.stdin_runs.get(e).is_some_and(|runs| !runs.is_empty()))
                .collect();
            if !fed_from_procsub.is_empty() {
                let Some(inners) = self.process_substitutions(text) else {
                    return;
                };
                let mut fed = Vec::new();
                for inner in inners {
                    let Some(bodies) = self.literal_output(&inner, depth) else {
                        return;
                    };
                    fed.extend(bodies);
                }
                let lines = fed_lines(&fed);
                if !self.spend(lines.iter().map(|l| WORD + l.len()).sum()) {
                    return;
                }
                for e in fed_from_procsub {
                    self.braces = braced.contains(&e);
                    self.feed_stdin(e, &lines, depth);
                    if self.halted {
                        return;
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
        // SH-F2: a program word, a `-c` script or eval words that use a variable read from stdin
        // (`while read l; do sh -c "$l"; done`, `for l in $(cat); do eval "$l"; done`, `$l`); review I-2: and,
        // once a name is read, a shell or `source` whose script is a file descriptor (`sh <(echo "$l")`)
        if !self.read_names.is_empty() {
            let names = &self.read_names;
            let uses = |text: &str| uses_name(text, true, |n| names.contains(n));
            if uses(&entry.prog_path)
                || (prog == "eval" && uses(&entry.args.join(" ")))
                || ((prog == "source" || prog == ".")
                    && first_arg(&entry.args).is_some_and(is_fd_path))
                || (SHELLS.contains(&prog) && {
                    let parsed = shell_parse(prog, &entry.args, 0);
                    match parsed.mode {
                        ShellMode::Command => parsed.scripts.iter().any(|(_, script)| uses(script)),
                        ShellMode::File => {
                            entry.args.get(parsed.start).is_some_and(|f| is_fd_path(f))
                        }
                        _ => false,
                    }
                })
            {
                return Some(true);
            }
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
                ShellMode::Command => {
                    self.held_entries.contains(&e)
                        || parsed.scripts.iter().any(|(_, script)| {
                            script.is_empty() || reads_stdin_substitution(script)
                        })
                }
            });
        }
        if prog == "eval" {
            // SH-seq I-2: `eval "$(cat)"`, `eval '$(</dev/stdin)'`: the words are the script
            return Some(
                self.held_entries.contains(&e)
                    || entry.args.iter().any(String::is_empty)
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
        let feeders = self.feeders;
        let e = self.add_one(tokens, redirs, group, exempt, depth, from_text)?;
        if self.entries[e].prog == "find" || FD_PROGRAMS.contains(&self.entries[e].prog.as_str()) {
            self.exec_walk(e, depth);
        }
        // SH-F2 review I-1: the scripts or payloads of `e` added an xargs/parallel layer
        if self.feeders > feeders + usize::from(self.stdin_runs.contains_key(&e)) {
            self.script_feeders.insert(e);
        }
        if self.halted { None } else { Some(e) }
    }

    /// `_collect_find_exec` and `_collect_fd_exec`, depth-first in Python's order, without recursion.
    fn exec_walk(&mut self, e: usize, depth: usize) {
        // PE-1: the words of `e`'s segment that held a parameter expansion; every payload comes from them
        let params = std::mem::take(&mut self.param_words);
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
            // SH-F3: a payload that reads stdin (`find . -exec sh \;`) makes the command running it a stdin
            // reader, and a runtime substitution in the command (`-exec sh -c "$(cat) " \;`) is one in its payload
            self.subst = self.held_entries.contains(&e);
            let Some(child) = self.add_one(payload, Vec::new(), group, exempt, depth, &[]) else {
                return;
            };
            // PE-1: a payload's program or script from a parameter expansion (`find . -exec $x ... \;`,
            // `-exec sh -c "$x" \;`); its words carry no holders, so any `$` in the program word counts
            // A program from the found file's name (`-exec {} \;`, fd's `{.}`) is only known at run time too
            let path = &self.entries[child].prog_path;
            self.unproven |= path.contains('$')
                || (path.contains('{') && path.contains('}'))
                || param_script(&params, &entry_scripts(&self.entries[child]));
            if self.reads_stdin_as_shell(child) == Some(true) {
                self.launchers.insert(e);
            }
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
        let holders = std::mem::take(&mut self.holders);
        let subst = std::mem::take(&mut self.subst);
        let known = std::mem::take(&mut self.top_known);
        let size: usize = tokens.iter().map(|t| WORD + t.len()).sum();
        if !self.spend(1 + 2 * size) {
            return None;
        }
        if fail_closed(&tokens, self.braces, self.quoted_braces) {
            self.halt();
            return None;
        }
        // F-8(b): the words as they are read, with a substitution at program position resolved (`which X`)
        let result = resolve_holders(&tokens, &holders, &mut self.work);
        let resolved = self.take(result)?;
        let words = resolved.as_deref().unwrap_or(tokens.as_slice());
        // PE-1: a program word from a parameter expansion. Carve-out (a): unless it is a standard directory
        // prefix (`"$HOME/.cargo/bin/cargo"`), read with HOME in place (any other prefix is an unknown
        // directory); `changes_prefixes` then checks the command never changes it
        let result = param_program(words, &holders, &mut self.work);
        let program = self.take(result)?;
        let carved = program
            .as_ref()
            .and_then(|(_, k)| *k)
            .and_then(|k| holders.iter().find(|h| h.param && h.index == k))
            .and_then(|h| h.prefix.map(|name| (h.index, name)));
        let mut owned: Option<Vec<String>> = None;
        if let Some((k, name)) = carved {
            self.prefix_names.insert(name);
            if name == "HOME" {
                let word = &words[k];
                let rest = word
                    .strip_prefix("${HOME}")
                    .or_else(|| word.strip_prefix("$HOME"))
                    .unwrap_or(word);
                let mut copy = words.to_vec();
                copy[k] = format!("{}{rest}", self.home);
                owned = Some(copy);
            }
        }
        // zsh runs a program word `=npx` as the path of `npx`
        let current = owned.as_deref().unwrap_or(words);
        let result = held_program(current, |k| equals_program(&current[k]), &mut self.work);
        if let Some((_, Some(k))) = self.take(result)? {
            owned.get_or_insert_with(|| words.to_vec())[k].remove(0);
        }
        let words: &[String] = owned.as_deref().unwrap_or(words);
        let mut alternates = Vec::new();
        let mut runs = Vec::new();
        let result = strip_prefixes(
            words,
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
        // F-8(b): a program word whose `${...}` holds a substitution (`${x#$(f)}`) is too complex; the lexer keeps
        // such a word's text, and no other word text is a substitution
        if cmd
            .first()
            .is_some_and(|w| w.contains("${") && (w.contains("$(") || w.contains('`')))
        {
            self.halt();
            return None;
        }
        let prog_path = cmd.first().map(|w| pystr::lower(w)).unwrap_or_default();
        let prog = cmd.first().map(|w| prog_name(w)).unwrap_or_default();
        let args: Vec<String> = cmd.get(1..).map(<[String]>::to_vec).unwrap_or_default();
        // PE-1, PE-3, PE-5, PE-6: a program from a parameter expansion, code from a value, a nested
        // subscript, a renamed program. The analysis goes on and is too complex at its end (`unproven`), so
        // a rule that blocks a reading still names itself (`${x:-npx} playwright install` is G1)
        let positional_program = program.as_ref().is_some_and(|(w, _)| positional(w));
        // a program word with an unquoted glob character runs whatever file matches (`/usr/bin/np?`)
        let result = held_program(
            words,
            |k| holders.iter().any(|h| h.glob && !h.param && h.index == k),
            &mut self.work,
        );
        let globbed = self
            .take(result)?
            .is_some_and(|(w, _)| !TEST_BRACKETS.contains(&w.as_str()) && !element_assignment(&w));
        let unproven = (program.is_some() && !positional_program && carved.is_none())
            || globbed
            || value_code_words(&prog, &args, &tokens)
            || renames_program(&prog, &args, &tokens)
            || tokens.iter().any(|t| nested_subscript_runs(t));
        let prefix = &words[..words.len().saturating_sub(cmd.len())];
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
        // SH-F2: the variables this command sets from what it reads, before it is itself read as a stdin reader
        let names = setter_names(&prog, &args, &tokens, subst, &self.read_names);
        if !self.spend(names.iter().map(|n| WORD + n.len()).sum()) {
            return None;
        }
        let setter = !names.is_empty();
        self.read_names.extend(names);
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
        // PE-1: and a script from a parameter expansion (`eval "$x"`, `bash -c "$x"`); not the reading with a
        // shell's positional parameters put in place below, which holds their values as the script's data
        let params = param_words(&tokens, &holders);
        // carve-out (a): a double-quoted script whose only expansions are `$HOME/` and `$PWD/` (`sh -c "cd
        // $HOME/x && make"`), read with HOME in place as the outer shell puts it there
        let paths: Vec<(&String, &Vec<&'static str>)> = holders
            .iter()
            .filter(|h| h.param)
            .filter_map(|h| Some((tokens.get(h.index)?, h.paths.as_ref()?)))
            .collect();
        let (mut shown, mut other) = (Vec::new(), Vec::new());
        for word in &params {
            if positional(word) {
                shown.push(word.clone());
            } else if !paths.iter().any(|(p, _)| *p == word) {
                other.push(word.clone());
            }
        }
        for (word, names) in &paths {
            if scripts.iter().any(|s| s.contains(word.as_str())) {
                self.prefix_names.extend(names.iter().copied());
            }
        }
        for script in scripts.iter_mut() {
            if paths.iter().any(|(p, _)| *p == script) {
                let home = format!("{}/", self.home);
                *script = script.replace("${HOME}/", &home).replace("$HOME/", &home);
            }
        }
        let positional_use = positional_program || param_script(&shown, &scripts);
        self.unproven |= unproven || param_script(&other, &scripts);
        self.param_words = params;
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
        if positional_use {
            self.positional_uses.push((e, known));
        }
        if !runs.is_empty() {
            self.stdin_runs.insert(e, runs);
            self.feeders += 1;
        }
        if peeled.shell_launch {
            self.launchers.insert(e);
        }
        if subst {
            self.held_entries.insert(e);
        }
        if setter {
            self.setters.insert(e);
        }
        if self.reads_stdin_as_shell(e) == Some(true) {
            self.readers += 1;
        }
        // SH-F2 review I-2: a shell's positional parameters put in place in its script (`sh -c 'eval "$1"' _
        // "$l"`); SH-F4 (review-sh-f3 I-1): whatever they hold (`sh -c '"$@"' _ npx playwright install`).
        // PE-1: with literal arguments that reading is the script's, so its own positional programs are read
        let mut known_script = None;
        if SHELLS.contains(&prog.as_str()) && prog != "fish" {
            let parsed = shell_parse(&prog, &args, 0);
            if parsed.mode == ShellMode::Command
                && let Some(script) = args.get(parsed.start)
                && args.len() > parsed.start + 1
            {
                let params = &args[parsed.start + 1..];
                let text = positional_script(script, params);
                if !self.spend(WORD + text.len()) {
                    return None;
                }
                let literal = !subst
                    && !peeled.stdin_args
                    && !peeled.placeholder
                    && !holders.iter().any(|h| h.param || h.subst)
                    && params
                        .iter()
                        .all(|a| !a.contains(['$', '`']) && !a.contains("{}"));
                if text != *script {
                    if literal {
                        known_script = Some(script.clone());
                    }
                    scripts.push(text);
                }
            }
        }
        let readers = self.readers;
        for script in &scripts {
            self.positional_known = known_script.as_ref() == Some(script);
            if self.collect(script, exempt, depth + 1) {
                mark(&mut self.entries[e].opaque, Opaque::DepthCap);
            }
        }
        if self.readers > readers {
            self.script_readers.insert(e);
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

/// `which X`, `command -v X`, `type -p X` and `realpath X` (F-8(b)): the one literal word X the body names, which
/// bash runs as the program. Anything else is `None`.
fn lookup_program(body: &str) -> Option<String> {
    let words: Vec<&str> = body.split_whitespace().collect();
    let name = match words.as_slice() {
        ["which" | "realpath", name] => *name,
        ["command", "-v", name] | ["type", "-p", name] => *name,
        _ => return None,
    };
    let literal = !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._+@/-".contains(c));
    (literal && !name.is_empty()).then(|| name.to_string())
}

/// F-8(b): the words of a segment with each substitution at program position resolved. The program word is the
/// one `strip_prefixes` reads: each holder (a word that held a `$(...)` or a backtick) is replaced by a stand-in
/// in a probe copy, and a holder the probe reads as the program is too complex, unless it is the sole word and
/// `lookup_program` reads it as X (then X is the program, `$(which npx) playwright install` is `npx playwright
/// install`). `None`: no holder is at program position, and the words stand as they are.
fn resolve_holders(
    tokens: &[String],
    holders: &[Holder],
    work: &mut Work,
) -> Result<Option<Vec<String>>, TooComplex> {
    // a word with only a parameter expansion is `param_program`'s
    if !holders.iter().any(|h| h.subst) {
        return Ok(None);
    }
    let mark = stand_in(tokens);
    let mut words: Option<Vec<String>> = None;
    let mut open: Vec<&Holder> = holders.iter().filter(|h| h.subst).collect();
    while !open.is_empty() {
        let current = words.as_deref().unwrap_or(tokens);
        let probe: Vec<String> = current
            .iter()
            .enumerate()
            .map(|(k, w)| {
                if open.iter().any(|h| h.index == k) && !is_assignment(w) {
                    format!("{mark}{k}")
                } else {
                    w.clone()
                }
            })
            .collect();
        let stripped = strip_prefixes(&probe, None, None, true, work)?;
        let at = stripped
            .cmd
            .first()
            .and_then(|w| w.strip_prefix(mark.as_str()))
            .and_then(|k| k.parse::<usize>().ok());
        let Some(k) = at else { break };
        let pos = open.iter().position(|h| h.index == k).ok_or(TooComplex)?;
        let holder = open.swap_remove(pos);
        let program = lookup_program(&holder.body)
            .filter(|_| holder.sole)
            .ok_or(TooComplex)?;
        words.get_or_insert_with(|| tokens.to_vec())[k] = program;
    }
    Ok(words)
}

/// A stand-in marker that no word of `tokens` contains.
fn stand_in(tokens: &[String]) -> String {
    let mut n = 0;
    loop {
        let mark = format!("\u{1}{n}\u{1}");
        if !tokens.iter().any(|t| t.contains(&mark)) {
            return mark;
        }
        n += 1;
    }
}

/// PE-1: the program word `strip_prefixes` reads from `words` (past assignments, keywords, wrappers such as
/// `env`, `command`, `exec`, `nice`, `time`, `sudo`, `xargs`) is a word that held a parameter expansion
/// (`$x`, `"${x}"`, `$1`, `"$@"`), so the program is only known at run time. Found as `resolve_holders` finds
/// a substitution: each such word is replaced by a stand-in in a probe copy. `Some(word)`: that word.
fn param_program(
    words: &[String],
    holders: &[Holder],
    work: &mut Work,
) -> Result<Option<(String, Option<usize>)>, TooComplex> {
    held_program(
        words,
        |k| holders.iter().any(|h| h.param && h.index == k),
        work,
    )
}

/// `param_program` for the words `held` accepts (by index): the program word and, when it is one word of
/// `words`, its index.
fn held_program(
    words: &[String],
    held: impl Fn(usize) -> bool,
    work: &mut Work,
) -> Result<Option<(String, Option<usize>)>, TooComplex> {
    if !(0..words.len()).any(&held) {
        return Ok(None);
    }
    let mark = stand_in(words);
    let probe: Vec<String> = words
        .iter()
        .enumerate()
        .map(|(k, w)| {
            if held(k) && !is_assignment(w) {
                format!("{mark}{k}")
            } else {
                w.clone()
            }
        })
        .collect();
    let stripped = strip_prefixes(&probe, None, None, true, work)?;
    Ok(stripped
        .cmd
        .first()
        .and_then(|w| w.strip_prefix(mark.as_str()))
        .and_then(|k| k.parse::<usize>().ok())
        .and_then(|k| words.get(k).map(|w| (w.clone(), Some(k))))
        .or_else(|| {
            // a stand-in inside a longer word (`env -S`'s split words): the program is from it anyway
            stripped
                .cmd
                .first()
                .filter(|w| w.contains(&mark))
                .map(|w| (w.clone(), None))
        }))
}

/// zsh's `=cmd` (EQUALS): the word is the path of `cmd`. Not `==`, not `=(...)`.
fn equals_program(word: &str) -> bool {
    word.len() > 1 && word.starts_with('=') && !word.starts_with("==") && !word.starts_with("=(")
}

/// `name[...]=value` or `name[...]+=value`: an array element assignment.
fn element_assignment(word: &str) -> bool {
    word.find('[').is_some_and(|open| {
        is_name(&word[..open]) && (word[open..].contains("]=") || word[open..].contains("]+="))
    })
}

/// PE-1: `word` is only a positional parameter (`$1`, `${2}`, `$@`, `$*`, `${@}`). The analysis puts a
/// shell's literal arguments (`sh -c S _ a b`) and a function's direct calls' arguments in place of these;
/// anywhere else they are unknown (`Ctx::positional_read`), and so are they where the command sets them some
/// other way (`sets_positionals`).
fn positional(word: &str) -> bool {
    let inner = word
        .strip_prefix("${")
        .and_then(|w| w.strip_suffix('}'))
        .filter(|w| {
            !w.is_empty() && (w.bytes().all(|c| c.is_ascii_digit()) || *w == "@" || *w == "*")
        });
    inner.is_some()
        || word.strip_prefix('$').is_some_and(|w| {
            w.len() == 1 && (w.as_bytes()[0].is_ascii_digit() || w == "@" || w == "*")
        })
}

/// PE-1: a command sets or moves the positional parameters (`set a b`, `set -- a`, `shift`, zsh's `argv=(a
/// b)`, `read -A argv`, any word naming `argv`), so a positional parameter can hold a program the analysis
/// did not put in place.
pub(crate) fn sets_positionals(entries: &[Entry]) -> bool {
    entries.iter().any(|e| {
        if e.prog == "shift" || e.raw.iter().any(|w| w.contains("argv")) {
            return true;
        }
        let operand = e.prog == "set" && {
            let mut skip = false;
            e.args.iter().any(|a| {
                if std::mem::take(&mut skip) {
                    return false;
                }
                if a == "--" || !a.starts_with(['-', '+']) {
                    return true;
                }
                skip = a.ends_with('o'); // `set -o pipefail`, `-euo pipefail`: the option name
                false
            })
        };
        operand
            || e.raw.iter().any(|w| {
                w.strip_prefix("argv")
                    .is_some_and(|r| r.starts_with(['=', '[', '+']))
            })
    })
}

/// PE-1: every entry (and reading) of the analysis that runs a function whose body's positional parameters
/// were read with its calls' arguments in place is one of those calls (`Ctx::function_calls`); not a call
/// from a `trap`, an `eval`, a wrapper (`xargs f`), another text or a later reading.
pub(crate) fn calls_only_direct(
    entries: &[Entry],
    alternates: &[Entry],
    calls: &[(String, HashSet<usize>)],
) -> bool {
    let mut direct: HashMap<&str, HashSet<usize>> = HashMap::new();
    for (name, found) in calls {
        direct
            .entry(name.as_str())
            .or_default()
            .extend(found.iter().copied());
    }
    direct.iter().all(|(name, found)| {
        entries
            .iter()
            .enumerate()
            .all(|(k, e)| e.prog != *name || found.contains(&k))
            && alternates.iter().all(|a| a.prog != *name)
    })
}

/// Carve-out (a): a command that may change a standard directory prefix read through (`names`): an
/// assignment to it (`HOME=/tmp`, `export HOME=/tmp`, `for HOME in ...`), a name `read`, `mapfile`,
/// `declare`, `unset`, `printf -v` and the like set, an arithmetic assignment (`(( HOME = 1 ))`), a default
/// assigned (`${TMPDIR:=/x}`), any nameref, `source`, `.`, `eval`, `set -a`; for `PWD` also `cd`, `pushd`,
/// `popd`.
pub(crate) fn changes_prefixes(
    entries: &[Entry],
    arith_texts: &[String],
    names: &HashSet<&'static str>,
) -> bool {
    let mut evaluated = HashSet::new();
    for text in arith_texts {
        // a text that cannot be followed may assign any name
        if expression_names(text, &mut evaluated).is_none() {
            return true;
        }
    }
    if names.iter().any(|n| evaluated.contains(*n)) {
        return true;
    }
    let defaults = |w: &String| {
        names.iter().any(|n| {
            [":=", "="]
                .iter()
                .any(|op| w.contains(&format!("{{{n}{op}")))
        })
    };
    if entries.iter().any(|e| e.raw.iter().any(defaults)) {
        return true;
    }
    const SETTERS: &[&str] = &[
        "export",
        "declare",
        "typeset",
        "local",
        "readonly",
        "read",
        "mapfile",
        "readarray",
        "unset",
        "printf",
        "vared",
        "getopts",
        "let",
        "integer",
        "float",
        "for",
        "select",
    ];
    let name_of = |w: &str| w.split(['=', '[', '+']).next().unwrap_or("").to_string();
    entries.iter().any(|e| {
        // `builtin cd`, `command eval`: the word after them
        let prog = match e.prog.as_str() {
            "builtin" | "command" => e.args.first().map_or("", String::as_str),
            prog => prog,
        };
        matches!(prog, "source" | "." | "eval")
            || (names.contains("PWD") && matches!(prog, "cd" | "pushd" | "popd" | "chdir"))
            || e.raw
                .iter()
                .any(|w| is_assignment(w) && names.contains(name_of(w).as_str()))
            || (SETTERS.contains(&prog)
                && e.args.iter().any(|a| names.contains(name_of(a).as_str())))
            || (matches!(prog, "declare" | "typeset" | "local" | "readonly")
                && e.args
                    .iter()
                    .any(|a| a.len() > 1 && a.starts_with('-') && a[1..].contains('n')))
            || (prog == "set"
                && e.args.iter().any(|a| {
                    a == "allexport"
                        || (a.starts_with('-') && !a.starts_with("--") && a.contains('a'))
                }))
    })
}

/// PE-1: a script the command runs (`eval`'s words, a shell's `-c` script, `watch`'s or `trap`'s command, a
/// command variable) holds a word that held a parameter expansion: the outer shell puts the value in before
/// the script is read (`eval "$x"`, `bash -c "echo; $x"`), so the script is only known at run time. A
/// single-quoted script (`sh -c 'echo "$1"' _ x`) holds no such word. `params`: those words (`param_words`).
fn param_script(params: &[String], scripts: &[String]) -> bool {
    params
        .iter()
        .any(|w| scripts.iter().any(|s| s.contains(w.as_str())))
}

/// PE-1: the words of `tokens` that held a parameter expansion (`Holder::param`), non-empty.
fn param_words(tokens: &[String], holders: &[Holder]) -> Vec<String> {
    holders
        .iter()
        .filter(|h| h.param)
        .filter_map(|h| tokens.get(h.index))
        .filter(|w| !w.is_empty())
        .cloned()
        .collect()
}

/// The scripts a shell's `-c` or `eval` of `entry` runs (for `param_script`).
fn entry_scripts(entry: &Entry) -> Vec<String> {
    let prog = entry.prog.as_str();
    if SHELLS.contains(&prog) {
        shell_parse(prog, &entry.args, 0)
            .scripts
            .into_iter()
            .map(|(_, s)| s)
            .collect()
    } else if prog == "eval" {
        vec![entry.args.join(" ")]
    } else {
        Vec::new()
    }
}

/// PE-6: a command that makes a name run something other than the program of that name: an alias
/// definition (`alias u='npx ...'`, zsh `alias -g`/`-s`), a hashed path (`hash -p /path/npx u`, zsh
/// `hash u=/path/npx`), or an assignment to the tables behind them (bash `BASH_ALIASES`, `BASH_CMDS`; zsh
/// `aliases`, `galiases`, `saliases`, `commands`, and `functions`, which defines a function from text). The
/// analysis does not follow them, so a later word could be any command. A bash table is named in any word
/// (`printf -v 'BASH_CMDS[u]'`, `declare -n r=BASH_CMDS`), and so is an exported function (`BASH_FUNC_u%%=`,
/// which a child bash defines); a zsh table with a subscript or a value (`commands[u]`, `aliases+=(...)`),
/// or as a name `set -A`, `typeset`, `read` and the like set (`set -A commands u /path`).
fn renames_program(prog: &str, args: &[String], tokens: &[String]) -> bool {
    const BASH_TABLES: &[&str] = &["BASH_ALIASES", "BASH_CMDS", "BASH_FUNC_"];
    const SETTERS: &[&str] = &[
        "set",
        "typeset",
        "declare",
        "local",
        "readonly",
        "export",
        "read",
        "printf",
        "unset",
        "vared",
        "mapfile",
        "readarray",
        "integer",
        "float",
    ];
    if (prog == "alias" || prog == "hash") && args.iter().any(|a| a.contains('=')) {
        return true;
    }
    if prog == "hash" && args.iter().any(|a| a.starts_with('-') && a.contains('p')) {
        return true;
    }
    tokens.iter().any(|t| {
        let name = t.split(['[', '=', '+']).next().unwrap_or("");
        BASH_TABLES.iter().any(|table| t.contains(table))
            || (ZSH_TABLES.contains(&name) && t.len() > name.len())
            || (SETTERS.contains(&prog) && ZSH_TABLES.contains(&t.as_str()))
    })
}

/// PE-6: zsh's tables of aliases, commands and functions.
const ZSH_TABLES: &[&str] = &[
    "aliases",
    "galiases",
    "saliases",
    "dis_aliases",
    "dis_galiases",
    "dis_saliases",
    "commands",
    "functions",
    "dis_functions",
];

/// PE-3: words that turn on code from values: zsh's `GLOB_SUBST` (a value's glob qualifiers run) and
/// `PROMPT_SUBST` (`print -P`, prompts and `${(%)x}` run a value's `$(...)`), spelled any way zsh accepts
/// (`setopt glob_subst`, `set -o GLOBSUBST`, `zsh -o globsubst`, `options[promptsubst]=on`); and a prompt
/// variable set or read in from a value that can hold a substitution (`PS4='$(cmd)'` with `set -x` runs it in
/// bash; `read PS4`), and the startup file names bash and sh expand (`BASH_ENV='$(cmd)' bash -c :`). Also a
/// nameref to one of those or to a table (`declare -n p=PS4`), or one the analysis cannot follow (`local -n
/// r=$1`, a bare `declare -n r`); a name `declare` and the like set from an expansion (`declare "$x=..."`); and
/// an option name from an expansion (`setopt ${o}subst`, `set -o $o`).
fn value_code_words(prog: &str, args: &[String], tokens: &[String]) -> bool {
    const PROMPTS: &[&str] = &[
        "BASH_ENV",
        "ENV",
        "PS1",
        "PS2",
        "PS3",
        "PS4",
        "PROMPT",
        "PROMPT2",
        "PROMPT3",
        "PROMPT4",
        "RPROMPT",
        "RPROMPT2",
        "RPS1",
        "RPS2",
        "PROMPT_COMMAND",
    ];
    const NAMERS: &[&str] = &[
        "read",
        "mapfile",
        "readarray",
        "printf",
        "declare",
        "typeset",
        "local",
        "export",
        "readonly",
    ];
    let options = tokens.iter().any(|t| {
        let flat: String = t
            .chars()
            .filter(|c| *c != '_')
            .collect::<String>()
            .to_ascii_lowercase();
        flat.contains("globsubst") || flat.contains("promptsubst")
    });
    let prompt = tokens.iter().any(|t| {
        t.split_once('=').is_some_and(|(name, value)| {
            PROMPTS.contains(&name.trim_end_matches('+')) && value.contains(['$', '`'])
        })
    });
    let named = NAMERS.contains(&prog) && args.iter().any(|a| PROMPTS.contains(&a.as_str()));
    let nameref = matches!(prog, "declare" | "typeset" | "local" | "readonly")
        && args
            .iter()
            .any(|a| a.len() > 1 && a.starts_with('-') && a[1..].contains('n'))
        && args.iter().filter(|a| !a.starts_with(['-', '+'])).any(|a| {
            let target = a.split_once('=').map(|(_, v)| v);
            target.is_none_or(|v| {
                let name = v.split('[').next().unwrap_or("");
                v.contains(['$', '`'])
                    || PROMPTS.contains(&name)
                    || ZSH_TABLES.contains(&name)
                    || name.starts_with("BASH_")
            })
        });
    // a name from an expansion may be any of those (`x=PS4; declare "$x=$(...)"`)
    let dynamic_name = matches!(
        prog,
        "declare" | "typeset" | "local" | "readonly" | "export"
    ) && args.iter().any(|a| {
        !a.starts_with(['-', '+'])
            && a.split('=')
                .next()
                .is_some_and(|name| name.contains(['$', '`']))
    });
    let dynamic_option = match prog {
        "setopt" | "unsetopt" => args.iter().any(|a| a.contains(['$', '`'])),
        "set" => args.windows(2).any(|pair| {
            pair[0].starts_with(['-', '+'])
                && pair[0].ends_with('o')
                && pair[1].contains(['$', '`'])
        }),
        _ => false,
    };
    options || prompt || named || nameref || dynamic_name || dynamic_option
}

/// PE-5: a subscript `name[...]` whose text before its first `]` holds another `[` or a quote, with a closed
/// `$(...)`, backtick pair or `$'...'` after its `[`. bash and zsh count nested brackets and quotes when they
/// find the subscript's end (`a[[]$(cmd)]`, `a["]"$(cmd)]` run `cmd`), where `subscript_runs` closes it at the
/// first `]`. Too complex.
fn nested_subscript_runs(word: &str) -> bool {
    let b = word.as_bytes();
    let Some(close) = word.rfind(']') else {
        return false;
    };
    let last = |closer: u8| b[..close].iter().rposition(|&c| c == closer);
    let (tick, paren, quote) = (last(b'`'), last(b')'), last(b'\''));
    let runs_from = |from: usize| {
        (from..close).any(|k| match b[k] {
            b'`' => tick.is_some_and(|t| t > k),
            b'$' => match b.get(k + 1) {
                Some(b'(') => paren.is_some_and(|p| p > k + 1),
                Some(b'\'') => quote.is_some_and(|q| q > k + 1),
                _ => false,
            },
            _ => false,
        })
    };
    (1..close).any(|j| {
        b[j] == b'['
            && (b[j - 1].is_ascii_alphanumeric() || b[j - 1] == b'_')
            && b[j + 1..]
                .iter()
                .take_while(|&&c| c != b']')
                .any(|c| matches!(c, b'[' | b'"' | b'\''))
            && runs_from(j + 1)
    })
}

/// PE-4: whether the shell evaluates, as an expression or a subscript, a variable that may hold a value the
/// analysis cannot see (`read s <<< '[$(cmd)]'; unset "a$s"`, `echo $(( s ))` with `s='a[$(cmd)]'`). The
/// evaluated names (`evaluated_names`) are those of the arithmetic contexts (`arith_texts`), `let`, `[[` with
/// an arithmetic operator or `-v`, and the subscripts and offsets of every word; carve-out (b): it is only too
/// complex when one of them is a name the command sets from what it reads or from an expansion
/// (`tainted_names`), so a read loop that counts (`n=$((n+1))`) is read. A name the analysis cannot follow
/// (an indirect `${!s}`, a `$` in a name `unset`, `declare`, `printf -v` and the like evaluate, an `-i`
/// declaration, a positional parameter, a nameref) gives up: any value may be evaluated.
pub(crate) fn evaluates_values(
    entries: &[Entry],
    arith_texts: &[String],
    held: &HashSet<usize>,
) -> bool {
    let Some(evaluated) = evaluated_names(entries, arith_texts) else {
        return true;
    };
    if evaluated.is_empty() {
        return false;
    }
    match tainted_names(entries, held) {
        Some(tainted) => evaluated.iter().any(|n| tainted.contains(n)),
        None => true,
    }
}

/// PE-4: the names `text` (an expression, a subscript, an offset) evaluates: every identifier not part of
/// a number (`16#ff`, `0x1f`). `None` when it holds a name the analysis cannot follow: an indirect `${!`,
/// or a positional parameter (`$1`, `${2}`, `$@`, `$*`).
fn expression_names(text: &str, out: &mut HashSet<String>) -> Option<()> {
    let b = text.as_bytes();
    if text.contains("${!") {
        return None;
    }
    for k in 0..b.len() {
        if b[k] == b'$' {
            let next = if b.get(k + 1) == Some(&b'{') {
                b.get(k + 2)
            } else {
                b.get(k + 1)
            };
            if next.is_some_and(|c| c.is_ascii_digit() || matches!(c, b'@' | b'*')) {
                return None;
            }
        }
    }
    let mut k = 0;
    while k < b.len() {
        let starts = (b[k].is_ascii_alphabetic() || b[k] == b'_')
            && (k == 0 || !(b[k - 1].is_ascii_alphanumeric() || matches!(b[k - 1], b'_' | b'#')));
        if !starts {
            k += 1;
            continue;
        }
        let end = (k..b.len())
            .find(|&j| !(b[j].is_ascii_alphanumeric() || b[j] == b'_'))
            .unwrap_or(b.len());
        out.insert(text[k..end].to_string());
        k = end;
    }
    Some(())
}

/// PE-4: every name the command evaluates (`evaluates_values`); `None` when one cannot be followed.
fn evaluated_names(entries: &[Entry], arith_texts: &[String]) -> Option<HashSet<String>> {
    const ARITH_TESTS: &[&str] = &["-eq", "-ne", "-lt", "-le", "-gt", "-ge", "-v"];
    let mut out = HashSet::new();
    for text in arith_texts {
        expression_names(text, &mut out)?;
    }
    for e in entries {
        let (prog, args) = (e.prog.as_str(), e.args.as_slice());
        // a `$` in a name the program evaluates (`unset "a$s"`, `export "$k=$v"`), not in a value (`x=$v`)
        let dollar = args
            .iter()
            .any(|a| a.split('=').next().is_some_and(|name| name.contains('$')));
        match prog {
            "let" => {
                for arg in args {
                    expression_names(arg, &mut out)?;
                }
            }
            "[[" => {
                if args.iter().any(|a| ARITH_TESTS.contains(&a.as_str())) {
                    for arg in args.iter().filter(|a| !ARITH_TESTS.contains(&a.as_str())) {
                        expression_names(arg, &mut out)?;
                    }
                }
            }
            "declare" | "typeset" | "local" | "readonly" | "export" => {
                let integer = args
                    .iter()
                    .any(|a| a.len() > 1 && a.starts_with(['-', '+']) && a.contains('i'));
                if dollar || integer {
                    return None;
                }
            }
            _ if SUBSCRIPT_PROGRAMS.contains(&prog) && dollar => return None,
            _ if SUBSCRIPT_V_PROGRAMS.contains(&prog)
                && dollar
                && args.iter().any(|a| a.starts_with("-v")) =>
            {
                return None;
            }
            _ => {}
        }
        for word in &e.raw {
            word_evaluates(word, &mut out)?;
        }
    }
    Some(out)
}

/// PE-4: the names `word` evaluates: those in a subscript after a name character (`a[$s]`, `${a[s]}`; not
/// `${a[@]}`), and in a `${x:...}` offset (`${x:$s}`, `${x:s}`; not `${x:-y}`). An indirect `${!x}` cannot
/// be followed (`None`).
fn word_evaluates(word: &str, out: &mut HashSet<String>) -> Option<()> {
    let b = word.as_bytes();
    if word.contains("${!") {
        return None;
    }
    for j in 1..b.len() {
        if b[j] == b'[' && (b[j - 1].is_ascii_alphanumeric() || b[j - 1] == b'_') {
            let end = (j + 1..b.len()).find(|&k| b[k] == b']').unwrap_or(b.len());
            expression_names(&word[j + 1..end], out)?;
        }
    }
    for k in 0..b.len() {
        if b[k] == b'$'
            && b.get(k + 1) == Some(&b'{')
            && let Some(end) = param_close(b, k)
            && let Some((_, rest)) = word[k + 2..end].split_once(':')
            && !rest.starts_with(['-', '=', '+', '?'])
        {
            expression_names(rest, out)?;
        }
    }
    Some(())
}

/// PE-4: the names the command sets from what it reads or from an expansion: the names `read`, `mapfile`,
/// `readarray`, `getopts` and `printf -v` set (and `REPLY`, `MAPFILE`, `OPTARG`), a `for`/`select` name over
/// words holding an expansion, and every assignment whose value holds a `$` or a backtick, or whose segment
/// held a substitution (`held`). An arithmetic `n=$((n+1))` keeps no text, so it is not one. `None` with a
/// nameref, which may set any name.
fn tainted_names(entries: &[Entry], held: &HashSet<usize>) -> Option<HashSet<String>> {
    let mut out = HashSet::new();
    for (k, e) in entries.iter().enumerate() {
        let (prog, args) = (e.prog.as_str(), e.args.as_slice());
        let names = args.iter().filter(|a| is_name(a)).cloned();
        match prog {
            "read" => {
                out.insert("REPLY".to_string());
                out.extend(names);
            }
            "mapfile" | "readarray" => {
                out.insert("MAPFILE".to_string());
                out.extend(names);
            }
            "getopts" => {
                out.insert("OPTARG".to_string());
                out.extend(names);
            }
            "printf" => {
                if let Some(at) = args.iter().position(|a| a == "-v") {
                    out.extend(args.get(at + 1).cloned());
                }
            }
            "for" | "select" => {
                let expanded =
                    held.contains(&k) || args.iter().skip(1).any(|a| a.contains(['$', '`']));
                if expanded && let Some(name) = args.first() {
                    out.insert(name.clone());
                }
            }
            "declare" | "typeset" | "local" | "readonly"
                if args
                    .iter()
                    .any(|a| a.len() > 1 && a.starts_with('-') && a[1..].contains('n')) =>
            {
                return None;
            }
            _ => {}
        }
        for word in e.raw.iter().filter(|w| is_assignment(w)) {
            let Some((name, value)) = word.split_once('=') else {
                continue;
            };
            if held.contains(&k) || value.contains(['$', '`']) {
                let name = name.split(['[', '+']).next().unwrap_or(name);
                out.insert(name.to_string());
            }
        }
    }
    Some(out)
}

/// PE-4: the command sets a value the analysis cannot see: a `read`, `mapfile` or `readarray` (from a
/// here-string, a heredoc, a pipe or a file), or a value holding a closed `$(...)` or backtick pair as text,
/// which runs when the shell evaluates the value: an assignment (`x='$(cmd)'`, `declare x='$(cmd)'`) or an
/// argument of `set` or `printf` (`set -- '$(cmd)'`, `printf -v x %s '$(cmd)'`).
pub(crate) fn unseen_values(entries: &[Entry]) -> bool {
    // a `$(` that is not a `$((`, with a `)` after it
    let code = |w: &String| {
        w.match_indices("$(")
            .any(|(k, _)| !w[k + 2..].starts_with('(') && w[k + 2..].contains(')'))
            || w.matches('`').count() >= 2
    };
    entries.iter().any(|e| {
        matches!(e.prog.as_str(), "read" | "mapfile" | "readarray")
            || e.raw.iter().any(|w| is_assignment(w) && code(w))
            || (matches!(e.prog.as_str(), "set" | "printf") && e.args.iter().any(code))
    })
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
