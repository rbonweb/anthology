//! Command prefixes and wrappers (SH2): guard_rules.py `_strip_prefixes`, `_skip_flags`, `_runner_program`,
//! `_python_runner_start`, `_strip_version` and their tables.

use crate::rtk_forms::rtk_command;
use crate::shell_args::{SHELLS, prog_name, shell_parse};

/// Shell grammar words peeled off a command (`_KEYWORDS`).
const KEYWORDS: &[&str] = &[
    "if", "then", "else", "elif", "do", "while", "until", "{", "coproc",
];

/// Wrapper program → flags that consume the following word (`_WRAPPERS`).
const WRAPPERS: &[(&str, &[&str])] = &[
    (
        "sudo",
        &[
            "-u", "-g", "-C", "-h", "-p", "-r", "-t", "-D", "-R", "-T", "-U", "--user", "--group",
            "--host", "--prompt", "--chdir",
        ],
    ),
    ("doas", &["-u", "-C"]),
    ("env", &["-u", "-C", "--unset", "--chdir"]),
    ("exec", &["-a"]),
    ("time", &["-f", "-o"]),
    ("nohup", &[]),
    ("nice", &["-n", "--adjustment"]),
    ("timeout", &["-s", "-k", "--signal", "--kill-after"]),
    (
        "xargs",
        &[
            "-I",
            "-n",
            "-P",
            "-L",
            "-s",
            "-d",
            "-E",
            "-a",
            "--max-args",
            "--max-lines",
            "--max-procs",
            "--replace",
            "--delimiter",
        ],
    ),
    (
        "xvfb-run",
        &[
            "-s",
            "-n",
            "-e",
            "-f",
            "-p",
            "--server-args",
            "--server-num",
            "--error-file",
            "--auth-file",
        ],
    ),
    ("caffeinate", &["-t", "-w"]),
    ("stdbuf", &[]),
    ("watch", &["-n", "--interval"]),
    ("arch", &[]),
    ("gtimeout", &["-s", "-k", "--signal", "--kill-after"]),
    ("setsid", &[]),
    ("ionice", &["-c", "-n", "-p", "-P", "-u"]),
    ("unbuffer", &[]),
];
const TIMEOUTS: &[&str] = &["timeout", "gtimeout"];
/// Flags of npm/pnpm/yarn/bun that take a value and may precede the subcommand (`_PM_VALUE_FLAGS`).
const PM_VALUE_FLAGS: &[&str] = &[
    "--filter",
    "-F",
    "-C",
    "--dir",
    "--prefix",
    "-w",
    "--workspace",
    "--cwd",
    "--registry",
];
const JS_PACKAGE_MANAGERS: &[&str] = &["npm", "pnpm", "yarn", "bun"];
const RUNNERS: &[&str] = &["npx", "bunx", "pnpx", "uvx"];
const RUNNER_VALUE_FLAGS: &[&str] = &[
    "-p",
    "--package",
    "-w",
    "--workspace",
    "--from",
    "--with",
    "--python",
    "-n",
    "--name",
    "--prefix",
    "--project",
    "--directory",
    "--env-file",
    "--extra",
    "--group",
    "-P",
];
const RUNNER_SUBCOMMANDS: &[&str] = &["exec", "dlx", "x"];
const PYTHON_GLOBAL_VALUE_FLAGS: &[&str] = &[
    "-C",
    "--directory",
    "--project",
    "-e",
    "--env",
    "-p",
    "--python",
    "--config-file",
    "--cache-dir",
    "--env-file",
    "-n",
    "--name",
    "--prefix",
];
/// Python project runners: `<name> [<global flags>] run ...` (`_PYTHON_RUNNERS`).
const PYTHON_RUNNERS: &[(&str, &[&[&str]])] = &[
    ("uv", &[&["run"], &["tool", "run"]]),
    ("poetry", &[&["run"]]),
    ("pipenv", &[&["run"]]),
    ("pdm", &[&["run"]]),
    ("hatch", &[&["run"]]),
    ("rye", &[&["run"]]),
    ("conda", &[&["run"]]),
];
const PM_VERBS: &[&str] = &[
    "add",
    "install",
    "i",
    "remove",
    "rm",
    "uninstall",
    "un",
    "r",
    "run",
    "exec",
    "dlx",
    "x",
    "init",
    "publish",
    "link",
    "unlink",
    "update",
    "upgrade",
    "up",
    "test",
    "start",
    "build",
    "ci",
    "cache",
    "config",
    "info",
    "list",
    "ls",
    "outdated",
    "audit",
    "why",
    "pack",
    "version",
    "workspace",
    "workspaces",
    "global",
    "create",
    "login",
    "logout",
    "whoami",
    "help",
    "prune",
    "store",
    "rebuild",
    "dedupe",
    "import",
    "patch",
    "set",
    "get",
    "plugin",
    "node",
    "bin",
    "dev",
];

fn has(set: &[&str], word: &str) -> bool {
    set.contains(&word)
}

/// What `strip_prefixes` peeled that the analysis could not see through (for `Opaque`).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Peeled {
    /// A `""` word (a `$(...)` placeholder) was dropped where the program word was expected.
    pub(crate) placeholder: bool,
    /// A wrapper's option was not one of the flags known to take a value: it may have taken one.
    pub(crate) unknown_option: bool,
}

/// `_ASSIGNMENT.match(word)`: `[A-Za-z_][A-Za-z0-9_]*=` at the start.
fn is_assignment(word: &str) -> bool {
    let bytes = word.as_bytes();
    match bytes.first() {
        Some(b) if b.is_ascii_alphabetic() || *b == b'_' => {}
        _ => return false,
    }
    for &b in &bytes[1..] {
        if b == b'=' {
            return true;
        }
        if !(b.is_ascii_alphanumeric() || b == b'_') {
            return false;
        }
    }
    false
}

/// `_PYTHON.match(name)`: `python[0-9.]*\Z`.
fn is_python(name: &str) -> bool {
    name.strip_prefix("python")
        .is_some_and(|rest| rest.bytes().all(|b| b.is_ascii_digit() || b == b'.'))
}

/// `_strip_version`: `name@1.2` → `name`, but `@scope/pkg` keeps its leading `@`.
fn strip_version(word: &str) -> String {
    match word.rfind('@') {
        Some(at) if at > 0 => word[..at].to_string(),
        _ => word.to_string(),
    }
}

/// `_skip_flags(tokens, i, value_flags)`: the index past the options from `tokens[i]` (it may pass the end,
/// as Python's does). `token[-1].isalpha()` is implied by `"-" + token[-1] in value_flags`: every value flag
/// is `-` and an ASCII letter (lead ruling R1). `unknown` is set for an option not in `value_flags`.
pub(crate) fn skip_flags(
    tokens: &[String],
    mut i: usize,
    value_flags: &[&str],
    unknown: &mut bool,
) -> usize {
    let n = tokens.len();
    while i < n {
        let token = tokens[i].as_str();
        if token == "--" {
            return i + 1;
        }
        let mut chars = token.chars();
        if chars.next() == Some('-') && chars.next().is_some() {
            let second = token.chars().nth(1);
            let last = token.chars().last();
            let takes_value = has(value_flags, token)
                || (second != Some('-')
                    && token.chars().count() > 2
                    && last.is_some_and(|c| {
                        let mut flag = String::from("-");
                        flag.push(c);
                        has(value_flags, &flag)
                    }));
            if !takes_value && !value_flags.is_empty() {
                *unknown = true;
            }
            i += if takes_value { 2 } else { 1 };
        } else {
            break;
        }
    }
    i
}

fn tail(tokens: &[String], i: usize) -> Vec<String> {
    tokens.get(i..).map(<[String]>::to_vec).unwrap_or_default()
}

/// `_runner_program`: `tokens[i..]` follow a package runner; skip its options and return the rest, starting at
/// the package program (version stripped). `-c <script>` / `--call` is recursed instead.
fn runner_program(tokens: &[String], mut i: usize, scripts: &mut Vec<String>) -> Vec<String> {
    let n = tokens.len();
    while i < n {
        let token = tokens[i].as_str();
        if token == "--" {
            i += 1;
            break;
        }
        if token == "-c" || token == "--call" {
            if i + 1 < n {
                scripts.push(tokens[i + 1].clone());
            }
            return Vec::new();
        }
        if let Some(script) = token.strip_prefix("--call=") {
            scripts.push(script.to_string());
            return Vec::new();
        }
        if has(RUNNER_VALUE_FLAGS, token) {
            i += 2;
        } else if token.starts_with('-') && token.chars().count() > 1 {
            i += 1;
        } else {
            break;
        }
    }
    let mut rest = tail(tokens, i);
    if let Some(first) = rest.first_mut() {
        *first = strip_version(first);
    }
    rest
}

/// `_python_runner_start`: the index of the first word after `uv run` / `uv tool run` / `poetry run` ..., if any.
fn python_runner_start(subcommands: &[&[&str]], tokens: &[String]) -> Option<usize> {
    let mut unknown = false;
    let start = skip_flags(tokens, 1, PYTHON_GLOBAL_VALUE_FLAGS, &mut unknown);
    subcommands.iter().find_map(|words| {
        let got = tokens.get(start..start + words.len())?;
        got.iter()
            .zip(words.iter())
            .all(|(a, b)| a == b)
            .then_some(start + words.len())
    })
}

/// `_strip_prefixes`: peel assignments, keywords, wrappers and package runners off a command. Returns the
/// command starting at its program, the embedded scripts to analyze, and what was peeled blind.
pub(crate) fn strip_prefixes(tokens: &[String]) -> (Vec<String>, Vec<String>, Peeled) {
    let mut toks: Vec<String> = tokens.to_vec();
    let mut scripts = Vec::new();
    let mut peeled = Peeled::default();
    for _ in 0..64 {
        if toks.is_empty() {
            break;
        }
        let word = toks[0].clone();
        let name = prog_name(&word);
        if is_assignment(&word) || word == "!" || has(KEYWORDS, &word) || word.is_empty() {
            // `""` is a `$(...)` placeholder
            peeled.placeholder |= word.is_empty();
            toks.remove(0);
        } else if let Some(&(_, value_flags)) = WRAPPERS.iter().find(|(w, _)| *w == name) {
            if name == "env" {
                let split = toks.iter().position(|t| t == "-S" || t == "--split-string");
                if let Some(split) = split
                    && split + 1 < toks.len()
                {
                    scripts.push(toks[split + 1].clone());
                    toks.clear();
                    break;
                }
            }
            let mut unknown = false;
            let mut i = skip_flags(&toks, 1, value_flags, &mut unknown);
            peeled.unknown_option |= unknown || (value_flags.is_empty() && has_option(&toks, i));
            if has(TIMEOUTS, &name) && i < toks.len() {
                i += 1; // the duration
            }
            if name == "watch" && i < toks.len() && toks[i].contains(' ') {
                scripts.push(toks[i].clone()); // watch runs its argument via sh -c
            }
            toks = tail(&toks, i);
        } else if name == "command" {
            if toks.len() > 1 && (toks[1] == "-v" || toks[1] == "-V") {
                let mut out = vec!["which".to_string()];
                out.extend(tail(&toks, 2));
                return (out, scripts, peeled); // lookup, not execution
            }
            let mut unknown = false;
            let i = skip_flags(&toks, 1, &[], &mut unknown);
            toks = tail(&toks, i);
        } else if name == "rtk" {
            toks = rtk_command(&toks);
        } else if has(RUNNERS, &name) {
            toks = runner_program(&toks, 1, &mut scripts);
        } else if let Some(start) = PYTHON_RUNNERS
            .iter()
            .find(|(runner, _)| *runner == name)
            .and_then(|(_, subs)| python_runner_start(subs, &toks))
        {
            toks = runner_program(&toks, start, &mut scripts);
        } else if has(JS_PACKAGE_MANAGERS, &name) && toks.len() > 1 {
            let mut unknown = false;
            let i = skip_flags(&toks, 1, PM_VALUE_FLAGS, &mut unknown);
            let verb = toks.get(i).map_or("", String::as_str).to_string();
            if has(RUNNER_SUBCOMMANDS, &verb) || (verb == "run" && name != "npm") {
                toks = runner_program(&toks, i + 1, &mut scripts); // `yarn run playwright ...`
            } else if verb == "workspace" && name == "yarn" && toks.len() > i + 2 {
                let mut out = vec!["yarn".to_string()];
                out.extend(tail(&toks, i + 2)); // `yarn workspace <w> <rest>`
                toks = out;
            } else if (name == "yarn" || name == "pnpm")
                && !verb.is_empty()
                && !has(PM_VERBS, &verb)
            {
                toks = runner_program(&toks, i, &mut scripts); // `yarn playwright test` runs the bin
            } else {
                break;
            }
        } else if toks.len() > 2 && name == "pipx" && toks[1] == "run" {
            toks = runner_program(&toks, 2, &mut scripts);
        } else if is_python(&name) {
            let module = toks
                .iter()
                .enumerate()
                .position(|(k, t)| t == "-m" && k + 1 < toks.len());
            let first_positional = toks
                .iter()
                .enumerate()
                .skip(1)
                .find(|(_, t)| !t.starts_with('-'))
                .map_or(toks.len(), |(k, _)| k);
            match module {
                Some(module) if module <= first_positional => toks = tail(&toks, module + 1),
                _ => break, // `python -m pip ...` runs `pip`; anything else stays python
            }
        } else {
            break;
        }
    }
    if let Some(first) = toks.first() {
        let name = prog_name(first);
        if has(SHELLS, &name) {
            let rest = tail(&toks, 1);
            scripts.extend(
                shell_parse(&name, &rest, 0)
                    .scripts
                    .into_iter()
                    .map(|(_, s)| s),
            );
        } else if name == "eval" {
            scripts.push(tail(&toks, 1).join(" "));
        }
    }
    (toks, scripts, peeled)
}

/// Whether a flag-less wrapper (`nohup`, `stdbuf`, ...) skipped an option word before index `i`.
fn has_option(toks: &[String], i: usize) -> bool {
    toks.iter()
        .take(i.min(toks.len()))
        .skip(1)
        .any(|t| t.starts_with('-') && t.chars().count() > 1 && t != "--")
}
