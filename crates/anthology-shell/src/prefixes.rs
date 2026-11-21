//! Command prefixes, wrappers and runners (SH2, fix-wave I): guard_rules.py `_strip_prefixes`, `_skip_flags`,
//! `_runner_program`, `_python_runner_start`, `_strip_version`, `_env_split_string`, `_option_scripts`,
//! `_xargs_replace`, the GNU parallel helpers, `_pm_verb_index`, `_alternate_span`, `_alternates`, the git /
//! tar / rsync / trap / assignment script finders, `_python_module` and their tables (owner direction
//! 2026-11-21: fix-wave I's findings are Rust requirements).
//!
//! Beyond the Python reference (case-scoped D-8 in the difftest, each from a reviewer's payload):
//! - a package manager's `--` ends the alternate span only when its verb is not `exec`/`dlx`/`x` (FW-I
//!   review R1-I1: `npm exec --omit dev -- playwright install`);
//! - `doppler`/`op`/`dotenvx`/`infisical run` give their `--command` as a script and run the words after `--`
//!   (FW-I review R1-M3);
//! - SH-FW review, part A: tar's `--checkpoint-action` in any getopt spelling (C-1); git's transport options
//!   (`clone -u`, `--upload-pack`, `--receive-pack`, `--exec`), its `remote.*.uploadpack`/`receivepack` and
//!   `difftool`/`mergetool.*.cmd` keys, and `ext::` URLs (C-2); a cap that would drop input (more than
//!   `MAX_ALTERNATES` alternates, an alternate cut short before the main program, more than `MAX_FED` GNU
//!   parallel inputs or combinations) is too complex (I-3); the wrappers `gosu`, `su-exec`, `chronic`,
//!   `torsocks`, `nodenv exec` and `micromamba run` (M-3);
//! - SH-FW re-review, part A: a `-c alias.<X>=<Y>` without `!` reads `git X ...` as `git Y ...` too (C-2a);
//!   tar reads the words of a `TAR_OPTIONS=` assignment before its arguments (C-1a).

pub(crate) mod hidden;

use hidden::{runs_assigned_command, wrapper_known};

use crate::lexer::{TooComplex, Work, pystr};
use crate::rtk_forms::rtk_command;
use crate::shell_args::WORD;
use crate::shell_args::{SHELLS, prog_name, shell_parse};

/// Shell grammar words peeled off a command (`_KEYWORDS`).
const KEYWORDS: &[&str] = &[
    "if", "then", "else", "elif", "do", "while", "until", "{", "coproc",
];

const SU_VALUE_FLAGS: &[&str] = &[
    "-c",
    "--command",
    "--session-command",
    "-s",
    "--shell",
    "-g",
    "--group",
    "-G",
    "--supp-group",
    "-w",
    "--whitelist-environment",
    "-u",
    "--user",
];

/// Wrapper program -> flags that consume the following word (`_WRAPPERS`).
const WRAPPERS: &[(&str, &[&str])] = &[
    (
        "sudo",
        &[
            "-u",
            "-g",
            "-C",
            "-h",
            "-p",
            "-r",
            "-t",
            "-D",
            "-R",
            "-T",
            "-U",
            "--user",
            "--group",
            "--host",
            "--prompt",
            "--chdir",
            "--close-from",
            "--chroot",
            "--role",
            "--type",
            "--command-timeout",
            "--other-user",
        ],
    ),
    ("doas", &["-u", "-C"]),
    ("env", &["-u", "-C", "--unset", "--chdir"]),
    ("exec", &["-a"]),
    ("time", &["-f", "-o", "--format", "--output"]),
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
            "-J",
            "-R",
            "-S",
            "--max-args",
            "--max-lines",
            "--max-procs",
            "--delimiter",
            "--arg-file",
            "--max-chars",
            "--process-slot-var",
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
    (
        "stdbuf",
        &["-i", "-o", "-e", "--input", "--output", "--error"],
    ),
    ("watch", &["-n", "--interval", "-q", "--equexit"]),
    ("arch", &["-arch", "-e", "-d"]),
    ("gtimeout", &["-s", "-k", "--signal", "--kill-after"]),
    ("setsid", &[]),
    ("ionice", &["-c", "-n", "-p", "-P", "-u"]),
    ("unbuffer", &[]),
    ("builtin", &[]),
    ("busybox", &[]),
    ("chroot", &["--userspec", "--groups"]),
    (
        "nsenter",
        &["-t", "--target", "-S", "--setuid", "-G", "--setgid"],
    ),
    (
        "unshare",
        &[
            "-S",
            "--setuid",
            "-G",
            "--setgid",
            "-R",
            "--root",
            "-w",
            "--wd",
            "--map-user",
            "--map-group",
            "--map-users",
            "--map-groups",
            "--propagation",
            "--setgroups",
        ],
    ),
    (
        "flock",
        &[
            "-w",
            "--wait",
            "--timeout",
            "-E",
            "--conflict-exit-code",
            "-c",
            "--command",
        ],
    ),
    ("taskset", &[]),
    ("chrt", &[]),
    ("setarch", &[]),
    (
        "setpriv",
        &[
            "--reuid",
            "--regid",
            "--ruid",
            "--rgid",
            "--euid",
            "--egid",
            "--groups",
            "--inh-caps",
            "--ambient-caps",
            "--bounding-set",
            "--securebits",
            "--pdeathsig",
            "--selinux-label",
            "--apparmor-profile",
        ],
    ),
    ("prlimit", &["-p", "--pid"]),
    ("fakeroot", &["-l", "--lib", "--faked", "-i", "-s"]),
    (
        "strace",
        &[
            "-o", "-e", "-p", "-s", "-u", "-E", "-P", "-a", "-b", "-I", "-O", "-S", "-X",
            "--output",
        ],
    ),
    ("dbus-run-session", &["--config-file", "--dbus-daemon"]),
    ("dumb-init", &["-r", "--rewrite"]),
    ("tini", &["-e", "-p"]),
    ("pkexec", &["--user"]),
    ("su", SU_VALUE_FLAGS),
    ("runuser", SU_VALUE_FLAGS),
    (
        "script",
        &[
            "-c",
            "--command",
            "-E",
            "--echo",
            "-m",
            "--logging-format",
            "-T",
            "--log-timing",
            "-I",
            "--log-in",
            "-O",
            "--log-out",
            "-B",
            "--log-io",
            "-t",
        ],
    ),
    ("entr", &[]),
    ("corepack", &[]),
    ("cross-env", &[]),
    ("dotenv", &["-e", "-c", "-v", "-p"]),
    (
        "env-cmd",
        &["-f", "--file", "-e", "--environments", "-r", "--rc-file"],
    ),
    (
        "systemd-run",
        &[
            "-u",
            "--unit",
            "-p",
            "--property",
            "-E",
            "--setenv",
            "-M",
            "--machine",
            "-H",
            "--host",
            "--description",
            "--slice",
            "--uid",
            "--gid",
            "--nice",
            "--working-directory",
            "--on-calendar",
            "--on-active",
            "--timer-property",
            "--path-property",
            "--socket-property",
            "--service-type",
        ],
    ),
    ("dbus-launch", &["--config-file"]),
    ("ssh-agent", &["-a", "-E", "-t", "-P", "-O"]),
    (
        "ltrace",
        &[
            "-o",
            "-e",
            "-p",
            "-s",
            "-u",
            "-n",
            "-a",
            "-A",
            "-D",
            "-F",
            "-l",
            "-x",
            "-w",
            "--output",
            "--align",
            "--indent",
            "--library",
        ],
    ),
    ("valgrind", &[]),
    (
        "chpst",
        &[
            "-u", "-U", "-e", "-/", "-n", "-l", "-L", "-m", "-d", "-o", "-p", "-f", "-c",
        ],
    ),
    (
        "cpulimit",
        &["-l", "--limit", "-p", "--pid", "-e", "--exe", "-c", "--cpu"],
    ),
    ("gosu", &[]),
    ("su-exec", &[]),
    ("chronic", &[]),
    (
        "torsocks",
        &[
            "-u",
            "-p",
            "-a",
            "-P",
            "--user",
            "--pass",
            "--address",
            "--port",
        ],
    ),
];

/// Wrappers whose first word(s) after the options are operands (`_WRAPPER_OPERANDS`).
const WRAPPER_OPERANDS: &[&str] = &[
    "timeout", "gtimeout", "chroot", "flock", "taskset", "chrt", "setarch", "script", "gosu",
    "su-exec",
];

/// Options that give a program a shell command line (`_WRAPPER_SCRIPT_OPTIONS`).
const WRAPPER_SCRIPT_OPTIONS: &[(&str, &[&str])] = &[
    ("su", &["-c", "--command", "--session-command"]),
    ("runuser", &["-c", "--command", "--session-command"]),
    ("script", &["-c", "--command"]),
    ("flock", &["-c", "--command"]),
    ("nodemon", &["-x", "--exec"]),
    ("nix-shell", &["--run", "--command"]),
];

/// `<tool> <subcommand> [options] [--] <command>` (`_SUBCOMMAND_WRAPPERS`).
const SUBCOMMAND_WRAPPERS: &[(&str, &[&str])] = &[
    ("mise", &["exec", "x"]),
    ("rtx", &["exec", "x"]),
    ("asdf", &["exec"]),
    ("direnv", &["exec"]),
    ("devbox", &["run"]),
    ("volta", &["run"]),
    ("fnm", &["exec"]),
    ("nvm", &["exec"]),
    ("nodenv", &["exec"]),
    ("pixi", &["run", "exec"]),
    ("perf", &["stat", "record", "trace"]),
];

/// Secret and environment loaders: `<tool> run [--command X] [--] <command>` (Rust-only, FW-I review R1-M3).
const SECRET_RUNNERS: &[&str] = &["doppler", "op", "dotenvx", "infisical"];

/// Programs whose `--` passes arguments on to something that is not a command line (`_DASHDASH_ARGUMENTS`).
const DASHDASH_ARGUMENTS: &[&str] = &[
    "cargo",
    "go",
    "make",
    "gmake",
    "just",
    "task",
    "pytest",
    "jest",
    "vitest",
    "mocha",
    "ava",
    "mvn",
    "gradle",
    "gradlew",
    "dotnet",
    "swift",
    "zig",
    "rustc",
    "helm",
    "terraform",
    "gh",
    "ansible-playbook",
];

/// Programs whose every argument is a shell command line, with their value flags (`_SCRIPT_ARGUMENTS`).
const SCRIPT_ARGUMENTS: &[(&str, &[&str])] = &[
    (
        "concurrently",
        &[
            "-n",
            "--names",
            "-c",
            "--prefix-colors",
            "-p",
            "--prefix",
            "-s",
            "--success",
            "--restart-tries",
            "--restart-after",
            "-m",
            "--max-processes",
            "-t",
            "--timestamp-format",
            "-l",
            "--prefix-length",
            "--name-separator",
            "--default-input-target",
        ],
    ),
    (
        "hyperfine",
        &[
            "-w",
            "--warmup",
            "-m",
            "--min-runs",
            "-M",
            "--max-runs",
            "-r",
            "--runs",
            "-S",
            "--shell",
            "-u",
            "--time-unit",
            "--style",
            "--sort",
            "--export-asciidoc",
            "--export-csv",
            "--export-json",
            "--export-markdown",
            "--export-orgmode",
            "--output",
            "--input",
            "-n",
            "--command-name",
            "-D",
            "--parameter-step-size",
        ],
    ),
];

const PARALLEL_VALUE_FLAGS: &[&str] = &[
    "-j",
    "-P",
    "--jobs",
    "-a",
    "--arg-file",
    "-S",
    "--sshlogin",
    "--sshloginfile",
    "--colsep",
    "-C",
    "-d",
    "--delimiter",
    "-E",
    "-I",
    "-n",
    "-N",
    "-L",
    "--max-args",
    "--max-lines",
    "--halt",
    "--timeout",
    "--results",
    "--joblog",
    "--tmpdir",
    "--workdir",
    "--wd",
    "--tagstring",
    "--env",
    "--retries",
    "--delay",
    "--memfree",
    "--load",
    "--basefile",
    "--return",
    "--transferfile",
];
const PARALLEL_INPUTS: &[&str] = &[":::", "::::", ":::+", "::::+"];

/// stdin lines (xargs, parallel) and `:::` inputs turned into commands (`_MAX_FED`).
pub(crate) const MAX_FED: usize = 64;
/// Alternate readings of one command (`_MAX_ALTERNATES`).
const MAX_ALTERNATES: usize = 64;
/// Words of each alternate (`_MAX_ALTERNATE_WORDS`).
const MAX_ALTERNATE_WORDS: usize = 256;

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
    "--loglevel",
    "--userconfig",
    "--cache",
    "--reporter",
    "--tag",
    "--node-options",
    "--script-shell",
];
pub(crate) const JS_PACKAGE_MANAGERS: &[&str] = &["npm", "pnpm", "yarn", "bun"];
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
    "--registry",
    "--cache",
    "--userconfig",
    "--loglevel",
    "--node-options",
    "--script-shell",
    "--index",
    "--index-url",
    "--extra-index-url",
    "--default-index",
    "--find-links",
    "--with-requirements",
    "--with-editable",
    "--cache-dir",
    "--config-file",
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
    ("micromamba", &[&["run"]]),
];
pub(crate) const PM_VERBS: &[&str] = &[
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

/// Programs that run their arguments on another machine or in a container (`_ELSEWHERE`).
const ELSEWHERE: &[&str] = &[
    "docker",
    "docker-compose",
    "podman",
    "nerdctl",
    "orb",
    "orbctl",
    "ssh",
    "kubectl",
    "limactl",
    "lima",
    "multipass",
    "vagrant",
];
/// Programs that only read or print text (`_SEARCH_PROGRAMS | _G4_EXEMPT_PROGRAMS`).
const QUIET_PROGRAMS: &[&str] = &[
    "grep", "egrep", "fgrep", "rg", "ag", "ack", "git", "echo", "printf", "which", "type", "ls",
    "cat", "head", "tail", "less", "wc", "file", "stat", "rm", "cd", "pushd", "mkdir", "touch",
    "cp", "mv", "find", "du", "diff", "tree", "realpath", "basename", "dirname", "readlink",
    "chmod", "chown", "ln",
];
/// G1's program tables that start an alternate reading (`_PLAYWRIGHT | _PUPPETEER | _OS_INSTALLERS`, plus
/// `create-playwright`, `uv`, `eval`).
const ALTERNATE_EXTRA_STARTS: &[&str] = &[
    "playwright",
    "playwright-core",
    "@playwright/test",
    "@playwright/cli",
    "puppeteer",
    "@puppeteer/browsers",
    "brew",
    "apt",
    "apt-get",
    "dnf",
    "yum",
    "snap",
    "pacman",
    "create-playwright",
    "uv",
    "eval",
];
/// Triggers beyond the peeling tables (`{"command", "rtk", "parallel", "sg", "nodemon", "pipx"}`).
const ALTERNATE_EXTRA_TRIGGERS: &[&str] = &["command", "rtk", "parallel", "sg", "nodemon", "pipx"];

fn has(set: &[&str], word: &str) -> bool {
    set.contains(&word)
}

fn table<'a>(set: &'a [(&str, &'a [&str])], name: &str) -> Option<&'a [&'a str]> {
    set.iter().find(|(w, _)| *w == name).map(|(_, v)| *v)
}

/// `name in _ALTERNATE_TRIGGERS`.
fn is_trigger(name: &str) -> bool {
    table(WRAPPERS, name).is_some()
        || table(SUBCOMMAND_WRAPPERS, name).is_some()
        || table(SCRIPT_ARGUMENTS, name).is_some()
        || PYTHON_RUNNERS.iter().any(|(r, _)| *r == name)
        || has(RUNNERS, name)
        || has(JS_PACKAGE_MANAGERS, name)
        || has(ALTERNATE_EXTRA_TRIGGERS, name)
}

/// `name in _ALTERNATE_STARTS or _PIP.match(name)`.
fn is_alternate_start(name: &str) -> bool {
    is_trigger(name) || has(ALTERNATE_EXTRA_STARTS, name) || has(SHELLS, name) || is_pip(name)
}

/// `_PIP.match(name)`: `pip[0-9.]*\Z`.
fn is_pip(name: &str) -> bool {
    name.strip_prefix("pip")
        .is_some_and(|rest| rest.bytes().all(|b| b.is_ascii_digit() || b == b'.'))
}

/// `_INTERPRETER.match(name)`.
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

/// What `strip_prefixes` peeled that the analysis could not see through (for `Opaque`). Rust-only: Python's
/// `_strip_prefixes` has no such output, so none of these change an entry's parity fields.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Peeled {
    /// A `""` word (a `$(...)` placeholder) was dropped where the program word was expected.
    pub(crate) placeholder: bool,
    /// A wrapper's, runner's, package manager's or rtk's option was not one the analysis knows: it may have
    /// taken the next word as its value.
    pub(crate) unknown_option: bool,
    /// The program's arguments come from stdin at run time (`xargs`, `parallel`).
    pub(crate) stdin_args: bool,
    /// A wrapper or runner was peeled and left no program and no script to analyze (`... | xargs npx`).
    pub(crate) emptied: bool,
}

/// One xargs or GNU parallel layer whose command takes words or lines from stdin: (kind, template, replace
/// string).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StdinRun {
    pub(crate) parallel: bool,
    pub(crate) template: Vec<String>,
    pub(crate) replace: Option<String>,
}

/// An alternate reading: the index in the command of its first word, and its words.
pub(crate) type Alternate = (usize, Vec<String>);

/// `_strip_prefixes`' result.
#[derive(Clone, Debug, Default)]
pub(crate) struct Stripped {
    pub(crate) cmd: Vec<String>,
    pub(crate) scripts: Vec<String>,
    pub(crate) peeled: Peeled,
}

/// `_ASSIGNMENT.match(word)`: `[A-Za-z_][A-Za-z0-9_]*=` at the start.
pub(crate) fn is_assignment(word: &str) -> bool {
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
pub(crate) fn strip_version(word: &str) -> String {
    match word.rfind('@') {
        Some(at) if at > 0 => word[..at].to_string(),
        _ => word.to_string(),
    }
}

/// `_skip_flags(tokens, i, value_flags)`: the index past the options from `tokens[i]` (it may pass the end,
/// as Python's does). `token[-1].isalpha()` is implied by `"-" + token[-1] in value_flags`: every value flag
/// is `-` and an ASCII letter or symbol (lead ruling R1). Rust-only: `unknown` is set for an option that is
/// neither in `value_flags` nor `known` (never when `value_flags` is empty).
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
                        c.is_alphabetic() && {
                            let mut flag = String::from("-");
                            flag.push(c);
                            has(value_flags, &flag)
                        }
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

/// Options of the package runners (and of the Python runners' `run`) known to take no value (Rust-only).
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
/// npm/pnpm/yarn/bun options before the verb known to take no value (Rust-only).
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

/// Drop `toks[..i]` in place (`i` may pass the end, as Python's slices may).
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

/// `_python_runner_start`: the index of the first word after `uv run` / `uv tool run` / `poetry run` ..., if
/// any. Rust-only: `unknown` is set when an option not known to take a value or not came before the
/// subcommand (whether or not it then matched, if a `run` follows).
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

/// `_env_split_string`: for `env ...`, (index of the word holding -S/--split-string, index after its value,
/// the value). Only env's own options count.
fn env_split_string(toks: &[String]) -> Option<(usize, usize, String)> {
    const LONG: &[&str] = &["--split-string", "--unset", "--chdir", "--argv0"];
    let n = toks.len();
    let mut i = 1;
    while i < n {
        let token = toks[i].as_str();
        if token == "--" || token == "-" || !token.starts_with('-') {
            return None;
        }
        if token.starts_with("--") {
            let (name, value) = match token.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (token, None),
            };
            let full = LONG
                .iter()
                .find(|o| name.chars().count() > 2 && o.starts_with(name))
                .copied()
                .unwrap_or("");
            if full == "--split-string" {
                return match value {
                    Some(v) => Some((i, i + 1, v.to_string())),
                    None => toks.get(i + 1).map(|v| (i, i + 2, v.clone())),
                };
            }
            i += if !full.is_empty() && value.is_none() {
                2
            } else {
                1
            };
            continue;
        }
        let mut stepped = false;
        for (at, letter) in token.char_indices().skip(1) {
            if matches!(letter, 'u' | 'C' | 'S' | 'a') {
                let attached = &token[at + letter.len_utf8()..];
                if letter == 'S' {
                    if !attached.is_empty() {
                        return Some((i, i + 1, attached.to_string()));
                    }
                    return toks.get(i + 1).map(|v| (i, i + 2, v.clone()));
                }
                i += if attached.is_empty() { 2 } else { 1 };
                stepped = true;
                break;
            }
        }
        if !stepped {
            i += 1;
        }
    }
    None
}

/// `_split_words`: POSIX shell-like splitting, or a whitespace split on an unbalanced quote.
fn split_words(text: &str) -> Vec<String> {
    pystr::shlex_split(text).unwrap_or_else(|| {
        pystr::split_ws(text)
            .into_iter()
            .map(str::to_string)
            .collect()
    })
}

/// `_option_scripts`: command lines given by `options` in any of `toks[1..]`.
fn option_scripts(toks: &[String], options: &[&str], value_flags: &[&str]) -> Vec<String> {
    let mut found = Vec::new();
    let n = toks.len();
    let mut k = 1;
    while k < n {
        let token = toks[k].as_str();
        let split = token.split_once('=');
        if has(options, token) {
            if k + 1 < n {
                found.push(toks[k + 1].clone());
            }
            k += 1;
        } else if let Some((name, value)) = split
            && name.starts_with("--")
            && has(options, name)
        {
            found.push(value.to_string());
        } else if token.chars().count() > 2
            && token.starts_with('-')
            && !token[1..].starts_with('-')
        {
            for (pos, letter) in token.char_indices().skip(1) {
                let flag = format!("-{letter}");
                if has(options, &flag) {
                    let attached = &token[pos + letter.len_utf8()..];
                    if !attached.is_empty() {
                        found.push(attached.to_string());
                    } else if k + 1 < n {
                        found.push(toks[k + 1].clone());
                        k += 1;
                    }
                    break;
                }
                if has(value_flags, &flag) {
                    break; // the rest is that option's value
                }
            }
        }
        k += 1;
    }
    found
}

/// `_xargs_replace`: the string xargs replaces with each input line, or `None`.
fn xargs_replace(options: &[String]) -> Option<String> {
    const VALUE_LETTERS: &str = "adEeLlnPsRS";
    let mut replace = None;
    let mut k = 0;
    while k < options.len() {
        let token = options[k].as_str();
        if token == "--replace" {
            replace = Some("{}".to_string());
        } else if let Some(r) = token.strip_prefix("--replace=") {
            replace = Some(if r.is_empty() {
                "{}".to_string()
            } else {
                r.to_string()
            });
        } else if token.chars().count() > 1
            && token.starts_with('-')
            && !token[1..].starts_with('-')
        {
            for (pos, letter) in token.char_indices().skip(1) {
                let attached = &token[pos + letter.len_utf8()..];
                if letter == 'I' || letter == 'J' {
                    if !attached.is_empty() {
                        replace = Some(attached.to_string());
                    } else if k + 1 < options.len() {
                        replace = Some(options[k + 1].clone());
                        k += 1;
                    }
                    break;
                }
                if letter == 'i' {
                    replace = Some(if attached.is_empty() {
                        "{}".to_string()
                    } else {
                        attached.to_string()
                    });
                    break;
                }
                if VALUE_LETTERS.contains(letter) {
                    if attached.is_empty() && letter != 'e' && letter != 'l' {
                        k += 1;
                    }
                    break;
                }
            }
        }
        k += 1;
    }
    replace
}

/// `_needs_shell`: `" ".join(words)` would lex into other words (a word holds a blank, a quote or shell syntax,
/// or starts a comment).
fn needs_shell(words: &[String]) -> bool {
    words.iter().any(|w| {
        w.starts_with('#')
            || w.chars().any(|c| {
                pystr::is_space(c)
                    || matches!(
                        c,
                        '\'' | '"' | '`' | '$' | '\\' | ';' | '&' | '|' | '<' | '>' | '(' | ')'
                    )
            })
    })
}

/// `_parallel_replace`: GNU parallel's replacement string.
fn parallel_replace(options: &[String]) -> String {
    let mut replace = "{}".to_string();
    for (k, token) in options.iter().enumerate() {
        if token == "-I" && k + 1 < options.len() {
            replace = options[k + 1].clone();
        } else if token.starts_with("-I") && token.chars().count() > 2 {
            replace = token[2..].to_string();
        } else if let Some(r) = token.strip_prefix("--replace=") {
            replace = if r.is_empty() {
                "{}".to_string()
            } else {
                r.to_string()
            };
        }
    }
    replace
}

/// `_parallel_inputs`: one list per `:::`-style separator. Rust: a source of more than `MAX_FED` words is too
/// complex (Python drops the rest; SH-FW review I-3).
fn parallel_inputs(words: &[String]) -> Result<Vec<Vec<String>>, TooComplex> {
    let mut groups: Vec<Vec<String>> = Vec::new();
    for word in words {
        if has(PARALLEL_INPUTS, word) {
            groups.push(Vec::new());
        } else if let Some(last) = groups.last_mut() {
            last.push(word.clone());
        }
    }
    groups.retain(|g| !g.is_empty());
    if groups.iter().any(|g| g.len() > MAX_FED) {
        return Err(TooComplex);
    }
    Ok(groups)
}

/// `_PARALLEL_POSITION.sub(...)`: each `{N}` replaced by the N-th input of `combo` when it has one.
fn put_positions(line: &str, combo: &[&String]) -> Option<String> {
    let mut out = String::with_capacity(line.len());
    let mut found = false;
    let mut rest = line;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && after.as_bytes().get(digits) == Some(&b'}') {
            found = true;
            out.push_str(&rest[..open]);
            let number: Option<usize> = after[..digits].parse().ok();
            match number.filter(|&k| k > 0 && k <= combo.len()) {
                Some(k) => out.push_str(combo[k - 1]),
                None => out.push_str(&rest[open..open + digits + 2]),
            }
            rest = &after[digits + 1..];
        } else {
            out.push_str(&rest[..=open]);
            rest = after;
        }
    }
    out.push_str(rest);
    found.then_some(out)
}

/// `_parallel_lines`: the command lines GNU parallel runs, at most `MAX_FED`. Rust: each line's size is
/// charged before it is built (a line of many `{}` and a long input would otherwise be quadratic), and more
/// than `MAX_FED` combinations are too complex (Python drops the rest; SH-FW review I-3).
pub(crate) fn parallel_lines(
    line: &str,
    replace: &str,
    groups: &[Vec<String>],
    work: &mut Work,
) -> Result<Vec<String>, TooComplex> {
    let mut out = Vec::new();
    if groups.is_empty() || groups.iter().any(Vec::is_empty) {
        return Ok(out);
    }
    work.charge_extra(2 * line.len())?;
    let braces = line.bytes().filter(|&b| b == b'{').count();
    let replaced = if replace.is_empty() {
        line.chars().count() + 1
    } else {
        line.matches(replace).count()
    };
    let mut index = vec![0usize; groups.len()];
    loop {
        if out.len() >= MAX_FED {
            return Err(TooComplex);
        }
        let combo: Vec<&String> = index
            .iter()
            .enumerate()
            .map(|(g, &k)| &groups[g][k])
            .collect();
        let joined = combo
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let size = (braces.max(replaced) + 1).saturating_mul(joined.len());
        work.charge_extra(WORD + line.len() + size)?;
        if let Some(put) = put_positions(line, &combo) {
            out.push(put);
        } else if line.is_empty() {
            out.push(joined);
        } else if line.contains(replace) {
            out.push(line.replace(replace, &joined));
        } else {
            out.push(format!("{line} {joined}"));
        }
        // the next combination, the last source fastest (`itertools.product`)
        let mut g = groups.len();
        loop {
            if g == 0 {
                return Ok(out);
            }
            g -= 1;
            index[g] += 1;
            if index[g] < groups[g].len() {
                break;
            }
            index[g] = 0;
        }
    }
}

/// `_unknown_option_before`: `toks[i - 1]` is an option whose value-taking the tables do not know.
fn unknown_option_before(toks: &[String], i: usize) -> bool {
    let prev = if i > 0 { toks[i - 1].as_str() } else { "" };
    prev.chars().count() > 1
        && prev.starts_with('-')
        && prev != "--"
        && !prev.contains('=')
        && !has(PM_VALUE_FLAGS, prev)
}

/// `_pm_verb_index`: the index of a package manager's subcommand in `toks`.
fn pm_verb_index(toks: &[String], unknown: &mut bool) -> usize {
    let known = |t: &str| known_bool(t, PM_BOOL_FLAGS);
    let n = toks.len();
    let mut i = skip_flags(toks, 1, PM_VALUE_FLAGS, &known, unknown);
    while i < n && !has(PM_VERBS, &toks[i]) && unknown_option_before(toks, i) {
        i = skip_flags(toks, i + 1, PM_VALUE_FLAGS, &known, unknown);
    }
    if i < n {
        i
    } else {
        skip_flags(toks, 1, PM_VALUE_FLAGS, &known, unknown)
    }
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

/// `_alternate_span(tokens, main_at)`: the words that may start an alternate reading. Rust: a package
/// manager's `--` ends the span only when its verb is not `exec`/`dlx`/`x`, whose `--` ends npm's own
/// options before the program (FW-I review R1-I1: `npm exec --omit dev -- playwright install`); after a
/// script or test verb the words are the script's arguments (`pnpm run e2e -- playwright test`).
fn alternate_span(tokens: &[String], main_at: usize) -> Option<(usize, usize)> {
    for (k, word) in tokens.iter().enumerate() {
        if is_assignment(word) || word == "!" || has(KEYWORDS, word) || word.is_empty() {
            continue;
        }
        let name = prog_name(word);
        let rest = &tokens[k + 1..];
        if is_trigger(&name) {
            let after = tokens.get(main_at + 1..).unwrap_or(&[]);
            if has(JS_PACKAGE_MANAGERS, &name)
                && let Some(dd) = after.iter().position(|t| t == "--")
            {
                let mut unused = false;
                let verb = k + pm_verb_index(&tokens[k..], &mut unused);
                if !tokens.get(verb).is_some_and(|v| has(RUNNER_SUBCOMMANDS, v)) {
                    return Some((k + 1, main_at + 1 + dd));
                }
            }
            return Some((k + 1, tokens.len()));
        }
        let dd = rest.iter().position(|t| t == "--")?;
        if has(QUIET_PROGRAMS, &name)
            || has(ELSEWHERE, &name)
            || has(SHELLS, &name)
            || has(DASHDASH_ARGUMENTS, &name)
            || is_interpreter(&name)
        {
            return None;
        }
        return Some((k + 2 + dd, tokens.len()));
    }
    None
}

/// `_alternates(tokens, start, end)`: each word naming a program the peeling or G1 knows starts one, up to
/// the first word naming a program in `ELSEWHERE`. Each with the index of its first word in `tokens`. Rust
/// (SH-FW review I-3): where Python drops input, the analysis is too complex: more than `MAX_ALTERNATES`
/// alternates, or one cut at `MAX_ALTERNATE_WORDS` that does not start at the main program (`main_at`) and
/// does not read as it either (its stripped command is not the start of `main`, the main reading, which keeps
/// every word: `sudo -u x timeout 5 npx eslint <300 files>` is complete).
fn alternates_of(
    tokens: &[String],
    start: usize,
    end: usize,
    (main_at, main): (usize, &[String]),
    work: &mut Work,
) -> Result<Vec<Alternate>, TooComplex> {
    let mut out = Vec::new();
    for k in start..end.min(tokens.len()) {
        let word = strip_version(&tokens[k]);
        let name = prog_name(&word);
        if has(ELSEWHERE, &name) {
            break;
        }
        if is_alternate_start(&name) {
            if out.len() >= MAX_ALTERNATES {
                return Err(TooComplex);
            }
            let stop = (k + MAX_ALTERNATE_WORDS).min(tokens.len());
            let mut alt = Vec::with_capacity(stop - k);
            alt.push(word);
            if k + 1 < stop {
                alt.extend(tokens[k + 1..stop].iter().cloned());
            }
            if k != main_at && stop < tokens.len() {
                work.charge_extra(alt.iter().map(|w| WORD + w.len()).sum())?;
                let read = strip_prefixes(&alt, None, None, false, work)?.cmd;
                if read.is_empty() || !main.starts_with(&read) {
                    return Err(TooComplex);
                }
            }
            out.push((k, alt));
        }
    }
    Ok(out)
}

/// `key` is `<section>.<subsection>.<name>` with a non-empty subsection (`key` lowercased, `name` too).
fn git_subsection_key(key: &str, section: &str, name: &str) -> bool {
    key.strip_prefix(section)
        .and_then(|r| r.strip_prefix('.'))
        .and_then(|r| r.strip_suffix(name))
        .and_then(|r| r.strip_suffix('.'))
        .is_some_and(|middle| !middle.is_empty())
}

/// The command a git `ext::` URL runs: its words split at unescaped blanks (`% ` is a blank in a word, `%%`
/// a `%`), quoted for the shell; git runs them without one (git-remote-ext).
fn git_ext_command(url: &str) -> Option<String> {
    let rest = url.strip_prefix("ext::")?;
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '%' => match chars.next() {
                Some(' ') => word.push(' '),
                Some('%') => word.push('%'),
                Some(other) => {
                    word.push('%');
                    word.push(other);
                }
                None => word.push('%'),
            },
            ' ' => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            _ => word.push(c),
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    let quoted: Vec<String> = words.iter().map(|w| pystr::shlex_quote(w)).collect();
    Some(quoted.join(" "))
}

/// `_git_command_value`: the shell command git config `key` = `value` runs. Rust (SH-FW review C-2): also
/// `remote.*.uploadpack`/`receivepack`, `difftool.*.cmd`, `mergetool.*.cmd`, an `ext::` URL in
/// `remote.*.url`/`pushurl`, and an `ext::` base in `url.*.insteadOf`/`pushInsteadOf`.
fn git_command_value(key: &str, value: &str) -> Option<String> {
    let original = key;
    let key = pystr::lower(key);
    if git_subsection_key(&key, "remote", "uploadpack")
        || git_subsection_key(&key, "remote", "receivepack")
        || git_subsection_key(&key, "difftool", "cmd")
        || git_subsection_key(&key, "mergetool", "cmd")
    {
        return Some(value.to_string());
    }
    if git_subsection_key(&key, "remote", "url") || git_subsection_key(&key, "remote", "pushurl") {
        return git_ext_command(value);
    }
    if key.starts_with("url.ext::") {
        // the subsection keeps its case: `url.<base>.insteadOf`
        let base = original.get(4..original.rfind('.').unwrap_or(4))?;
        return git_ext_command(base);
    }
    if key.starts_with("alias.")
        || (key.starts_with("credential.") && key.ends_with(".helper"))
        || key == "credential.helper"
    {
        return value.strip_prefix('!').map(str::to_string);
    }
    const KEYS: &[&str] = &[
        "core.pager",
        "core.sshcommand",
        "core.fsmonitor",
        "core.editor",
        "core.askpass",
        "sequence.editor",
        "diff.external",
        "interactive.difffilter",
    ];
    if has(KEYS, &key) || git_command_key(&key) {
        return Some(value.to_string());
    }
    None
}

/// `_GIT_COMMAND_KEY.match(key)`: `pager\.[^.]+\Z|(?:diff|merge|filter)\..+\.(?:textconv|...)\Z`.
fn git_command_key(key: &str) -> bool {
    if let Some(rest) = key.strip_prefix("pager.") {
        return !rest.is_empty() && !rest.contains('.');
    }
    let Some(rest) = ["diff.", "merge.", "filter."]
        .iter()
        .find_map(|p| key.strip_prefix(p))
    else {
        return false;
    };
    const ENDS: &[&str] = &[
        "textconv", "command", "driver", "clean", "smudge", "process",
    ];
    ENDS.iter().any(|end| {
        rest.strip_suffix(end)
            .and_then(|m| m.strip_suffix('.'))
            .is_some_and(|middle| !middle.is_empty() && !middle.contains('\n'))
    })
}

/// `_assignment_scripts`: commands that `NAME=value` words before a program hand to it.
fn assignment_scripts(words: &[String]) -> Vec<String> {
    const COMMAND_VARIABLES: &[&str] = &[
        "GIT_SSH_COMMAND",
        "GIT_SSH",
        "GIT_PAGER",
        "GIT_EDITOR",
        "GIT_SEQUENCE_EDITOR",
        "GIT_EXTERNAL_DIFF",
        "GIT_ASKPASS",
        "PAGER",
        "EDITOR",
        "VISUAL",
        "RSYNC_RSH",
    ];
    // `dict(word.split("=", 1) ...)`: first-seen order, last value
    let mut values: Vec<(&str, &str)> = Vec::new();
    for word in words {
        let (name, value) = word.split_once('=').unwrap_or((word.as_str(), ""));
        match values.iter_mut().find(|(n, _)| *n == name) {
            Some(slot) => slot.1 = value,
            None => values.push((name, value)),
        }
    }
    let mut out = Vec::new();
    for &(name, value) in &values {
        if has(COMMAND_VARIABLES, name) {
            out.push(Some(value.to_string()));
        } else if name == "GIT_CONFIG_PARAMETERS" {
            for (key, val) in git_config_parameters(value) {
                out.push(git_command_value(&key, &val));
            }
        } else if let Some(n) = name.strip_prefix("GIT_CONFIG_KEY_")
            && !n.is_empty()
            && n.bytes().all(|b| b.is_ascii_digit())
        {
            let other = format!("GIT_CONFIG_VALUE_{n}");
            if let Some(&(_, v)) = values.iter().find(|(k, _)| *k == other) {
                out.push(git_command_value(value, v));
            }
        }
    }
    out.into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect()
}

/// `re.finditer(r"'([^']*)'(?:='([^']*)')?", value)` → (key, value) pairs as `_assignment_scripts` reads them.
fn git_config_parameters(value: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = value;
    while let Some(open) = rest.find('\'') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('\'') else {
            break;
        };
        let first = &after[..close];
        let mut tail = &after[close + 1..];
        let mut second = None;
        if let Some(t) = tail.strip_prefix("='")
            && let Some(end) = t.find('\'')
        {
            second = Some(&t[..end]);
            tail = &t[end + 1..];
        }
        match second {
            Some(v) => out.push((first.to_string(), v.to_string())),
            None => {
                let (k, v) = first.split_once('=').unwrap_or((first, ""));
                out.push((k.to_string(), v.to_string()));
            }
        }
        rest = tail;
    }
    out
}

/// `_positional(args, value_flags)`.
fn positional<'a>(args: &'a [String], value_flags: &[&str]) -> Vec<&'a String> {
    let mut words = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if has(value_flags, &args[i]) {
            i += 1;
        } else if !args[i].starts_with('-') {
            words.push(&args[i]);
        }
        i += 1;
    }
    words
}

/// git's own options that take a value (`_GIT_VALUE_FLAGS`).
const GIT_VALUE_FLAGS: &[&str] = &[
    "-c",
    "-C",
    "--git-dir",
    "--work-tree",
    "--namespace",
    "--config-env",
    "--super-prefix",
    "--list-cmds",
];

/// The subcommand of a git invocation (`""` when none) and the words after it.
fn git_subcommand(args: &[String]) -> (&str, &[String]) {
    let mut unused = false;
    let i = skip_flags(args, 0, GIT_VALUE_FLAGS, &|_| true, &mut unused);
    match args.get(i) {
        Some(sub) => (sub.as_str(), &args[i + 1..]),
        None => ("", &[]),
    }
}

/// Rust-only (SH-FW review C-2): git subcommands' options whose value git runs as a command (a long name may
/// be shortened, as git's parse-options allows). `clone -u` is `--upload-pack` too.
const GIT_COMMAND_OPTIONS: &[(&str, &[&str])] = &[
    ("clone", &["--upload-pack"]),
    ("fetch", &["--upload-pack"]),
    ("pull", &["--upload-pack"]),
    ("ls-remote", &["--upload-pack", "--exec"]),
    ("fetch-pack", &["--upload-pack", "--exec"]),
    ("push", &["--receive-pack", "--exec"]),
    ("send-pack", &["--receive-pack", "--exec"]),
    ("archive", &["--exec"]),
];

/// Rust-only (SH-FW review C-2): the commands a git invocation runs from its transport options
/// (`clone -u X`, `--upload-pack X`, `push --receive-pack=X`, `archive --exec X`) and its `ext::` URLs.
fn git_transport_scripts(args: &[String]) -> Vec<String> {
    let mut out: Vec<String> = args.iter().filter_map(|a| git_ext_command(a)).collect();
    let (sub, rest) = git_subcommand(args);
    let Some(longs) = table(GIT_COMMAND_OPTIONS, sub) else {
        return out;
    };
    for (k, arg) in rest.iter().enumerate() {
        let following = rest.get(k + 1).map(String::as_str);
        let (name, value) = match arg.split_once('=') {
            Some((name, value)) => (name, Some(value)),
            None => (arg.as_str(), None),
        };
        if name.len() > 2 && name.starts_with("--") && longs.iter().any(|o| o.starts_with(name)) {
            out.extend(value.or(following).map(str::to_string));
        } else if sub == "clone"
            && let Some(letters) = arg.strip_prefix('-').filter(|l| !l.starts_with('-'))
        {
            for (at, c) in letters.char_indices() {
                if c == 'u' {
                    let attached = &letters[at + c.len_utf8()..];
                    out.extend(if attached.is_empty() {
                        following.map(str::to_string)
                    } else {
                        Some(attached.to_string())
                    });
                    break;
                }
                if "obcj".contains(c) {
                    break; // the rest is that option's value
                }
            }
        }
    }
    out
}

/// How many `-c alias.*` expansions in a row `git_scripts` follows (git chains aliases).
const GIT_ALIAS_DEPTH: usize = 4;

/// Rust-only (SH-FW re-review C-2a): `args` with its subcommand word replaced by the words of the last
/// `-c alias.<sub>=<Y>` before it, unless Y is a `!` shell command (`git_command_value` has that one). Key and
/// subcommand compare case-insensitively, as git's alias lookup does. A builtin name is expanded too
/// (stricter than git, which runs the builtin).
fn git_alias_reading(args: &[String]) -> Option<Vec<String>> {
    let mut unused = false;
    let i = skip_flags(args, 0, GIT_VALUE_FLAGS, &|_| true, &mut unused);
    let sub = args.get(i)?;
    let value = (0..i.saturating_sub(1))
        .filter(|&k| args[k] == "-c")
        .filter_map(|k| args[k + 1].split_once('='))
        .rfind(|(key, _)| {
            key.get(..6)
                .is_some_and(|p| p.eq_ignore_ascii_case("alias."))
                && key.get(6..).is_some_and(|n| n.eq_ignore_ascii_case(sub))
        })?
        .1;
    if value.starts_with('!') {
        return None;
    }
    let words = split_words(value);
    if words.is_empty() {
        return None;
    }
    let mut reading = args[..i].to_vec();
    reading.extend(words);
    reading.extend_from_slice(&args[i + 1..]);
    Some(reading)
}

/// `_git_scripts`: commands a git invocation runs from its config or its command-running subcommands. Rust:
/// also `git_transport_scripts` (SH-FW review C-2), and the same scans of the invocation read through up to
/// `GIT_ALIAS_DEPTH` aliases (SH-FW re-review C-2a), each reading charged to `work`.
fn git_scripts(args: &[String], work: &mut Work) -> Result<Vec<String>, TooComplex> {
    let mut out = git_reading_scripts(args);
    let mut seen: std::collections::HashSet<String> = out.iter().cloned().collect();
    let mut next = git_alias_reading(args);
    for _ in 0..GIT_ALIAS_DEPTH {
        let Some(reading) = next else {
            break;
        };
        work.charge_extra(reading.iter().map(|w| WORD + w.len()).sum())?;
        for script in git_reading_scripts(&reading) {
            if seen.insert(script.clone()) {
                out.push(script);
            }
        }
        next = git_alias_reading(&reading);
    }
    Ok(out)
}

/// `git_scripts` for one reading of the invocation.
fn git_reading_scripts(args: &[String]) -> Vec<String> {
    const GIT_CONFIG_VALUE_FLAGS: &[&str] = &[
        "-f",
        "--file",
        "--blob",
        "--type",
        "--default",
        "--comment",
        "--value",
    ];
    let mut out: Vec<Option<String>> = Vec::new();
    for k in 0..args.len().saturating_sub(1) {
        if args[k] == "-c" {
            out.push(
                args[k + 1]
                    .split_once('=')
                    .and_then(|(key, value)| git_command_value(key, value)),
            );
        }
    }
    let mut unused = false;
    let (sub, rest) = git_subcommand(args);
    match sub {
        "config" => {
            let mut words = positional(rest, GIT_CONFIG_VALUE_FLAGS);
            if words.first().is_some_and(|w| *w == "set") {
                words.remove(0);
            }
            if words.len() >= 2 {
                out.push(git_command_value(words[0], words[1]));
            }
        }
        "bisect" if rest.first().is_some_and(|w| w == "run") => {
            let quoted: Vec<String> = rest[1..].iter().map(|w| pystr::shlex_quote(w)).collect();
            out.push(Some(quoted.join(" ")));
        }
        "submodule" => {
            let k = skip_flags(rest, 0, &[], &|_| true, &mut unused);
            if rest.get(k).is_some_and(|w| w == "foreach") {
                let from = skip_flags(rest, k + 1, &[], &|_| true, &mut unused);
                out.push(Some(rest.get(from..).unwrap_or(&[]).join(" ")));
            }
        }
        "rebase" | "difftool" => {
            let long = if sub == "rebase" {
                "--exec"
            } else {
                "--extcmd"
            };
            for (k, arg) in rest.iter().enumerate() {
                if arg == "-x" || arg == long {
                    out.push(rest.get(k + 1).cloned());
                } else if let Some(v) = arg.strip_prefix(long).and_then(|r| r.strip_prefix('=')) {
                    out.push(Some(v.to_string()));
                } else if arg.starts_with("-x") && arg.chars().count() > 2 {
                    out.push(Some(arg[2..].to_string()));
                }
            }
        }
        _ => {}
    }
    out.extend(git_transport_scripts(args).into_iter().map(Some));
    out.into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect()
}

/// GNU tar's `--checkpoint-action` as getopt reads it: the whole name or any prefix longer than
/// `--checkpoint` (Rust-only, SH-FW review C-1).
fn is_checkpoint_action(name: &str) -> bool {
    name.len() > "--checkpoint".len() && "--checkpoint-action".starts_with(name)
}

/// `_tar_scripts`: commands GNU tar runs. Rust: `--checkpoint-action` in any spelling, its value after `=`
/// or in the next word (SH-FW review C-1).
fn tar_scripts(args: &[String]) -> Vec<String> {
    const OPTIONS: &[&str] = &[
        "--to-command",
        "--use-compress-program",
        "--info-script",
        "--new-volume-script",
        "--rsh-command",
    ];
    let mut out: Vec<Option<String>> = Vec::new();
    for (k, arg) in args.iter().enumerate() {
        let following = args.get(k + 1).cloned();
        let (name, value) = match arg.split_once('=') {
            Some((name, value)) => (name, Some(value)),
            None => (arg.as_str(), None),
        };
        if arg == "-I" || arg == "-F" {
            out.push(following);
        } else if arg.starts_with("-I") || arg.starts_with("-F") {
            out.push(Some(arg[2..].to_string()));
        } else if is_checkpoint_action(name) {
            let action = value.or(following.as_deref());
            out.push(
                action
                    .and_then(|a| a.strip_prefix("exec="))
                    .map(str::to_string),
            );
        } else if name.starts_with("--")
            && name.chars().count() > 4
            && OPTIONS.iter().any(|o| o.starts_with(name))
        {
            out.push(match value {
                Some(v) => Some(v.to_string()),
                None => following,
            });
        }
    }
    out.into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect()
}

/// `_rsync_scripts`: the remote shell rsync runs.
fn rsync_scripts(args: &[String]) -> Vec<String> {
    let mut out: Vec<Option<String>> = Vec::new();
    for (k, arg) in args.iter().enumerate() {
        let following = args.get(k + 1).cloned();
        if arg == "--rsh" {
            out.push(following);
        } else if let Some(v) = arg.strip_prefix("--rsh=") {
            out.push(Some(v.to_string()));
        } else if arg.chars().count() > 1
            && arg.starts_with('-')
            && !arg[1..].starts_with('-')
            && arg[1..].contains('e')
        {
            let at = 1 + arg[1..].find('e').unwrap_or(0);
            let attached = &arg[at + 1..];
            out.push(if attached.is_empty() {
                following
            } else {
                Some(attached.to_string())
            });
        }
    }
    out.into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect()
}

/// `_python_module`: for `python ... -m mod args`, `[mod, args...]`.
fn python_module(toks: &[String]) -> Option<Vec<String>> {
    let n = toks.len();
    let mut k = 1;
    while k < n {
        let token = toks[k].as_str();
        if token == "-X" || token == "-W" {
            k += 2;
            continue;
        }
        if token == "-" || token == "--" || !token.starts_with('-') {
            return None;
        }
        if token.starts_with("--") {
            k += 1;
            continue;
        }
        let letters = &token[1..];
        for (pos, letter) in letters.char_indices() {
            if letter == 'm' {
                let attached = &letters[pos + 1..];
                if !attached.is_empty() {
                    let mut out = vec![attached.to_string()];
                    out.extend(toks[k + 1..].iter().cloned());
                    return Some(out);
                }
                return if k + 1 < n {
                    Some(toks[k + 1..].to_vec())
                } else {
                    None
                };
            }
            if letter == 'c' {
                return None;
            }
            if letter == 'X' || letter == 'W' {
                if letters[pos + 1..].is_empty() {
                    k += 1;
                }
                break;
            }
        }
        k += 1;
    }
    None
}

/// `_trap_script`: `trap [--] <command> <signal>...`.
fn trap_script(args: &[String]) -> Option<String> {
    let mut i = 0;
    while i < args.len() && args[i].starts_with('-') && args[i] != "-" {
        i += 1;
        if args[i - 1] == "--" {
            break;
        }
    }
    (args.len().saturating_sub(i) >= 2 && args[i] != "-").then(|| args[i].clone())
}

/// `_strip_prefixes`: peel assignments, keywords, wrappers and package runners off a command. Returns the
/// command starting at its program, the embedded scripts to analyze, and what was peeled blind. When given,
/// `alternates` receives other token lists that may be the real command, and `stdin_runs` the command's first
/// xargs / parallel layer that takes words or lines from stdin. With `layer_scripts` false (an alternate
/// reading) no joined watch/sg/parallel line is added. Rust: the words copied into alternates and the GNU
/// parallel lines are charged to `work`.
pub(crate) fn strip_prefixes(
    tokens: &[String],
    alternates: Option<&mut Vec<Alternate>>,
    mut stdin_runs: Option<&mut Vec<StdinRun>>,
    layer_scripts: bool,
    work: &mut Work,
) -> Result<Stripped, TooComplex> {
    let mut toks: Vec<String> = tokens.to_vec();
    let mut scripts: Vec<String> = Vec::new();
    let mut assigned: Vec<String> = Vec::new();
    let mut peeled = Peeled::default();
    // each command-running construct is taken once per command (FW-I review C1)
    let mut joined = !layer_scripts;
    let mut fed = false;
    let mut scanned: Vec<String> = Vec::new();
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
            if is_assignment(word) {
                assigned.push(toks[0].clone());
            }
            toks.remove(0);
            continue;
        }
        ran = true;
        if name == "env"
            && let Some((at, after, value)) = env_split_string(&toks)
        {
            // the split words are env's arguments
            let tail: Vec<String> = toks.drain(at..after.min(toks.len())).collect();
            drop(tail);
            let words = split_words(&value);
            toks.splice(at..at, words);
        } else if name == "busybox" && (toks.len() < 2 || toks[1].starts_with('-')) {
            break; // `busybox --list`: no applet
        } else if let Some(flags) = table(WRAPPERS, &name) {
            if let Some(options) = table(WRAPPER_SCRIPT_OPTIONS, &name)
                && !scanned.contains(&name)
            {
                scanned.push(name.clone());
                scripts.extend(option_scripts(&toks, options, flags));
            }
            let mut unknown = false;
            let known = |t: &str| wrapper_known(&name, t);
            let mut i = skip_flags(&toks, 1, flags, &known, &mut unknown);
            if has(WRAPPER_OPERANDS, &name) && i < toks.len() {
                i = skip_flags(&toks, i + 1, flags, &known, &mut unknown);
            }
            peeled.unknown_option |= unknown || (flags.is_empty() && has_option(&toks, i, &name));
            let rest = toks.get(i..).unwrap_or(&[]);
            if name == "watch" && !joined {
                // watch runs its words via sh -c
                joined = true;
                if needs_shell(rest) {
                    scripts.push(rest.join(" "));
                }
            }
            if name == "entr"
                && i < toks.len()
                && toks[1..i]
                    .iter()
                    .any(|t| t.starts_with('-') && !t[1..].starts_with('-') && t.contains('s'))
            {
                scripts.push(toks[i].clone()); // `entr -s X` runs X via $SHELL
            }
            if name == "xargs" {
                peeled.stdin_args = true;
                if let Some(runs) = stdin_runs.as_deref_mut()
                    && !fed
                {
                    fed = true;
                    runs.push(StdinRun {
                        parallel: false,
                        template: rest.to_vec(),
                        replace: xargs_replace(toks.get(1..i).unwrap_or(&[])),
                    });
                }
            }
            drop_front(&mut toks, i);
        } else if name == "sg" {
            // `sg [-] group [-c] command`: sh -c
            let mut rest: Vec<String> = toks[1..].to_vec();
            if rest.first().is_some_and(|w| w == "-") {
                drop_front(&mut rest, 2);
            } else {
                drop_front(&mut rest, 1);
            }
            if rest.first().is_some_and(|w| w == "-c") {
                rest.remove(0);
            }
            if !joined {
                joined = true;
                if needs_shell(&rest) {
                    scripts.push(rest.join(" "));
                }
            }
            toks = rest;
        } else if name == "parallel" {
            peeled.stdin_args = true;
            let mut unused = false;
            let i = skip_flags(&toks, 1, PARALLEL_VALUE_FLAGS, &|_| true, &mut unused);
            let replace = parallel_replace(toks.get(1..i).unwrap_or(&[]));
            let rest = toks.get(i..).unwrap_or(&[]);
            let cut = rest
                .iter()
                .position(|t| has(PARALLEL_INPUTS, t))
                .unwrap_or(rest.len());
            let template: Vec<String> = rest[..cut].to_vec();
            if !joined {
                // parallel runs its line via a shell
                joined = true;
                let line = template.join(" ");
                if needs_shell(&template) {
                    scripts.push(line.clone());
                }
                scripts.extend(parallel_lines(
                    &line,
                    &replace,
                    &parallel_inputs(&rest[cut..])?,
                    work,
                )?);
            }
            if cut == rest.len()
                && let Some(runs) = stdin_runs.as_deref_mut()
                && !fed
            {
                fed = true;
                runs.push(StdinRun {
                    parallel: true,
                    template: template.clone(),
                    replace: Some(replace),
                });
            }
            toks = template;
        } else if name == "gdb" {
            // `gdb [options] --args prog args`
            let Some(at) = toks.iter().position(|t| t == "--args") else {
                break;
            };
            drop_front(&mut toks, at + 1);
        } else if name == "nix" && toks.get(1).is_some_and(|t| t == "develop" || t == "shell") {
            let Some(at) = toks.iter().position(|t| t == "-c" || t == "--command") else {
                break;
            };
            drop_front(&mut toks, at + 1); // `nix develop -c cmd args`
        } else if let Some(flags) = table(SCRIPT_ARGUMENTS, &name) {
            let mut k = 1;
            while k < toks.len() {
                if has(flags, &toks[k]) {
                    k += 1;
                } else if !toks[k].starts_with('-') {
                    scripts.push(toks[k].clone());
                }
                k += 1;
            }
            break;
        } else if let Some(options) = table(WRAPPER_SCRIPT_OPTIONS, &name) {
            // `nodemon --exec X`
            scripts.extend(option_scripts(&toks, options, &[]));
            break;
        } else if let Some(subs) = table(SUBCOMMAND_WRAPPERS, &name)
            && toks.len() > 1
            && has(subs, &toks[1])
        {
            let skip = if name == "direnv" { 3 } else { 2 }; // `direnv exec <dir> cmd`
            let mut rest: Vec<String> = toks.get(skip..).unwrap_or(&[]).to_vec();
            match rest.iter().position(|t| t == "--") {
                Some(dd) => drop_front(&mut rest, dd + 1),
                None => {
                    let mut unused = false;
                    let i = skip_flags(&rest, 0, &[], &|_| true, &mut unused);
                    drop_front(&mut rest, i);
                }
            }
            toks = rest;
        } else if has(SECRET_RUNNERS, &name) && toks.get(1).is_some_and(|t| t == "run") {
            // Rust-only (FW-I review R1-M3): `doppler run --command X`, `op run -- cmd`
            scripts.extend(option_scripts(&toks, &["--command"], &[]));
            match toks.iter().position(|t| t == "--") {
                Some(dd) => drop_front(&mut toks, dd + 1),
                None => break,
            }
        } else if name == "command" {
            if toks.len() > 1 && (toks[1] == "-v" || toks[1] == "-V") {
                toks[0] = "which".to_string();
                toks.remove(1);
                // lookup, not execution
                return Ok(Stripped {
                    cmd: toks,
                    scripts,
                    peeled,
                });
            }
            let mut unused = false;
            let i = skip_flags(&toks, 1, &[], &|_| true, &mut unused);
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
            let i = pm_verb_index(&toks, &mut unknown);
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
            match python_module(&toks) {
                Some(module) => toks = module, // `python -m pip ...` runs `pip`
                None => break,
            }
        } else {
            break;
        }
    }
    peeled.emptied = ran && scripts.is_empty() && toks.first().is_none_or(String::is_empty);
    scripts.extend(assignment_scripts(&assigned));
    if let Some(first) = toks.first() {
        let name = prog_name(first);
        let rest = &toks[1..];
        if has(SHELLS, &name) {
            scripts.extend(
                shell_parse(&name, rest, 0)
                    .scripts
                    .into_iter()
                    .map(|(_, s)| s),
            );
        } else if name == "eval" {
            scripts.push(rest.join(" "));
        } else if name == "trap" {
            scripts.extend(trap_script(rest).filter(|s| !s.is_empty())); // runs at the signal or at exit
        } else if name == "git" {
            scripts.extend(git_scripts(rest, work)?);
        } else if name == "tar" || name == "gtar" || name == "bsdtar" {
            // Rust-only (SH-FW re-review C-1a): GNU tar reads the words of `TAR_OPTIONS` before its arguments
            match assigned
                .iter()
                .rev()
                .find_map(|w| w.strip_prefix("TAR_OPTIONS="))
            {
                Some(options) => {
                    let mut words = split_words(options);
                    words.extend_from_slice(rest);
                    work.charge_extra(words.iter().map(|w| WORD + w.len()).sum())?;
                    scripts.extend(tar_scripts(&words));
                }
                None => scripts.extend(tar_scripts(rest)),
            }
        } else if name == "rsync" {
            scripts.extend(rsync_scripts(rest));
        }
    }
    if let Some(alternates) = alternates {
        let main = toks.first().map(|w| prog_name(w)).unwrap_or_default();
        let main_at = tokens.len().saturating_sub(toks.len());
        if let Some((start, end)) = alternate_span(tokens, main_at)
            && !has(QUIET_PROGRAMS, &main)
        {
            let found = alternates_of(tokens, start, end, (main_at, &toks), work)?;
            work.charge_extra(
                found
                    .iter()
                    .flat_map(|(_, alt)| alt)
                    .map(|w| WORD + w.len())
                    .sum(),
            )?;
            alternates.extend(found);
        }
    }
    Ok(Stripped {
        cmd: toks,
        scripts,
        peeled,
    })
}

/// Whether a flag-less wrapper (`nohup`, `setsid`, ...) skipped an option word before index `i`. Rust-only:
/// options known to take no value (`hidden::wrapper_known`) do not count.
fn has_option(toks: &[String], i: usize, name: &str) -> bool {
    toks.iter().take(i.min(toks.len())).skip(1).any(|t| {
        t.starts_with('-') && t.chars().count() > 1 && t != "--" && !wrapper_known(name, t)
    })
}
