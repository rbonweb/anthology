//! Command prefixes and wrappers (SH2): guard_rules.py `_strip_prefixes`, `_skip_flags`, `_runner_program`,
//! `_python_runner_start`, `_strip_version` and their tables.

pub(crate) mod hidden;

use crate::rtk_forms::rtk_command;
use crate::shell_args::{SHELLS, prog_name, shell_parse};
use hidden::{env_attached_split, runs_assigned_command, wrapper_known};

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

/// What `strip_prefixes` peeled that the analysis could not see through (for `Opaque`). Rust-only: Python's
/// `_strip_prefixes` has no such output, so none of these change an entry's parity fields.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Peeled {
    /// A `""` word (a `$(...)` placeholder) was dropped where the program word was expected.
    pub(crate) placeholder: bool,
    /// A wrapper's, runner's, package manager's or rtk's option was not one the analysis knows: it may have
    /// taken the next word as its value. Also `env -S` found past env's own options.
    pub(crate) unknown_option: bool,
    /// The program's arguments come from stdin at run time (`xargs`).
    pub(crate) stdin_args: bool,
    /// A wrapper or runner was peeled and left no program and no script to analyze (`... | xargs npx`).
    pub(crate) emptied: bool,
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

/// Options of the package runners (and of the Python runners' `run`) known to take no value: past one of
/// these the next word is surely the program. Rust-only (`Opaque::UnknownWrapperOption`); parity never reads it.
const RUNNER_BOOL_FLAGS: &[&str] = &[
    "-y",
    "--yes",
    "--no",
    "--no-install",
    "--ignore-existing",
    "-q",
    "--quiet",
    "-v",
    "--verbose",
    "--prefer-offline",
    "--prefer-online",
    "--offline",
    "--bun",
    "-b",
    "--isolated",
    "--no-cache",
    "--refresh",
    "--native-tls",
    "--no-sync",
    "--frozen",
    "--locked",
    "--no-project",
    "--active",
    "--all-packages",
    "--all-extras",
    "--no-dev",
    "--dev",
    "--exact",
    "--silent",
    "-s",
    "-m",
    "--module",
];
/// npm/pnpm/yarn/bun options before the verb known to take no value (as `RUNNER_BOOL_FLAGS`).
const PM_BOOL_FLAGS: &[&str] = &[
    "-s",
    "--silent",
    "-q",
    "--quiet",
    "-g",
    "--global",
    "-r",
    "--recursive",
    "-y",
    "--yes",
    "-D",
    "--dev",
    "-d",
    "--verbose",
    "--offline",
    "--prefer-offline",
    "--frozen-lockfile",
    "--workspace-root",
    "--if-present",
    "--ignore-scripts",
    "--include-workspace-root",
    "--bun",
    "--workspaces",
    "-ws",
    "--parallel",
    "--stream",
    "--sequential",
    "--no-bail",
    "--aggregate-output",
    "--watch",
    "--hot",
    "--smol",
    "--no-install",
];
/// Programs that run a command or script the analysis does not parse (`builtin eval ...`, `su -c ...`,
/// `flock f cmd`, `busybox sh -c ...`, `ssh host cmd`): their entry is marked `UnknownWrapperOption` when they
/// have arguments. Rust-only; parity never reads it.
pub(crate) const UNPARSED_RUNNERS: &[&str] = &[
    "builtin",
    "su",
    "runuser",
    "script",
    "flock",
    "busybox",
    "chroot",
    "nsenter",
    "unshare",
    "parallel",
    "setpriv",
    "sg",
    "systemd-run",
    "firejail",
    "bwrap",
    "pkexec",
    "sandbox-exec",
    "taskset",
    "chrt",
    "numactl",
    "prlimit",
    "strace",
    "ltrace",
    "ssh",
];
/// Programs whose command takes its arguments from stdin (`Opaque::DynamicCommandWord`).
pub(crate) const STDIN_ARG_RUNNERS: &[&str] = &["xargs", "parallel"];

/// `token` is an option known not to take the next word: listed in `bools`, or a `--name=value`.
fn known_bool(token: &str, bools: &[&str]) -> bool {
    bools.contains(&token) || (token.starts_with("--") && token.contains('='))
}

/// `_skip_flags(tokens, i, value_flags)`: the index past the options from `tokens[i]` (it may pass the end,
/// as Python's does). `token[-1].isalpha()` is implied by `"-" + token[-1] in value_flags`: every value flag
/// is `-` and an ASCII letter (lead ruling R1). Rust-only: `unknown` is set for an option that is neither in
/// `value_flags` nor `known` (never when `value_flags` is empty).
pub(crate) fn skip_flags(
    tokens: &[String],
    mut i: usize,
    value_flags: &[&str],
    known: &dyn Fn(&str) -> bool,
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
            if !takes_value && !value_flags.is_empty() && !known(token) {
                *unknown = true;
            }
            i += if takes_value { 2 } else { 1 };
        } else {
            break;
        }
    }
    i
}

/// Drop `toks[..i]` in place (`i` may pass the end, as Python's slices may). Moves pointers, never copies words.
pub(crate) fn drop_front(toks: &mut Vec<String>, i: usize) {
    toks.drain(..i.min(toks.len()));
}

/// `_runner_program`: `toks[i..]` follow a package runner; skip its options and return the rest, starting at
/// the package program (version stripped). `-c <script>` / `--call` is recursed instead. Rust-only: `unknown`
/// is set for an option not known to take a value or not.
fn runner_program(
    mut toks: Vec<String>,
    mut i: usize,
    scripts: &mut Vec<String>,
    unknown: &mut bool,
) -> Vec<String> {
    let n = toks.len();
    while i < n {
        let token = toks[i].as_str();
        if token == "--" {
            i += 1;
            break;
        }
        if token == "-c" || token == "--call" {
            if i + 1 < n {
                scripts.push(toks[i + 1].clone());
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
            *unknown |= !known_bool(token, RUNNER_BOOL_FLAGS);
            i += 1;
        } else {
            break;
        }
    }
    drop_front(&mut toks, i);
    if let Some(first) = toks.first_mut() {
        *first = strip_version(first);
    }
    toks
}

/// `_python_runner_start`: the index of the first word after `uv run` / `uv tool run` / `poetry run` ..., if any.
/// Rust-only: `unknown` is set when an option not known to take a value or not came before the subcommand
/// (whether or not it then matched, if a `run` follows).
fn python_runner_start(
    subcommands: &[&[&str]],
    tokens: &[String],
    unknown: &mut bool,
) -> Option<usize> {
    let mut seen = false;
    let start = skip_flags(
        tokens,
        1,
        PYTHON_GLOBAL_VALUE_FLAGS,
        &|t| known_bool(t, RUNNER_BOOL_FLAGS),
        &mut seen,
    );
    let found = subcommands.iter().find_map(|words| {
        let got = tokens.get(start..start + words.len())?;
        got.iter()
            .zip(words.iter())
            .all(|(a, b)| a == b)
            .then_some(start + words.len())
    });
    // an unknown option may have hidden `run` (`uv --color always run x` is read as plain `uv`)
    *unknown |= seen && (found.is_some() || tokens.iter().any(|t| t == "run"));
    found
}

/// `_strip_prefixes`: peel assignments, keywords, wrappers and package runners off a command. Returns the
/// command starting at its program, the embedded scripts to analyze, and what was peeled blind. Words are
/// dropped in place, never re-copied per peel.
pub(crate) fn strip_prefixes(tokens: &[String]) -> (Vec<String>, Vec<String>, Peeled) {
    let mut toks: Vec<String> = tokens.to_vec();
    let mut scripts = Vec::new();
    let mut peeled = Peeled::default();
    let mut ran = false; // a wrapper or runner (not an assignment or keyword) was looked at
    for _ in 0..64 {
        if toks.is_empty() {
            break;
        }
        let name = prog_name(&toks[0]);
        let word = toks[0].as_str();
        if is_assignment(word) || word == "!" || has(KEYWORDS, word) || word.is_empty() {
            // `""` is a `$(...)` placeholder
            peeled.placeholder |= word.is_empty();
            peeled.unknown_option |= runs_assigned_command(word);
            toks.remove(0);
            continue;
        }
        ran = true;
        if let Some(&(_, value_flags)) = WRAPPERS.iter().find(|(w, _)| *w == name) {
            let mut unknown = false;
            let mut i = skip_flags(
                &toks,
                1,
                value_flags,
                &|t| wrapper_known(&name, t),
                &mut unknown,
            );
            if name == "env" {
                let split = toks.iter().position(|t| t == "-S" || t == "--split-string");
                if let Some(split) = split
                    && split + 1 < toks.len()
                {
                    // Python searches every word, not only env's options: flag a `-S` past them
                    peeled.unknown_option |= split >= i;
                    scripts.push(toks[split + 1].clone());
                    toks.clear();
                    break;
                }
                // `env -S'...'` / `--split-string=...`: Python reads the next word as the program
                peeled.unknown_option |= toks[1..i.min(toks.len())]
                    .iter()
                    .any(|t| env_attached_split(t));
            }
            peeled.unknown_option |=
                unknown || (value_flags.is_empty() && has_option(&toks, i, &name));
            peeled.stdin_args |= has(STDIN_ARG_RUNNERS, &name);
            if has(TIMEOUTS, &name) && i < toks.len() {
                i += 1; // the duration
            }
            if name == "watch" && i < toks.len() && toks[i].contains(' ') {
                scripts.push(toks[i].clone()); // watch runs its argument via sh -c
            }
            drop_front(&mut toks, i);
        } else if name == "command" {
            if toks.len() > 1 && (toks[1] == "-v" || toks[1] == "-V") {
                toks[0] = "which".to_string();
                toks.remove(1);
                return (toks, scripts, peeled); // lookup, not execution
            }
            let mut unknown = false;
            let i = skip_flags(&toks, 1, &[], &|_| false, &mut unknown);
            drop_front(&mut toks, i);
        } else if name == "rtk" {
            toks = rtk_command(toks, &mut peeled.unknown_option);
        } else if has(RUNNERS, &name) {
            toks = runner_program(toks, 1, &mut scripts, &mut peeled.unknown_option);
        } else if let Some(start) = PYTHON_RUNNERS
            .iter()
            .find(|(runner, _)| *runner == name)
            .and_then(|(_, subs)| python_runner_start(subs, &toks, &mut peeled.unknown_option))
        {
            toks = runner_program(toks, start, &mut scripts, &mut peeled.unknown_option);
        } else if has(JS_PACKAGE_MANAGERS, &name) && toks.len() > 1 {
            let mut unknown = false;
            let i = skip_flags(
                &toks,
                1,
                PM_VALUE_FLAGS,
                &|t| known_bool(t, PM_BOOL_FLAGS),
                &mut unknown,
            );
            // an unknown option may have hidden the verb, whichever branch follows
            peeled.unknown_option |= unknown;
            let verb = toks.get(i).map_or("", String::as_str).to_string();
            if has(RUNNER_SUBCOMMANDS, &verb) || (verb == "run" && name != "npm") {
                // `yarn run playwright ...`
                toks = runner_program(toks, i + 1, &mut scripts, &mut peeled.unknown_option);
            } else if verb == "workspace" && name == "yarn" && toks.len() > i + 2 {
                toks.drain(1..i + 2); // `yarn workspace <w> <rest>`
                toks[0] = "yarn".to_string();
            } else if (name == "yarn" || name == "pnpm")
                && !verb.is_empty()
                && !has(PM_VERBS, &verb)
            {
                // `yarn playwright test` runs the bin
                toks = runner_program(toks, i, &mut scripts, &mut peeled.unknown_option);
            } else {
                break;
            }
        } else if toks.len() > 2 && name == "pipx" && toks[1] == "run" {
            toks = runner_program(toks, 2, &mut scripts, &mut peeled.unknown_option);
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
                Some(module) if module <= first_positional => drop_front(&mut toks, module + 1),
                _ => break, // `python -m pip ...` runs `pip`; anything else stays python
            }
        } else {
            break;
        }
    }
    peeled.emptied = ran && scripts.is_empty() && toks.first().is_none_or(String::is_empty);
    if let Some(first) = toks.first() {
        let name = prog_name(first);
        if has(SHELLS, &name) {
            scripts.extend(
                shell_parse(&name, &toks[1..], 0)
                    .scripts
                    .into_iter()
                    .map(|(_, s)| s),
            );
        } else if name == "eval" {
            scripts.push(toks[1..].join(" "));
        }
    }
    (toks, scripts, peeled)
}

/// Whether a flag-less wrapper (`nohup`, `stdbuf`, ...) skipped an option word before index `i`.
/// Rust-only: options known to take no value (`hidden::wrapper_known`) do not count.
fn has_option(toks: &[String], i: usize, name: &str) -> bool {
    toks.iter().take(i.min(toks.len())).skip(1).any(|t| {
        t.starts_with('-') && t.chars().count() > 1 && t != "--" && !wrapper_known(name, t)
    })
}

/// Rust-only: `prog` is a JS package manager running one of its own verbs (`bun install`, `bun test`), not
/// running code as an interpreter.
pub(crate) fn pm_runs_own_verb(prog: &str, args: &[String]) -> bool {
    let mut unused = false;
    let i = skip_flags(
        args,
        0,
        PM_VALUE_FLAGS,
        &|t| known_bool(t, PM_BOOL_FLAGS),
        &mut unused,
    );
    has(JS_PACKAGE_MANAGERS, prog) && args.get(i).is_some_and(|verb| has(PM_VERBS, verb))
}
