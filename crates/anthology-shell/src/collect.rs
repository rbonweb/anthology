//! Entry collection (SH2, fix-waves H and I): guard_rules.py `_Context`, `_add_command`, `_add_alternates`,
//! `_collect_find_exec`, `_collect_fd_exec`, `_reads_stdin_as_shell`, `_reads_fd_as_shell`, `_feed_stdin`,
//! `_process_substitutions`, `_literal_output`, `_echo_text`, `_first_arg`, `_collect`, `_collect_once`, and the
//! path helpers `_expand` and `_normalize` (which the tracked `cd` directory needs; the guard reuses them, so
//! there is one implementation, G-2).
//!
//! One work budget (`Work`, `limits::work_limit`) covers the whole analysis, every reading pass included: the
//! lexer charges what it visits, and this module the words it stores, the texts it analyzes and the lines it
//! builds. Over budget, the analysis stops and is `incomplete` (Python raises `_TooComplex`; the guard
//! blocks, "too complex to check safely").
//!
//! Beyond Python (each sets `incomplete` or is case-scoped in the difftest):
//! - `find -exec find -exec ...` nesting is walked with an explicit stack (Python recurses and raises
//!   `RecursionError` near 500 levels) and stops past `FIND_EXEC_CAP` levels (D-1);
//! - every `fd -x`/`--exec` group ended by `;` is a command, not only the first (D-8, FW-I review R1-M2);
//! - alternate readings are all kept in `Analysis.alternates` for G1 (lead ruling Q3=A), where Python checks
//!   G1 as it goes and keeps the first that hits.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use crate::analyze::Entry;
use crate::lexer::scan::scan_paren;
use crate::lexer::words::Memo;
use crate::lexer::{
    Group, GroupId, LexOut, Reading, Redirect, TooComplex, Work, find_from, lex_in, pystr,
    skip_escape,
};
use crate::limits::{DEFAULT_BACKTRACK_LIMIT, Limits, MAX_CWD_DEPTH, MAX_DEPTH};
use crate::opaque::{Opaque, TEST_BRACKETS, is_dynamic, mark};
use crate::prefixes::hidden::runs_option_command;
use crate::prefixes::{
    MAX_FED, STDIN_ARG_RUNNERS, StdinRun, Stripped, UNPARSED_RUNNERS, is_interpreter,
    parallel_lines, pm_runs_own_verb, strip_prefixes,
};
use crate::shell_args::{
    SHELLS, ShellMode, WORD, embedded_shells_in, prog_name, script_starts_in, shell_parse,
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
    let flag = |a: &String| {
        a.strip_prefix('-')
            .is_some_and(|r| !r.is_empty() && r.bytes().all(|b| matches!(b, b'n' | b'e' | b'E')))
    };
    let start = args.iter().position(|a| !flag(a)).unwrap_or(args.len());
    args[start..].join(" ").replace("\\n", "\n")
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

/// `_FD_PATH`: a numbered file descriptor.
fn is_fd_path(path: &str) -> bool {
    ["/dev/fd/", "/proc/self/fd/"].iter().any(|prefix| {
        path.strip_prefix(prefix)
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    })
}

/// The commands `find -exec`/`-execdir`/`-ok`/`-okdir` (each ended by `;` or `+`) or fd's `-x`/`--exec`/`-X`/
/// `--exec-batch` (ended by `;`) run. Rust stricter (FW-I review R1-M2): every fd group, not only the first.
fn exec_payloads(prog: &str, args: &[String]) -> Vec<Vec<String>> {
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
            if FD_EXEC.contains(&args[k].as_str()) {
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

    fn spend(&mut self, n: usize) -> bool {
        if self.halted || self.work.charge(n).is_err() {
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
        self.collect_once(text, exempt, depth);
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
        for seg in lexed.segments {
            let group = ids.get(&seg.group).copied().unwrap_or(GroupId(u32::MAX));
            match self.add_command(seg.tokens, seg.redirs, group, exempt, depth, &from_text) {
                Some(entry) => new_entries.push(entry),
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
            for &e in &entries {
                let entry = &self.entries[e];
                if entry.prog == "echo" || entry.prog == "printf" {
                    texts.push(echo_text(&entry.args));
                }
            }
            if !self.spend(texts.iter().map(|t| WORD + t.len()).sum()) {
                return;
            }
            for &e in &interpreters {
                // each is scanned per entry (G3)
                if !self.spend(texts.iter().map(|t| WORD + t.len()).sum()) {
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
                if texts.is_empty() && !info_only {
                    mark(
                        &mut self.entries[shell].opaque,
                        Opaque::StdinShellUnreadable,
                    );
                }
                let exempt = self.entries[shell].exempt;
                for body in &texts {
                    if self.collect(body, exempt, depth + 1) {
                        mark(&mut self.entries[shell].opaque, Opaque::DepthCap);
                    }
                }
            }
            if !feeders.is_empty() {
                let lines = fed_lines(&texts);
                for &e in &feeders {
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
        if text.contains("<(") && !consumers.is_empty() {
            let Some(inners) = self.process_substitutions(text) else {
                return;
            };
            for inner in inners {
                let Some(bodies) = self.literal_output(&inner, depth) else {
                    return;
                };
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
                for line in found {
                    if self.collect(&line, exempt, depth + 1) {
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
                for line in lines {
                    if !self.spend(base + found.saturating_mul(line.len())) {
                        return;
                    }
                    let tokens = run
                        .template
                        .iter()
                        .map(|t| t.replace(replace.as_str(), line))
                        .collect();
                    if self
                        .add_command(tokens, Vec::new(), group, exempt, depth + 1, &[])
                        .is_none()
                    {
                        return;
                    }
                }
            } else if !run.template.is_empty() {
                let mut tokens = run.template.clone();
                tokens.extend(
                    lines
                        .iter()
                        .flat_map(|line| pystr::split_ws(line))
                        .map(str::to_string),
                );
                if self
                    .add_command(tokens, Vec::new(), group, exempt, depth + 1, &[])
                    .is_none()
                {
                    return;
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
        for seg in &lexed.segments {
            let result = strip_prefixes(&seg.tokens, None, None, true, &mut self.work);
            let stripped = self.take(result)?;
            if let Some(first) = stripped.cmd.first()
                && matches!(prog_name(first).as_str(), "echo" | "printf")
            {
                let out = echo_text(&stripped.cmd[1..]);
                if !self.spend(WORD + out.len()) {
                    return None;
                }
                texts.push(out);
            }
            if let Some(group) = by_id.remove(&seg.group) {
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
        if SHELLS.contains(&prog) {
            let parsed = shell_parse(prog, &entry.args, 0);
            return Some(match parsed.mode {
                ShellMode::Stdin => true,
                ShellMode::File => entry
                    .args
                    .get(parsed.start)
                    .is_some_and(|f| is_stdin_path(f)),
                _ => false,
            });
        }
        if prog == "source" || prog == "." {
            return Some(first_arg(&entry.args).is_some_and(is_stdin_path));
        }
        if DOCKER.contains(&prog) {
            let args = entry.args.clone();
            let Some(shells) = embedded_shells_in(&args, &mut self.work.budget) else {
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
        let size: usize = tokens.iter().map(|t| WORD + t.len()).sum();
        if !self.spend(1 + 2 * size) {
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
            peeled,
        } = self.take(result)?;
        let prog_path = cmd.first().map(|w| pystr::lower(w)).unwrap_or_default();
        let prog = cmd.first().map(|w| prog_name(w)).unwrap_or_default();
        let args: Vec<String> = cmd.get(1..).map(<[String]>::to_vec).unwrap_or_default();
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
        let mut differs = false;
        for (at, alternate) in alternates {
            let result = strip_prefixes(&alternate, None, None, false, &mut self.work);
            let other = self.take(result)?;
            for script in other.scripts {
                if !scripts.contains(&script) {
                    scripts.push(script);
                }
            }
            if other.cmd == cmd {
                continue;
            }
            differs |= at < main_at;
            if !self.spend(1 + 2 * size + other.cmd.iter().map(|t| WORD + t.len()).sum::<usize>()) {
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
            || cmd
                .first()
                .is_some_and(|w| is_dynamic(w) && !TEST_BRACKETS.contains(&w.as_str()))
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
        for script in &scripts {
            if self.collect(script, exempt, depth + 1) {
                mark(&mut self.entries[e].opaque, Opaque::DepthCap);
            }
        }
        if DOCKER.contains(&prog.as_str()) {
            // `docker run img sh -euc "..."`
            let Some(starts) = script_starts_in(&args, &mut self.work.budget) else {
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

/// `_feed_stdin`'s lines: every non-blank line of `texts`, stripped, at most `MAX_FED`.
fn fed_lines(texts: &[String]) -> Vec<String> {
    let mut lines = Vec::new();
    for text in texts {
        for line in pystr::splitlines(text) {
            let line = pystr::strip_ws(line);
            if !line.is_empty() {
                lines.push(line.to_string());
                if lines.len() >= MAX_FED {
                    return lines;
                }
            }
        }
    }
    lines
}
