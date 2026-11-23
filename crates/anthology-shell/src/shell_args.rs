//! Shell option parsing (guard_rules.py `_parse_shell_args`, the fish parser, `_shell_parse`,
//! `_embedded_shells`, `_script_starts`) and `_prog_name`. In SH1 (lead ruling R2), because the lenient lexer
//! needs `_script_starts`.

use std::collections::HashSet;

use crate::lexer::pystr;

/// The shells whose `-c` scripts and stdin are analyzed (`_SHELLS`).
pub const SHELLS: &[&str] = &[
    "bash", "sh", "zsh", "dash", "ksh", "fish", "ash", "mksh", "hush", "yash", "csh", "tcsh",
    "rbash", "posh",
];

/// Long shell options that take the next word as their value (`_SHELL_VALUE_OPTIONS`).
const SHELL_VALUE_OPTIONS: &[&str] = &["--rcfile", "--init-file", "--emulate"];

// fish options (`_FISH_LONG_VALUE`, `_FISH_LONG_FLAG`, `_FISH_SHORT_VALUE`, `_FISH_SCRIPT_LONG`), in
// guard_rules.py's order: a prefix names the first value option it fits.
const FISH_LONG_VALUE: &[&str] = &[
    "--command",
    "--init-command",
    "--debug",
    "--debug-output",
    "--debug-stack-frames",
    "--features",
    "--profile",
    "--profile-startup",
];
const FISH_LONG_FLAG: &[&str] = &[
    "--interactive",
    "--login",
    "--no-config",
    "--no-execute",
    "--private",
    "--print-rusage-self",
    "--print-debug-categories",
    "--help",
    "--version",
];
const FISH_SHORT_VALUE: &[char] = &['c', 'C', 'd', 'D', 'o', 'f', 'p'];
const FISH_SCRIPT_LONG: &[&str] = &["--command", "--init-command"];

/// How a shell gets its script: `'c'`, `'stdin'` or `'file'` in guard_rules.py.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ShellMode {
    /// `-c`: the word at `start` is the script.
    Command,
    /// The script comes from stdin (`-s`, no words left, `-c` without a script, or past an unknown fish option).
    Stdin,
    /// The word at `start` is a script file.
    File,
}

/// `_shell_parse`'s `(mode, start, scripts)`: `scripts` is each `(index of the word holding it, command text)`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShellParse {
    pub mode: ShellMode,
    pub start: usize,
    pub scripts: Vec<(usize, String)>,
}

/// Lowercased program name: basename of a path, but scoped packages keep `@scope/name` (`_prog_name`).
pub fn prog_name(word: &str) -> String {
    let word = pystr::lower(word);
    if word.starts_with('@') {
        return word;
    }
    let trimmed = word.trim_end_matches('/');
    let base = match trimmed.rfind('/') {
        Some(at) => &trimmed[at + 1..],
        None => trimmed,
    };
    if base.is_empty() {
        word
    } else {
        base.to_string()
    }
}

fn char_count(s: &str) -> usize {
    s.chars().count()
}

/// `_parse_shell_args`: a bash/sh/zsh/dash/ksh argument list from `args[i]` → (mode, first word after the options).
pub fn parse_shell_args(args: &[String], mut i: usize) -> (ShellMode, usize) {
    let n = args.len();
    let (mut command, mut stdin) = (false, false);
    while i < n {
        let token = args[i].as_str();
        if token == "--" || token == "-" {
            i += 1;
            break;
        }
        if token.starts_with("--") {
            i += if SHELL_VALUE_OPTIONS.contains(&token) {
                2
            } else {
                1
            };
        } else if char_count(token) > 1 && (token.starts_with('-') || token.starts_with('+')) {
            let letters = &token[1..];
            command = command || letters.contains('c');
            stdin = stdin || letters.contains('s');
            i += 1 + letters.matches('o').count() + letters.matches('O').count();
        } else {
            break;
        }
    }
    i = i.min(n);
    if command {
        return if i < n {
            (ShellMode::Command, i)
        } else {
            (ShellMode::Stdin, i)
        };
    }
    if stdin || i >= n {
        (ShellMode::Stdin, i)
    } else {
        (ShellMode::File, i)
    }
}

/// `_fish_long_option`: (option, attached value) for a `--...` token; a prefix of a value option names it.
fn fish_long_option(token: &str) -> (String, Option<&str>) {
    let (name, value) = match token.find('=') {
        Some(at) => (&token[..at], Some(&token[at + 1..])),
        None => (token, None),
    };
    let fits = FISH_LONG_VALUE
        .iter()
        .find(|opt| char_count(name) > 2 && opt.starts_with(name));
    (fits.copied().unwrap_or(name).to_string(), value)
}

/// `_fish_option_at`: (scripts, runs_c, takes_next) for the option word `args[i]`.
fn fish_option_at(args: &[String], i: usize) -> (Vec<(usize, String)>, bool, bool) {
    let token = args[i].as_str();
    let n = args.len();
    let mut scripts = Vec::new();
    let (mut runs_c, mut takes_next) = (false, false);
    if token.starts_with("--") && char_count(token) > 2 {
        let (name, value) = fish_long_option(token);
        takes_next = FISH_LONG_VALUE.contains(&name.as_str()) && value.is_none();
        if FISH_SCRIPT_LONG.contains(&name.as_str()) {
            runs_c = name == "--command";
            if let Some(value) = value {
                scripts.push((i, value.to_string()));
            } else if i + 1 < n {
                scripts.push((i + 1, args[i + 1].clone()));
            }
        }
    } else if token.starts_with('-') && char_count(token) > 1 && token != "--" {
        for (at, letter) in token.char_indices().skip(1) {
            if !FISH_SHORT_VALUE.contains(&letter) {
                continue;
            }
            let attached = &token[at + letter.len_utf8()..];
            if letter == 'c' || letter == 'C' {
                runs_c = letter == 'c';
                if !attached.is_empty() {
                    scripts.push((i, attached.to_string()));
                } else if i + 1 < n {
                    scripts.push((i + 1, args[i + 1].clone()));
                }
            }
            takes_next = attached.is_empty();
            break;
        }
    }
    (scripts, runs_c, takes_next)
}

/// A work allowance for the option parsers. guard_rules.py's `_script_starts` is quadratic in its output
/// (each `fish` word rescans every later word) and the lenient lexer then copies a segment per start, so a
/// few kilobytes of `fish -cX` can ask for gigabytes; a failed allocation aborts the guard, and an abort does
/// not block. Past the allowance the caller stops and sets `incomplete` (fail closed, D-7: Rust may block
/// where Python would merely be slow).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Budget(pub(crate) usize);

/// The charge for one copied word on top of its bytes: about what a `String` costs besides its text.
pub(crate) const WORD: usize = 24;

impl Budget {
    /// Spend `n` units; false (and nothing left) when there were not enough.
    pub(crate) fn spend(&mut self, n: usize) -> bool {
        match self.0.checked_sub(n) {
            Some(left) => {
                self.0 = left;
                true
            }
            None => {
                self.0 = 0;
                false
            }
        }
    }
}

/// `_parse_fish_args` within a budget: `None` when the budget ran out.
pub(crate) fn parse_fish_args_in(
    args: &[String],
    mut i: usize,
    budget: &mut Budget,
) -> Option<ShellParse> {
    let n = args.len();
    let first = i;
    let (mut command, mut unknown) = (false, false);
    while i < n {
        let token = args[i].as_str();
        if token == "--" || token == "-" {
            i += 1;
            break;
        }
        if !(token.starts_with('-') && char_count(token) > 1) {
            break;
        }
        let (_scripts, runs_c, takes_next) = fish_option_at(args, i);
        command = command || runs_c;
        if token.starts_with("--") && char_count(token) > 2 {
            let name = token.split('=').next().unwrap_or(token);
            unknown = unknown
                || !FISH_LONG_VALUE
                    .iter()
                    .chain(FISH_LONG_FLAG)
                    .any(|opt| char_count(name) > 2 && opt.starts_with(name));
        }
        i += if takes_next { 2 } else { 1 };
    }
    i = i.min(n);
    // Every later word is read as an option too (fail closed); `found not in scripts` keeps first-seen order.
    // Costs: 1 + length per word read, and 1 + twice its length per script kept.
    let mut scripts: Vec<(usize, String)> = Vec::new();
    let mut seen: HashSet<(usize, String)> = HashSet::new();
    for k in first..n {
        if !budget.spend(WORD + args[k].len()) {
            return None;
        }
        for found in fish_option_at(args, k).0 {
            if !budget.spend(WORD + 2 * found.1.len()) {
                return None;
            }
            if seen.insert(found.clone()) {
                scripts.push(found);
            }
        }
    }
    let mode = if command {
        ShellMode::Command
    } else if i < n && !unknown {
        ShellMode::File
    } else {
        ShellMode::Stdin
    };
    Some(ShellParse {
        mode,
        start: i,
        scripts,
    })
}

fn parse_or_stdin(parsed: Option<ShellParse>, args: &[String], i: usize) -> ShellParse {
    parsed.unwrap_or_else(|| ShellParse {
        mode: ShellMode::Stdin,
        start: i.min(args.len()),
        scripts: Vec::new(),
    })
}

/// `_parse_fish_args`: fish's options from `args[i]`, failing closed past options it may not know.
pub fn parse_fish_args(args: &[String], i: usize) -> ShellParse {
    parse_or_stdin(
        parse_fish_args_in(args, i, &mut Budget(usize::MAX)),
        args,
        i,
    )
}

/// `_shell_parse` within a budget: `None` when the budget ran out.
pub(crate) fn shell_parse_in(
    prog: &str,
    args: &[String],
    i: usize,
    budget: &mut Budget,
) -> Option<ShellParse> {
    if prog == "fish" {
        return parse_fish_args_in(args, i, budget);
    }
    let (mode, start) = parse_shell_args(args, i);
    let scripts = match args.get(start) {
        Some(script) if mode == ShellMode::Command => vec![(start, script.clone())],
        _ => Vec::new(),
    };
    Some(ShellParse {
        mode,
        start,
        scripts,
    })
}

/// `_shell_parse`: (mode, start, scripts) for shell `prog`'s arguments from `args[i]`.
pub fn shell_parse(prog: &str, args: &[String], i: usize) -> ShellParse {
    parse_or_stdin(
        shell_parse_in(prog, args, i, &mut Budget(usize::MAX)),
        args,
        i,
    )
}

/// `_embedded_shells` within a budget: `None` when the budget ran out.
pub(crate) fn embedded_shells_in(
    words: &[String],
    budget: &mut Budget,
) -> Option<Vec<(ShellMode, usize)>> {
    let mut out = Vec::new();
    let (mut k, n) = (0, words.len());
    while k < n {
        let prog = prog_name(&words[k]);
        if SHELLS.contains(&prog.as_str()) {
            let parsed = shell_parse_in(&prog, words, k + 1, budget)?;
            out.push((parsed.mode, parsed.start));
            k = parsed.start.max(k + 1);
        } else {
            k += 1;
        }
    }
    Some(out)
}

/// `_embedded_shells`: (mode, start) for each shell program among `words`; its own options are not rescanned.
pub fn embedded_shells(words: &[String]) -> Vec<(ShellMode, usize)> {
    embedded_shells_in(words, &mut Budget(usize::MAX)).unwrap_or_default()
}

/// `_SHELL_C_FLAG.match(word)`: `-[A-Za-z]*c[A-Za-z]*\Z`.
fn is_shell_c_flag(word: &str) -> bool {
    match word.strip_prefix('-') {
        Some(rest) => rest.bytes().all(|b| b.is_ascii_alphabetic()) && rest.contains('c'),
        None => false,
    }
}

/// Programs whose `-c` option takes data, not a shell script (`psql -c "select count(*) from t"`). An
/// allowlist: only a name listed here is read as data, every other program keeps the fail-closed reading.
const DATA_C_PROGRAMS: &[&str] = &["psql"];

/// Programs that run a `-c` word through a shell, or run another program that may: a data program listed
/// before one of them no longer owns a later `-c` (`psql su -c ...` stays a shell reading).
const SHELL_RUNNERS: &[&str] = &[
    "su", "runuser", "sudo", "doas", "script", "sg", "newgrp", "flock", "env", "xargs", "time",
    "nice", "nohup", "timeout", "watch", "setsid", "exec", "command", "busybox", "docker", "ssh",
];

/// `_script_starts` within a budget (each start found costs one unit too): `None` when it ran out.
pub(crate) fn script_starts_in(
    words: &[String],
    budget: &mut Budget,
) -> Option<Vec<(usize, String)>> {
    script_starts_with(words, budget, false)
}

/// `_script_starts` for the words of a command run through docker, where `psql -c` takes SQL: the same
/// starts, less a `-c` flag word that a data program (`DATA_C_PROGRAMS`) owns. The script of a shell word
/// (`sh -c`) is never dropped.
pub(crate) fn docker_script_starts_in(
    words: &[String],
    budget: &mut Budget,
) -> Option<Vec<(usize, String)>> {
    script_starts_with(words, budget, true)
}

fn script_starts_with(
    words: &[String],
    budget: &mut Budget,
    data_programs: bool,
) -> Option<Vec<(usize, String)>> {
    let mut starts = Vec::new();
    // the last program word read is a data program: its `-c` words are its own options (one pass, linear)
    let mut data_owner = false;
    let (mut k, n) = (0, words.len());
    while k + 1 < n {
        let word = &words[k];
        let prog = prog_name(word);
        if SHELLS.contains(&prog.as_str()) {
            data_owner = false;
            let parsed = shell_parse_in(&prog, words, k + 1, budget)?;
            let cost = parsed.scripts.iter().map(|(_, s)| WORD + s.len()).sum();
            if !budget.spend(cost) {
                return None;
            }
            starts.extend(parsed.scripts);
            k = parsed.start.max(k + 1);
            continue;
        }
        if DATA_C_PROGRAMS.contains(&prog.as_str()) {
            data_owner = data_programs;
        } else if SHELL_RUNNERS.contains(&prog.as_str()) {
            data_owner = false;
        }
        if is_shell_c_flag(word) && !data_owner {
            if !budget.spend(WORD + words[k + 1].len()) {
                return None;
            }
            starts.push((k + 1, words[k + 1].clone()));
        }
        k += 1;
    }
    Some(starts)
}

/// `_script_starts`: (index of the word an embedded script starts in, the script's first word) for each one.
/// Quadratic in the number of `fish` words, as in Python; the lexer and `analyze` use a bounded form.
pub fn script_starts(words: &[String]) -> Vec<(usize, String)> {
    script_starts_in(words, &mut Budget(usize::MAX)).unwrap_or_default()
}
