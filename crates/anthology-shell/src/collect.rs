//! Entry collection (SH2): guard_rules.py `_Context`, `_add_command`, `_collect_find_exec`,
//! `_reads_stdin_as_shell`, `_echo_text`, `_first_arg`, `_collect`, and the path helpers `_expand` and
//! `_normalize` (which the tracked `cd` directory needs; the guard reuses them, so there is one
//! implementation, G-2).
//!
//! Bounds Python does not have (each sets `incomplete`, so the guard blocks; Rust may block where Python
//! raised or would merely be slow, D-1/D-7):
//! - `find -exec find -exec ...` nesting is walked with an explicit stack (Python recurses and raises
//!   `RecursionError` near 500 levels) and stops past `FIND_EXEC_CAP` levels;
//! - a work budget (`ANALYZE_BUDGET`: words stored, texts lexed, option rescans) stops runaway expansion,
//!   such as `echo X | echo X | sh` nested to `MAX_DEPTH`, which is exponential in Python too.

use std::collections::HashMap;
use std::sync::LazyLock;

use crate::analyze::Entry;
use crate::lexer::{Group, GroupId, Redirect, lex_in, pystr};
use crate::limits::{DEFAULT_BACKTRACK_LIMIT, Limits, MAX_CWD_DEPTH, MAX_DEPTH};
use crate::opaque::{Opaque, TEST_BRACKETS, is_dynamic, mark};
use crate::prefixes::hidden::runs_option_command;
use crate::prefixes::{STDIN_ARG_RUNNERS, UNPARSED_RUNNERS, pm_runs_own_verb, strip_prefixes};
use crate::shell_args::{
    Budget, SHELLS, ShellMode, WORD, embedded_shells_in, prog_name, script_starts_in, shell_parse,
};

const DOCKER: &[&str] = &["docker", "docker-compose"];
const CHDIR_PROGRAMS: &[&str] = &["tar", "git", "make"];
const FIND_EXEC: &[&str] = &["-exec", "-execdir", "-ok", "-okdir"];

/// `find -exec find -exec ...` levels walked before giving up (Python raises `RecursionError` well below).
pub(crate) const FIND_EXEC_CAP: usize = 1000;

/// Work units one `analyze` may spend: a word stored in an entry costs `WORD` plus its bytes, twice (`raw` and
/// `args`) and once more in an interpreter's `code`; a text lexed costs 1 plus its bytes (and the lexer charges
/// the words it copies); an option rescan charges the words it reads and the scripts it copies. It bounds
/// memory and time; past it the analysis stops and is `incomplete` (D-7).
pub(crate) const ANALYZE_BUDGET: usize = 64_000_000;

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

/// `_INTERPRETER.match(prog)`: programs whose arguments (and fed stdin text) are inline code.
pub(crate) fn is_interpreter(prog: &str) -> bool {
    const VERSIONED: &[&str] = &["python", "pypy", "ruby", "perl", "php", "lua"];
    const EXACT: &[&str] = &[
        "node",
        "nodejs",
        "deno",
        "bun",
        "tsx",
        "ts-node",
        "osascript",
        "rscript",
    ];
    VERSIONED.iter().any(|name| {
        prog.strip_prefix(name)
            .is_some_and(|rest| rest.bytes().all(|b| b.is_ascii_digit() || b == b'.'))
    }) || EXACT.contains(&prog)
}

/// `_echo_text`: an `echo`/`printf`'s words as the text it writes (`-n`/`-e`/`-E` dropped, `\n` a newline).
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

fn exec_payloads(args: &[String]) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    let mut k = 0;
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
    out
}

/// One frame of the `find -exec` walk: its payloads, the next one, and what they inherit.
struct FindFrame {
    payloads: Vec<Vec<String>>,
    next: usize,
    group: GroupId,
    exempt: bool,
    depth: usize,
}

/// `_Context` plus the analysis-wide state: every group, the flags and the budget.
pub(crate) struct Ctx<'a> {
    home: &'a str,
    limits: &'a Limits,
    re: &'a PathRegexes,
    cwd: Option<String>,
    pub(crate) entries: Vec<Entry>,
    pub(crate) groups: Vec<Group>,
    pub(crate) incomplete: bool,
    budget: Budget,
    halted: bool,
}

impl<'a> Ctx<'a> {
    pub(crate) fn new(home: &'a str, limits: &'a Limits, re: &'a PathRegexes) -> Ctx<'a> {
        Ctx {
            home,
            limits,
            re,
            cwd: None,
            entries: Vec::new(),
            groups: Vec::new(),
            incomplete: false,
            budget: Budget(ANALYZE_BUDGET),
            halted: false,
        }
    }

    fn halt(&mut self) {
        self.halted = true;
        self.incomplete = true;
    }

    fn spend(&mut self, n: usize) -> bool {
        if self.halted || !self.budget.spend(n) {
            self.halt();
            return false;
        }
        true
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

    /// `_collect(text, ctx, exempt, depth)`. Returns true when `text` was skipped at `MAX_DEPTH` (the caller
    /// marks the entry it belongs to `DepthCap`).
    pub(crate) fn collect(&mut self, text: &str, exempt: bool, depth: usize) -> bool {
        if text.is_empty() || self.halted {
            return false;
        }
        if depth > MAX_DEPTH {
            return true;
        }
        if !self.spend(1 + text.len()) {
            return false;
        }
        let (lexed, info) = lex_in(text, 0, self.limits, &mut self.budget);
        let mut from_text = Vec::new();
        if lexed.lenient || info.nested_lenient {
            from_text.push(Opaque::Unbalanced);
        }
        if info.depth_capped {
            from_text.push(Opaque::DepthCap);
        }
        if lexed.incomplete {
            self.incomplete = true; // the regex limit or the work budget
        }
        if info.regex_limited {
            from_text.push(Opaque::RegexLimit);
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
                None => return false,
            }
        }
        // Literal text that reaches a shell's stdin is a command (heredocs, here-strings, `echo | sh`), and
        // text that reaches an interpreter's stdin is code. Groups in first-appearance order.
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
                    None => return false,
                }
            }
            let interpreters: Vec<usize> = entries
                .iter()
                .copied()
                .filter(|&e| is_interpreter(&self.entries[e].prog))
                .collect();
            if shell.is_none() && interpreters.is_empty() {
                continue;
            }
            let mut texts = self
                .groups
                .get(group.0 as usize)
                .map(|g| g.texts.clone())
                .unwrap_or_default();
            for &e in &entries {
                let entry = &self.entries[e];
                if entry.prog == "echo" || entry.prog == "printf" {
                    texts.push(echo_text(&entry.args));
                }
            }
            for &e in &interpreters {
                if !self.spend(texts.iter().map(|t| WORD + t.len()).sum()) {
                    return false;
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
        }
        false
    }

    /// `_reads_stdin_as_shell`: a shell fed from stdin (`bash`, `sh -s`) or a docker command that runs one.
    /// `None` when the work budget ran out.
    fn reads_stdin_as_shell(&mut self, e: usize) -> Option<bool> {
        let entry = &self.entries[e];
        if SHELLS.contains(&entry.prog.as_str()) {
            return Some(shell_parse(&entry.prog, &entry.args, 0).mode == ShellMode::Stdin);
        }
        if DOCKER.contains(&entry.prog.as_str()) {
            let args = entry.args.clone();
            let Some(shells) = embedded_shells_in(&args, &mut self.budget) else {
                self.halt();
                return None;
            };
            return Some(shells.iter().any(|(mode, _)| *mode == ShellMode::Stdin));
        }
        Some(false)
    }

    /// `_add_command`, with the `find -exec` recursion walked iteratively. Returns the entry's index, or
    /// `None` once the analysis has stopped.
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
        if self.entries[e].prog == "find" {
            self.find_exec(e, depth);
        }
        Some(e)
    }

    /// `_collect_find_exec`, depth-first in Python's order, without recursion.
    fn find_exec(&mut self, e: usize, depth: usize) {
        let entry = &self.entries[e];
        let mut stack = vec![FindFrame {
            payloads: exec_payloads(&entry.args),
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
            if child.prog == "find" {
                let frame = FindFrame {
                    payloads: exec_payloads(&child.args),
                    next: 0,
                    group: child.group,
                    exempt: child.exempt,
                    depth,
                };
                stack.push(frame);
            }
        }
    }

    /// `_add_command` for one command, except the `find -exec` payloads.
    fn add_one(
        &mut self,
        tokens: Vec<String>,
        redirs: Vec<Redirect>,
        group: GroupId,
        exempt: bool,
        depth: usize,
        from_text: &[Opaque],
    ) -> Option<usize> {
        if !self.spend(1 + 2 * tokens.iter().map(|t| WORD + t.len()).sum::<usize>()) {
            return None;
        }
        let (cmd, scripts, peeled) = strip_prefixes(&tokens);
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
        for script in &scripts {
            if self.collect(script, exempt, depth + 1) {
                mark(&mut self.entries[e].opaque, Opaque::DepthCap);
            }
        }
        if DOCKER.contains(&prog.as_str()) {
            // `docker run img sh -euc "..."`
            let Some(starts) = script_starts_in(&args, &mut self.budget) else {
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
