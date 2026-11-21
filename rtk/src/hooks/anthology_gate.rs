//! Anthology fork change: the Claude hook never auto-allows past a deny or ask
//! rule (security finding C1).
//!
//! The plugin installs `Bash(rtk *)` as an allow rule, and Claude Code judges the
//! command the hook hands back in `updatedInput`, not the one the agent typed. So
//! upstream's "deny steps aside, ask leaves the decision to the host" is not enough
//! on this install:
//!
//! - `rtk git push`, `rtk bypass git push`, `rtk run -c 'git push'` and
//!   `bash -c 'git push'` are never matched against the user's `Bash(git push:*)`
//!   deny, because the host sees `rtk ...` and `Bash(rtk *)` allows it;
//! - an explicit ask rule (`Bash(kubectl apply:*)`) turns `kubectl apply` into
//!   `rtk kubectl apply`, which `Bash(rtk *)` then allows without a prompt;
//! - `Read(./secret.txt)` deny rules were ignored, so `cat secret.txt` became
//!   `rtk read secret.txt`, which no Read rule can match;
//! - managed settings were never read, so a managed `Bash(curl:*)` deny is lost
//!   once `curl x` becomes `rtk curl x`.
//!
//! What this module does, for `rtk hook claude` and `rtk hook check --agent
//! claude` only (`rtk rewrite` and every other host are untouched):
//!
//! 1. **Deny anywhere.** A command is denied when a Bash deny rule matches it, the
//!    inner command of any `rtk`-prefixed segment, or the script of a shell wrapper
//!    (`sh|bash|zsh|dash|ksh|mksh|fish -c '...'`), recursively (depth cap
//!    [`MAX_DEPTH`]; past it the answer is ask). The Claude hook prints that as an
//!    explicit `permissionDecision: "deny"` rather than saying nothing.
//! 2. **Ask anywhere.** Otherwise a matching ask rule, or an inner command the gate
//!    cannot read (command substitution, a non-POSIX `--shell`, the depth cap),
//!    makes the hook ask itself (`AskOwner::Hook` -> `permissionDecision: "ask"`).
//!    A hook "ask" beats an allow rule; "no decision" would let `Bash(rtk *)` allow
//!    the rewritten text.
//! 3. **`Read()` deny.** `Read(...)` entries of every `permissions.deny` list are
//!    loaded and enforced against the commands rtk rewrites into reads (`cat`,
//!    `head`, `tail`, `grep`, `rg`, `diff`, `wc`, `ls`, `find`, `tree`, and the
//!    `rtk read`-family subcommands). A denied path is a deny; a path the gate
//!    cannot resolve (`$VAR`, a glob, an unknown cwd, `/proc/...`, `/dev/fd/...`)
//!    asks, at top level too (round 6). An input redirect's operand
//!    (`cat < secr?t.txt`) is checked the same way, every glued token of it.
//! 4. **Rule sources.** The project's and user's `settings{,.local}.json` as
//!    before, plus `CLAUDE_PROJECT_DIR` as an additional project root and the
//!    managed settings (`managed-settings.json` and `managed-settings.d/*.json`,
//!    resolved through `user_dirs::managed_settings_dir`). When a managed file sets
//!    `allowManagedPermissionRulesOnly`, rules from the other files are dropped,
//!    as Claude Code ignores them then. An unreadable or unparseable managed file
//!    fails closed like any settings file (`ask *`).
//!
//! 5. **The rewrite is gated too.** When the ordinary decision rewrites a command
//!    (`pnpm exec cat secret.txt` into `rtk pnpm exec cat secret.txt`), the
//!    rewritten text is evaluated as well, in the rtk context the host will judge
//!    it in: a deny is a deny, an ask asks, and an unresolvable path leaves the
//!    command unrewritten.
//!
//! 6. **The plain shape** (round 7). Five review rounds each found a new
//!    spelling, descriptor or option that ran a ruled command or read a ruled file
//!    behind `rtk`. So the gate no longer models shell semantics for what may
//!    pass: whenever any deny, ask or `Read` rule exists, a line that may run rtk
//!    ([`mentions_rtk`]) passes without a prompt only when the whole line has the
//!    plain shape ([`plain`], below); anything else asks (or is denied, when
//!    points 1-3 deny it). A rewrite into a line without the plain shape is not
//!    made: the original goes back unrewritten (an ask stays an ask on it).
//! 7. **Literal backstop** (round 6). Inside [`plain`]: a Bash deny or ask rule's
//!    command text found anywhere in the line asks, quotes and backslashes
//!    removed, whitespace collapsed, case folded where Claude Code folds it
//!    (macOS, Windows), on word boundaries. `rtk ls 'git push'` asks; `rtk ls
//!    gitpush` does not.
//! 8. **The shared parser** (RG1, ADR-023). After the rule scan, the line is read
//!    by `anthology_shell::analyze`, the guard's own lexer and expansion (bounded
//!    by one work budget), and each entry it finds (what runs, past wrappers,
//!    rtk forms, shells' scripts, heredocs and here-strings fed to a shell,
//!    `eval`, substitutions, `find -exec`) meets the rules: a deny rule or a
//!    `Read()`-denied path denies, an ask rule asks, and an analysis cut short
//!    (`incomplete`) asks. In a line that may run rtk an entry the analysis
//!    could not see into (`opaque`) asks too. This pass only adds a deny or an
//!    ask ([`Scanner::scan_shared`]), so the gate is never less strict than the
//!    rule scan alone; and [`plain`] passes a line only when the shared parser
//!    reads it as the same single command.
//!
//! With no deny, ask or `Read` rule anywhere the gate is inert and the hook's
//! output is what it was before.
//!
//! # Scope
//!
//! The gate aims at parity-plus with Claude Code's own rule matching. A Bash deny
//! or ask rule is enforced against the commands the agent types: every rtk form,
//! wrapper, compound, substitution and redirect. A `Read()` rule is enforced
//! against every file the line names or redirects. What an allowed program does
//! with the files, plugins, configs or build steps it loads (`build.rs`, test
//! files, MSBuild targets, the linker, plugins, a kubeconfig `exec`, compose
//! files) is out of scope: deny rules restrict commands, not what programs do.
//! The plain shape lists no program that loads configuration or runs code a flag
//! names; it does not make a listed program safe from its own files.
//!
//! # The plain shape
//!
//! A line has it when all of these hold:
//!
//! - **One simple command, nothing expanded.** Outside quotes only letters, digits,
//!   spaces, tabs and `-_./:,=+%@` stand. So no `;`, `&`, `|`, newline, subshell,
//!   group, redirect of any kind (`<`, `>`, a here-string, a here-document, an fd
//!   duplication), substitution, glob, brace or extglob character, comment, or a
//!   word opening with `=` or `@` (an argument file to rustc, rustdoc and others).
//!   `$`, a backquote, a backslash and `!` are refused outside quotes and inside
//!   double quotes; inside single quotes every printable character is literal in
//!   every POSIX shell and zsh (`-m 'feat!: x'`). zsh's `EXTENDED_GLOB` makes `^`,
//!   `#` and a later `~` glob operators (`c^1./.env` matches `config/.env`), so an
//!   unquoted word holding one asks, with two exceptions: a `commit -m` message,
//!   and a git revision of the strict shape `REF` then any of `~N`, `^N`, `~`, `^`
//!   (or two of those joined by `..` or `...`), `REF` being letters, digits, `_`
//!   and `-` with no `/` or `.` (`HEAD~1`, `main^`, `HEAD~5..HEAD`). A leading `~`
//!   is the home only as `~` or `~/...` with no later `^`, `~` or `#` (from
//!   `$HOME`; an unset, empty or relative `HOME` asks).
//! - **`rtk <sub>` from [`PLAIN`].** The first word is `rtk` (so no assignment,
//!   `cd`, `exec`, `eval`, `source`, launcher or rtk global flag in front), then
//!   one of: `ls`, `tree`, `read`, `wc`, `diff`, `grep`, `rg`, `find`; `git status`,
//!   `log`, `diff`, `show`, `branch`, `blame`, `add`, `commit`, `checkout`,
//!   `switch`, `rev-parse`, `ls-files` (no git global option); `cargo build`,
//!   `test`, `check`, `clippy`, `fmt`.
//! - **Listed options only.** Each sub has its own list of options without a value
//!   and options with one; an unknown option, an abbreviation, or an option off the
//!   list (`git -C`, `git log --output`, `git diff --ext-diff`, `git commit -F`,
//!   `grep -f`/`--pre`, `rg -z`, `find -exec`/`-delete`/`-fprint*`/`-newer`, `cargo
//!   --config`/`-Z`/`+toolchain`/`--target`/`--manifest-path`, `git add -f`) asks.
//!   Any word after cargo's `--` asks (it goes to rustc, rustdoc or the test
//!   binary). An option whose value is optional takes it only glued, as the program
//!   does (`--pretty=x`, `-U5`, `-uno`); a bare one never takes the next word. An
//!   option that takes the next word as its value asks when that word opens with
//!   `-`, so a value never hides an option.
//! - **Literal paths.** Every operand and option value that is not a pattern (grep's
//!   pattern and `-e`, `--include`/`--glob`, find's `-name`/`-path`, libtest's
//!   filters) is resolved lexically against the hook's working directory (`..`
//!   normalised) and through symlinks too. It asks when it is `-` or lies under
//!   `/dev`, `/proc` or `/sys` (case folded on macOS and Windows), and meets the
//!   `Read()` rules: deny is a deny, ask an ask (quoted text is literal: `'$x'` is a
//!   name). git reads its pathspecs as globs, quoted or not, so a git word holding
//!   `*`, `?`, `[` or `\` asks (`rtk git log -p 'secre[t].txt'`). In a git command no word after the subcommand holds a `:` (`REV:path`
//!   is a blob; `--pretty=format:%h` asks too), except a `commit -m` message, which
//!   is no path and is not checked. A strict revision word `x~1`, `x^1` or
//!   `x^1..y` still matches, under `EXTENDED_GLOB`, the names in the working
//!   directory that start with `x` (`secret^1` matches `secret.txt`): each such
//!   name is checked too, and a directory of more than 10 000 entries asks.
//!
//! What is off the list asks: `rtk bypass`/`run`/`proxy`/`err`/`test`/`summary`,
//! kubectl, oc, docker, dotnet, go, prettier, rspec, npm, npx, pnpm, yarn, bun,
//! pip, uv, gh, glab, pytest, ruff, tsc, mypy, curl, and every other rtk
//! subcommand. A line that mentions `rtk` but does not start with it (`grep -rn rtk
//! src`, `nice rtk ls`, `ls; rtk ls`) asks too: Claude Code may split or peel it
//! and match `Bash(rtk *)` on a part.
//!
//! # What the hook cannot see
//!
//! `--settings`, `--disallowedTools`, server-managed settings and the macOS MDM
//! profile (`com.anthropic.claudecode`) never reach a hook; a rule that lives only
//! there is not enforced here (Claude Code still applies it to the text it judges).
//!
//! # Known residuals
//!
//! With a rule in place nothing but the plain shape passes without a prompt, so
//! the rule scan below only decides between a deny and an ask; where it reads a
//! line differently from the shell, the result is an ask, not a pass.
//!
//! - Files a recursive search descends into (`grep -r x .`, `find . -name x`) are
//!   not matched against `Read()` rules: only the paths named on the command line.
//!   A glob value that selects files within it (`rg -g secret.txt`, `grep -r
//!   --include secret.txt x .`) is a pattern, not a path, and is not checked either.
//! - Inside an rtk expansion every command's operand words are checked against
//!   `Read()` deny rules, whatever the command (`rtk sed -n p secret.txt`). What
//!   reads a file without naming it as an operand is not seen: `rtk deps`
//!   (manifests), `find -files0-from FILE`, `rg --pre`, a script or interpreter
//!   that opens a path itself. A short option glued to its value (`grep
//!   -fsecret.txt`) and an `@file` value (`curl -d @secret.txt`) are checked.
//! - Files, links and settings the same command creates (`ln -s . d && cat
//!   d/secret.txt`, `cp -r`, `tar`, a script it writes then runs, an edited
//!   `package.json`): the gate cannot resolve a path that does not exist yet.
//! - What a plain command runs from files: the tests, `build.rs` and proc macros
//!   `cargo test` builds, git hooks, git and cargo settings, `.gitattributes`
//!   filters and diff drivers. Also the environment the session already had
//!   (`GIT_PAGER`, `PAGER`, `RUSTC_WRAPPER`, `PATH`...). Out of scope by the
//!   ruling above.
//! - Plain git commands that print a file's content without naming it: `git show
//!   REV`, `git log -p`, `git diff` print the diffs of every changed file, a
//!   `Read()`-denied one included; so does a blob id another command printed
//!   (`git ls-files -s`, then `git show <blob>`). Claude Code does not apply Read
//!   rules to them either.
//! - What an argument file holds (`@file`, read by rustc, rustdoc and others): a
//!   word opening with `@` asks, but a file a program loads that way on its own
//!   (a cargo config's `rustflags`) is the program's business.
//! - Opaque inside an rtk expansion (the hook asks) when any deny or ask rule of
//!   any kind exists: `eval`, `source`, `.`, `trap`, `alias`; a command word with
//!   `$` or a glob; any shell with no parseable `-c` (stdin, a script file, tcsh
//!   and csh included); any invocation of an interpreter (python, perl, ruby,
//!   node, bun, deno, php, lua, osascript, expect, tclsh, wish, Rscript, R,
//!   julia, pwsh, powershell), a script file included; busybox, toybox, sudo,
//!   doas; `rg --pre`. Programs that run a string (`awk`, `make`, `ed`, `vim`,
//!   `sqlite3`, `sed`) and `./x.sh` are not in the plain shape and ask. Git
//!   aliases and config defined outside the command (`~/.gitconfig`, a
//!   repository's `.git/config`, hooks) are still unread. Git
//!   config set on the command line or in its environment (`-c alias.p=push`,
//!   `GIT_CONFIG_COUNT`, `GIT_PAGER`...) is read when a rule's head starts with
//!   `git`: it asks inside rtk and leaves a top-level command unrewritten.
//! - A launcher the gate does not model (`caffeinate`, `arch`, `ssh host cmd`,
//!   `watch`, `su -c`...) is not peeled. Backstop: the literal backstop (point 7)
//!   asks wherever the text has an rtk command; a rewrite into such a text is
//!   left to the host. A rule with a leading or inner `*` has no head to look for.
//! - A word that expands (`$x`, a glob, a brace, `$'..'`, `~user`) after literal
//!   words a Bash rule can extend (`rtk git $x`, `rtk kubectl $a`) is asked inside
//!   rtk and left unrewritten at top level; with any `Read()` rule, an expanding
//!   operand of any program inside rtk asks.
//! - Shell grammar (`if`, `then`, `while`, `time`, `coproc`, `function`, `name=value`
//!   in any case) is peeled before a rule is matched; `for`, `select`, `case`,
//!   `[[` inside rtk are asked.
//! - When the work budget is spent and any rule exists, the hook asks.
//! - Options between a rule's words (`git -C . push`, `kubectl -n prod apply`): git's
//!   global options are peeled so the rule fires as a deny; for every other tool a
//!   head's words found in order after its command word (before any `--`) make the
//!   hook ask inside rtk and leave a top-level rewrite to the host. It also fires
//!   on `git branch push` and the like.
//! - The command word is read by its basename, a leading `=` dropped, and folded to
//!   lower case on macOS and Windows (`/usr/bin/git`, `=git`, `GIT`).
//! - Inside an expansion every word-suffix of a segment is read as a command of its
//!   own, so `caffeinate X`, `ssh host X`, `exec -a x X` and any launcher the peel
//!   does not model are covered whatever X's quoting. What only a suffix reaches
//!   asks, it never denies; interpreters and shells are asked about in a suffix
//!   unless the program takes data (`grep python src`). A quoted word that reads
//!   like a script (`ssh h 'x=push; git $x'`) is read as one, rules only.
//! - `xargs CMD`, `parallel CMD` and `{}` in a `find -exec*`/`-ok*` payload are read
//!   as followed by a word that expands.
//! - A bare `.` or `source` with nothing after it, and a shell or interpreter
//!   named as an operand (`ruff check .`, `docker exec app python x.py`, `uv run
//!   python x.py`), are not commands: they ask only after a launcher the gate
//!   knows (`caffeinate`, `nice`, `ssh`, `env`, `xargs`, `npm exec`...), or with
//!   inline code (`-c`, `-e`) or stdin. A script a launcher runs (`caffeinate sh
//!   -c '...'`) is read in full; a deny found there is an ask.
//! - rtk's argument parser drops a `--` before the nested command, so `rtk git --
//!   push` is read as `git push`.
//! - When the work budget is spent and any rule exists, the hook asks. Every peel
//!   step and suffix reading is charged to it.
//! - Globs are never expanded against the disk: `rtk git add src/*.rs` asks (no
//!   plain shape), and `cat *.txt` asks when a `Read()` rule exists.
//! - `env` (with its options), `xargs` (with its options) and `find -exec` payloads
//!   are read, at top level too; an `xargs` or `env` option the gate does not know,
//!   or an unterminated `-exec`, is opaque. A top-level `find` with such a payload
//!   is left unrewritten (`Defer`).
//! - A wrapper the peel does not model (`sudo rtk ...`, `command -p rtk ...`,
//!   `exec -a x rtk ...`) is not expanded: the hook sees the command word `sudo`,
//!   `command` or `exec`.
//! - `rtk <sub>` for a subcommand that runs a different program than its name
//!   (`rtk vitest`, `rtk lint`) is matched as `<sub> <rest>`, as the brief says.

use super::constants::PRE_TOOL_USE_KEY;
use super::decision::{AskOwner, HookDecision};
use super::permissions::{self, PermissionVerdict};
use crate::core::user_dirs;
use crate::core::utils::strip_leading_bom;
use crate::discover::lexer::{self, TokenKind, resolve_word_text};
use crate::discover::registry;
use anthology_shell::{Analysis, Entry as ShellEntry, analyze};
use serde_json::{Value, json};
use std::borrow::Cow;
use std::path::{Path, PathBuf};

/// How many rtk or wrapper expansions deep the gate reads before it gives up and
/// asks.
const MAX_DEPTH: usize = 4;
/// Units of work (about sixteen bytes of text each) one command may cost, so a
/// crafted command cannot make the hook slow. Past it the answer is ask.
const WORK_BUDGET: usize = 1_000_000;
/// The least a segment costs: 4000 segments use the whole budget.
const SEGMENT_COST: usize = 250;
/// The shells whose `-c` script the gate reads.
const SHELLS: &[&str] = &[
    "sh", "bash", "zsh", "dash", "ksh", "mksh", "fish", "tcsh", "csh", "ash",
];
/// Programs that run code the gate cannot read from their words, whatever the
/// flags: a script file is as opaque as `-c`. Version suffixes (`python3.11`) are
/// stripped before the comparison.
const INTERPRETERS: &[&str] = &[
    "python",
    "pypy",
    "perl",
    "ruby",
    "node",
    "nodejs",
    "bun",
    "deno",
    "php",
    "lua",
    "luajit",
    "osascript",
    "expect",
    "tclsh",
    "wish",
    "rscript",
    "r",
    "julia",
    "pwsh",
    "powershell",
];
/// The shells whose grammar the lexer models: anything else is opaque.
/// Programs that run the rest of their arguments as a command. A shell or
/// interpreter that follows one of them is a command, not an operand.
const LAUNCHERS: &[&str] = &[
    "caffeinate",
    "nice",
    "ssh",
    "exec",
    "command",
    "builtin",
    "sudo",
    "doas",
    "su",
    "runuser",
    "env",
    "xargs",
    "parallel",
    "watch",
    "timeout",
    "gtimeout",
    "script",
    "arch",
    "xcrun",
    "sandbox-exec",
    "flock",
    "setsid",
    "unbuffer",
    "ionice",
    "chrt",
    "taskset",
    "nohup",
    "stdbuf",
    "busybox",
    "toybox",
    "find",
    "npm",
    "pnpm",
    "npx",
    "bunx",
    "bun",
    "yarn",
];
/// Interpreter options that carry code inline.
const INLINE_CODE: &[&str] = &[
    "-c",
    "-e",
    "-E",
    "-p",
    "-r",
    "-pe",
    "-ne",
    "-pi",
    "-ane",
    "--eval",
    "--print",
    "--command",
    "-Command",
    "-command",
    "-EncodedCommand",
];
const POSIX_SHELLS: &[&str] = &["sh", "bash", "zsh", "dash", "ksh", "mksh"];
/// The commands rtk rewrites into reads.
const READ_COMMANDS: &[&str] = &[
    "cat", "head", "tail", "grep", "rg", "diff", "wc", "ls", "find", "tree",
];
/// `grep`/`rg` options that take a separate value that is not a path.
const SEARCH_VALUE_OPTIONS: &[&str] = &[
    "-e",
    "--regexp",
    "-g",
    "--glob",
    "--iglob",
    "-t",
    "--type",
    "-T",
    "--type-not",
    "-m",
    "--max-count",
    "-A",
    "-B",
    "-C",
    "--after-context",
    "--before-context",
    "--context",
    "--include",
    "--exclude",
    "--exclude-dir",
    "--max-depth",
    "-j",
    "--threads",
    "-M",
    "--max-columns",
];

/// The reason the Claude hook gives for an explicit deny.
const DENY_REASON: &str = "RTK: a deny rule in the user's, project's or managed Claude Code \
settings matches the command or a command it runs.";

/// What the gate says about a command, ahead of the ordinary rewrite decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Gate {
    /// Nothing to add: the ordinary decision stands.
    Pass,
    /// An ask rule, or something the gate cannot read, applies: the hook asks.
    Ask,
    /// A path the gate cannot resolve at top level: no rewrite, the host judges
    /// the original.
    Defer,
    /// A deny rule applies somewhere in the command.
    Deny,
}

/// The `PreToolUse` response for an explicit deny: no `updatedInput`, so nothing
/// is rewritten and nothing runs.
pub(crate) fn deny_output() -> Value {
    json!({
        "hookSpecificOutput": {
            "hookEventName": PRE_TOOL_USE_KEY,
            "permissionDecision": "deny",
            "permissionDecisionReason": DENY_REASON,
        }
    })
}

/// The `PreToolUse` response for a tool call the hook could not read (stdin over
/// the size limit, or not JSON): ask, so that an allow rule never decides on an
/// unread command. No `updatedInput`.
pub(crate) fn ask_output() -> Value {
    json!({
        "hookSpecificOutput": {
            "hookEventName": PRE_TOOL_USE_KEY,
            "permissionDecision": "ask",
            "permissionDecisionReason": "RTK: the hook could not read this tool call (input over \
    the size limit, or not valid JSON), so it asks instead of letting an allow rule decide.",
        }
    })
}

/// Fold the gate's answer into the ordinary decision: an ask makes the hook ask
/// itself, with the rewrite when there is one and the original command when not.
///
/// Otherwise, a rewrite the host would decide on (no decision, or an allow) of a
/// command whose text holds the words of a deny or ask rule anywhere (`git log
/// --grep='git push'`, `ssh h git push`) is left unrewritten: the host judges the
/// original exactly as it would without rtk.
pub(crate) fn apply(
    gate: Gate,
    cmd: &str,
    decision: HookDecision,
    rules: &GateRules,
) -> HookDecision {
    if gate != Gate::Ask {
        if matches!(
            decision,
            HookDecision::AllowRewrite(_) | HookDecision::AskRewrite(_, AskOwner::Host)
        ) && rules.embeds_head(cmd)
        {
            return HookDecision::Defer;
        }
        return decision;
    }
    match decision {
        HookDecision::AllowRewrite(rewritten) | HookDecision::AskRewrite(rewritten, _) => {
            HookDecision::AskRewrite(rewritten, AskOwner::Hook)
        }
        HookDecision::Defer => HookDecision::AskRewrite(cmd.to_string(), AskOwner::Hook),
        HookDecision::Deny => HookDecision::Deny,
    }
}

// ---------------------------------------------------------------------------
// Rule sources
// ---------------------------------------------------------------------------

/// The settings files the Claude hook reads, split by who owns them.
pub(crate) struct SettingsFiles {
    /// `managed-settings.json` and the sorted `managed-settings.d/*.json`.
    pub(crate) managed: Vec<PathBuf>,
    /// Project, `CLAUDE_PROJECT_DIR` and user settings.
    pub(crate) other: Vec<PathBuf>,
    /// The project roots a `Read(/p)` rule anchors to.
    pub(crate) project_roots: Vec<PathBuf>,
}

impl SettingsFiles {
    /// `other` holds the files the hook has always read; this adds the project
    /// `CLAUDE_PROJECT_DIR` names when it is absolute and not the root already
    /// found, and the managed files under `managed_dir`.
    pub(crate) fn assemble(
        mut other: Vec<PathBuf>,
        project_root: Option<PathBuf>,
        env_project_dir: Option<PathBuf>,
        managed_dir: &Path,
    ) -> Self {
        let mut project_roots: Vec<PathBuf> = project_root.iter().cloned().collect();
        if let Some(dir) = env_project_dir
            && dir.is_absolute()
            && project_root.as_ref() != Some(&dir)
        {
            let claude = dir.join(super::constants::CLAUDE_DIR);
            other.push(claude.join(super::constants::SETTINGS_JSON));
            other.push(claude.join(super::constants::SETTINGS_LOCAL_JSON));
            project_roots.push(dir);
        }
        Self {
            managed: managed_settings_paths(managed_dir),
            other,
            project_roots,
        }
    }
}

/// `CLAUDE_PROJECT_DIR`, which Claude Code sets for a hook.
pub(crate) fn env_project_dir() -> Option<PathBuf> {
    user_dirs::env_path("CLAUDE_PROJECT_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// `managed-settings.json`, then each `*.json` in the sibling
/// `managed-settings.d/`, sorted by name.
///
/// A drop-in directory that exists but cannot be listed is returned as a path of
/// its own: reading a directory fails, which the loader treats as an unreadable
/// settings file and fails closed on.
pub(crate) fn managed_settings_paths(dir: &Path) -> Vec<PathBuf> {
    let mut paths = vec![dir.join("managed-settings.json")];
    let drop_in = dir.join("managed-settings.d");
    match std::fs::read_dir(&drop_in) {
        Ok(entries) => {
            let mut files: Vec<PathBuf> = entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
                .collect();
            files.sort();
            paths.extend(files);
        }
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) => {}
        Err(_) => paths.push(drop_in),
    }
    paths
}

/// Whether any managed file sets `allowManagedPermissionRulesOnly`. A file that
/// cannot be read or parsed says nothing here; it already fails the rule scan
/// closed.
pub(crate) fn managed_rules_only(managed: &[PathBuf]) -> bool {
    managed.iter().any(|path| {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|content| serde_json::from_str::<Value>(strip_leading_bom(&content)).ok())
            .and_then(|json| {
                json.get("allowManagedPermissionRulesOnly")
                    .and_then(Value::as_bool)
            })
            == Some(true)
    })
}

/// Append the `Read(...)` entries of a `permissions.deny` or `permissions.ask` array to `target`: the
/// path pattern for `Read(pattern)`, and `**` for a bare `Read`, which denies
/// every read.
pub(crate) fn append_read_rules(rules_value: Option<&Value>, target: &mut Vec<String>) {
    let Some(rules) = rules_value.and_then(Value::as_array) else {
        return;
    };
    for rule in rules.iter().filter_map(Value::as_str) {
        let rule = rule.trim();
        if rule == "Read" {
            target.push("**".to_string());
        } else if let Some(inner) = rule
            .strip_prefix("Read(")
            .and_then(|rest| rest.strip_suffix(')'))
        {
            target.push(inner.trim().to_string());
        }
    }
}

/// Everything the gate needs from the settings scan.
pub(crate) struct GateRules {
    deny: Vec<String>,
    ask: Vec<String>,
    read_deny: Vec<String>,
    read_ask: Vec<String>,
    /// The user's `transparent_prefixes`, normalized.
    prefixes: Vec<String>,
    project_roots: Vec<PathBuf>,
    /// The literal leading words of each Bash deny and ask rule, the command word
    /// normalized (see [`norm_command`]).
    heads: Vec<Vec<String>>,
    /// The same heads cut into the words [`GateRules::embeds_head`] reads (split at
    /// `=`, `:`, `!` and the like).
    embed_heads: Vec<Vec<String>>,
    /// Each Bash deny and ask rule's literal command text (`git push` for `git
    /// push:*`), words joined by one space, as written in the rule.
    literal_heads: Vec<String>,
    /// Some Bash rule's head starts with `git`.
    git_rules: bool,
    /// Some Bash rule has a `*` before its end, or no literal head: any command
    /// may extend it.
    wildcard: bool,
}

/// The words a rule pattern spells before its trailing `*` (`git push:*`,
/// `git push *`, `git push*` all give `git push`), or `None` for a pattern with a
/// leading or inner `*` or an empty head, which any command may extend.
fn rule_head(pattern: &str) -> Option<Vec<String>> {
    let pattern = pattern.trim();
    let head = match pattern.find('*') {
        None => pattern,
        Some(at) if pattern[at..].chars().all(|c| c == '*') => &pattern[..at],
        Some(_) => return None,
    };
    let words: Vec<String> = head
        .trim_end_matches(|c: char| c == ':' || c.is_whitespace())
        .split_whitespace()
        .map(str::to_string)
        .collect();
    (!words.is_empty()).then_some(words)
}

/// A text as the literal backstop compares it: quotes and backslashes removed,
/// every run of whitespace one space, folded to lower case where the file system
/// folds case.
fn normalize_literal(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for c in text.chars() {
        if matches!(c, '\'' | '"' | '\\') {
            continue;
        }
        if c.is_whitespace() {
            space = !out.is_empty();
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        out.push(c);
    }
    if cfg!(any(target_os = "macos", windows)) {
        out.to_lowercase()
    } else {
        out
    }
}

/// Where words of a command text break for [`GateRules::embeds_head`].
fn is_word_break(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            ';' | '&' | '|' | '(' | ')' | '{' | '}' | '`' | '=' | '!' | ':' | '<' | '>'
        )
}

/// How a command's words after its command word line up with a rule head.
#[derive(PartialEq)]
enum Walk {
    /// A positional word that is not the head's next word, or a `--`, comes first.
    Killed,
    /// Only options, their values and the head's words so far.
    Partial,
    /// Every later word of the head was found.
    Complete,
}

/// Walk `words` (after the command word) against `head[1..]`: between the head's
/// words only options and their values may stand. A word after an option is read
/// as the head's next word when it is one, else as the option's value, so
/// `git --bare push` and `git -C . push` both complete a `git push` head while
/// `git stash push` and `git log --grep push` do not.
fn walk_head(head: &[String], words: &[String]) -> Walk {
    let mut next = 1;
    let mut after_option = false;
    for word in words {
        if word == "--" {
            return Walk::Killed;
        }
        if next < head.len() && *word == head[next] {
            next += 1;
            after_option = false;
            if next == head.len() {
                return Walk::Complete;
            }
            continue;
        }
        if after_option {
            after_option = false;
            continue;
        }
        if word.starts_with('-') {
            after_option = !word.contains('=');
            continue;
        }
        return Walk::Killed;
    }
    Walk::Partial
}

/// Whether all of `head[1..]` follows in `words` with only options between.
fn later_words_follow(head: &[String], words: &[String]) -> bool {
    walk_head(head, words) == Walk::Complete
}

/// Whether `prefix` (whose command word already equals `head[0]`) can still turn
/// into `head`.
fn opts_then_rest(head: &[String], prefix: &[String]) -> bool {
    walk_head(head, &prefix[1..]) != Walk::Killed
}

impl GateRules {
    pub(crate) fn new(
        deny: Vec<String>,
        ask: Vec<String>,
        read_deny: Vec<String>,
        read_ask: Vec<String>,
        transparent_prefixes: &[String],
        project_roots: Vec<PathBuf>,
    ) -> Self {
        let mut heads = Vec::new();
        let mut embed_heads = Vec::new();
        let mut literal_heads = Vec::new();
        let mut wildcard = false;
        for pattern in deny.iter().chain(&ask) {
            match rule_head(pattern) {
                Some(mut head) => {
                    let literal = normalize_literal(&head.join(" "));
                    if !literal.is_empty() {
                        literal_heads.push(literal);
                    }
                    head[0] = norm_command(&head[0]);
                    let cut: Vec<String> = head
                        .join(" ")
                        .chars()
                        .filter(|c| !matches!(c, '\'' | '"' | '\\'))
                        .collect::<String>()
                        .split(is_word_break)
                        .filter(|word| !word.is_empty())
                        .map(str::to_string)
                        .collect();
                    if !cut.is_empty() {
                        embed_heads.push(cut);
                    }
                    heads.push(head);
                }
                None => wildcard = true,
            }
        }
        let git_rules = heads.iter().any(|head| head[0] == "git");
        Self {
            deny,
            ask,
            read_deny,
            read_ask,
            prefixes: registry::normalize_transparent_prefixes(transparent_prefixes),
            project_roots,
            heads,
            embed_heads,
            literal_heads,
            git_rules,
            wildcard,
        }
    }

    /// Whether a command whose literal leading words are `prefix` (followed by
    /// something that expands at run time) could turn into one a Bash rule names:
    /// the rule's head and the prefix agree as far as both go, or the prefix opens
    /// with the head's command word and only options (and their values) stand
    /// between it and the head's later words (`git -C . $x`).
    fn extendable(&self, prefix: &[String]) -> bool {
        if self.wildcard {
            return true;
        }
        let first = prefix.first().map(|word| norm_command(word));
        self.heads.iter().any(|head| {
            let strict = head.iter().zip(prefix).enumerate().all(|(at, (a, b))| {
                if at == 0 {
                    first.as_deref() == Some(a.as_str())
                } else {
                    a == b
                }
            });
            strict || (first.as_deref() == Some(head[0].as_str()) && opts_then_rest(head, prefix))
        })
    }

    /// Whether the unquoted `words` of a command hold a head's later words in order
    /// after its command word, with anything between and before any `--`:
    /// `git -C x push`, `kubectl -n prod apply`.
    fn subsequence_hit(&self, words: &[String]) -> bool {
        let Some(first) = words.first() else {
            return false;
        };
        let first = norm_command(first);
        self.heads
            .iter()
            .any(|head| head.len() > 1 && head[0] == first && later_words_follow(head, &words[1..]))
    }

    /// Whether some multi-word head's last word is one of `words` (after the first).
    fn tail_word_in(&self, words: &[String]) -> bool {
        self.heads.iter().any(|head| {
            head.len() > 1
                && words
                    .iter()
                    .skip(1)
                    .any(|word| word == &head[head.len() - 1])
        })
    }

    /// Whether `text` holds the words of a (non-wildcard) Bash rule's head, anywhere:
    /// consecutive after the quotes are removed the way the shell removes them
    /// (`g''it push`, `caffeinate git push`, `ssh h git push`, `-c 'alias.p=!git
    /// push'`), or with options between them (`ssh h git -C . push`).
    fn embeds_head(&self, text: &str) -> bool {
        if self.heads.is_empty() {
            return false;
        }
        let joined: String = text
            .chars()
            .filter(|c| !matches!(c, '\'' | '"' | '\\'))
            .collect();
        let words: Vec<&str> = joined
            .split(is_word_break)
            .filter(|word| !word.is_empty())
            .collect();
        let consecutive = self.embed_heads.iter().any(|head| {
            words.windows(head.len()).any(|window| {
                norm_command(window[0]) == head[0]
                    && window[1..]
                        .iter()
                        .zip(&head[1..])
                        .all(|(a, b)| *a == b.as_str())
            })
        });
        consecutive || self.option_split_in(text)
    }

    /// The subsequence test from every word of every segment of `text`.
    fn option_split_in(&self, text: &str) -> bool {
        let mut work = 0usize;
        for segment in lexer::split_for_wrappers(text) {
            let segment = without_redirects(segment);
            let (words, _) = lexer::words_and_spans(&segment);
            let words: Vec<String> = words
                .iter()
                .take(256)
                .map(|word| resolve_word_text(word))
                .collect();
            for start in 0..words.len() {
                work += 1;
                let first = norm_command(&words[start]);
                for head in self.heads.iter().filter(|h| h.len() > 1 && h[0] == first) {
                    work += words.len() - start;
                    if later_words_follow(head, &words[start + 1..]) {
                        return true;
                    }
                }
                if work > 400_000 {
                    // Too much text to read it all: leave it to the host.
                    return true;
                }
            }
        }
        false
    }

    /// Whether some Bash deny or ask rule's literal command text appears anywhere in
    /// `text` once quotes and backslashes are removed and whitespace is collapsed
    /// (`execSync("git push")`, a here-string, a `data:` URL), between word
    /// boundaries. Case as the file system folds it (see [`norm_command`]).
    fn literal_in(&self, text: &str) -> bool {
        if self.literal_heads.is_empty() {
            return false;
        }
        let text = normalize_literal(text);
        let ident = |c: char| c.is_alphanumeric() || c == '_';
        self.literal_heads.iter().any(|head| {
            text.match_indices(head.as_str()).any(|(at, _)| {
                let before = text[..at].chars().next_back().is_none_or(|c| !ident(c));
                let after = text[at + head.len()..]
                    .chars()
                    .next()
                    .is_none_or(|c| !ident(c));
                before && after
            })
        })
    }

    /// Whether any rule exists for the gate to enforce.
    pub(crate) fn active(&self) -> bool {
        !(self.deny.is_empty()
            && self.ask.is_empty()
            && self.read_deny.is_empty()
            && self.read_ask.is_empty())
    }
}

// ---------------------------------------------------------------------------
// The gate
// ---------------------------------------------------------------------------

/// What the gate says about `cmd`. `verdict` is the ordinary permission verdict:
/// a deny rule inside a shell wrapper's script (`Defer`) is a deny here.
pub(crate) fn evaluate(
    cmd: &str,
    rules: &GateRules,
    verdict: PermissionVerdict,
    cwd: Option<&str>,
) -> Gate {
    if matches!(verdict, PermissionVerdict::Deny | PermissionVerdict::Defer) {
        return Gate::Deny;
    }
    if !rules.active() {
        return Gate::Pass;
    }
    let initial = cwd
        .filter(|cwd| !cwd.is_empty())
        .map(PathBuf::from)
        .or_else(|| user_dirs::current_dir().ok());
    let mut scanner = Scanner::new(rules, initial.as_deref(), cmd);
    let mut cwds = Cwds::new(initial.clone());
    let ctx = Ctx {
        inner: false,
        in_rtk: false,
        payload: false,
        soft: false,
        demote: false,
        open: false,
    };
    if scanner.scan_text(cmd, 0, ctx, &mut cwds).is_err()
        || scanner.scan_shared(cmd, initial.as_deref()).is_err()
    {
        return Gate::Deny;
    }
    if scanner.exhausted {
        // Part of the command was never read, and a rule exists: ask. A command
        // that long costs the user one prompt.
        scanner.ask = true;
    }
    if scanner.ask {
        Gate::Ask
    } else if scanner.defer {
        Gate::Defer
    } else {
        Gate::Pass
    }
}

/// A deny rule matched.
struct Denied;

/// Where a text sits in the expansion tree.
#[derive(Clone, Copy)]
struct Ctx {
    /// Inside an rtk or wrapper expansion: an unresolvable path asks rather than
    /// defers.
    inner: bool,
    /// Reached through an rtk-prefixed segment: a substitution is opaque, because
    /// the host will not rewrite it and `Bash(rtk *)` would allow it unseen.
    in_rtk: bool,
    /// Inside the payload of an exec action (`find -exec`, `xargs`, `env`): what
    /// the gate cannot read there is deferred when the host would have rewritten it.
    payload: bool,
    /// Reached only by guessing that something runs it (a quoted word that reads
    /// like a script): a deny found there asks, and only the rules are matched.
    soft: bool,
    /// Reached only through a launcher's argument list: a deny found there asks.
    demote: bool,
    /// The text is a command whose last arguments arrive at run time (`xargs CMD`,
    /// `parallel CMD`): it is read as followed by a word that expands.
    open: bool,
}

/// One more text to scan, and how it was reached.
#[derive(PartialEq)]
struct Expansion {
    text: String,
    via_rtk: bool,
    payload: bool,
    /// See [`Ctx::soft`].
    soft: bool,
    /// See [`Ctx::demote`].
    demote: bool,
    /// See [`Ctx::open`].
    open: bool,
}

/// Queue an expansion once: the first spelling (a hard one, read before the soft
/// ones) wins.
fn push_expansion(expansions: &mut Vec<Expansion>, new: Expansion) {
    if !expansions.iter().any(|old| {
        old.text == new.text
            && old.via_rtk == new.via_rtk
            && old.payload == new.payload
            && old.open == new.open
    }) {
        expansions.push(new);
    }
}

/// What the command itself can change under the gate's feet.
#[derive(Clone, Copy, Default)]
struct EnvFlags {
    /// `HOME` or `CDPATH` is set, exported or unset in the command: `~` and a bare
    /// `cd` mean something the gate cannot know.
    home: bool,
    /// `CDPATH` is set, in the hook's environment or by the command: a relative
    /// `cd` operand may land elsewhere.
    cdpath: bool,
}

/// A segment form with its words, read once.
struct Parsed<'a> {
    form: &'a str,
    words: Vec<&'a str>,
    spans: Vec<usize>,
    /// Every word, unquoted.
    resolved: Vec<String>,
    /// The first word, unquoted.
    first: String,
    /// The command word as a rule sees it: its basename, a leading `=` (zsh's
    /// `=git`) dropped, folded to lower case where the file system folds.
    name: String,
    /// The form with every word unquoted and joined by single spaces: what a rule
    /// that names `git push` has to match when the text says `git 'push'`.
    unquoted: String,
}

impl<'a> Parsed<'a> {
    fn new(form: &'a str) -> Self {
        let (words, spans) = lexer::words_and_spans(form);
        let resolved: Vec<String> = words.iter().map(|word| resolve_word_text(word)).collect();
        let first = resolved.first().cloned().unwrap_or_default();
        let name = norm_command(&first);
        let unquoted = resolved.join(" ");
        Self {
            form,
            words,
            spans,
            resolved,
            first,
            name,
            unquoted,
        }
    }

    /// The normalized command word of the reading that starts at word `from`.
    fn name_at(&self, from: usize) -> String {
        if from == 0 {
            self.name.clone()
        } else {
            norm_command(&self.resolved[from])
        }
    }

    /// Every way a rule may name the command that starts at word `from`: as
    /// written, unquoted, with the command word normalized, and past git's global
    /// options.
    fn readings_at(&self, from: usize) -> Vec<Cow<'_, str>> {
        let mut out: Vec<Cow<str>> = Vec::new();
        let Some(&at) = self.spans.get(from) else {
            return out;
        };
        out.push(Cow::Borrowed(&self.form[at..]));
        if from == 0 {
            if self.unquoted != self.form {
                out.push(Cow::Borrowed(self.unquoted.as_str()));
            }
        } else {
            out.push(Cow::Owned(self.resolved[from..].join(" ")));
        }
        let name = self.name_at(from);
        let tail = |k: usize| self.resolved.get(k..).unwrap_or_default().join(" ");
        if name != self.resolved[from] {
            out.push(Cow::Owned(join_command(&name, &tail(from + 1))));
        }
        if name == "git" {
            let sub = git_subcommand_index(&self.resolved, &self.words, from);
            if sub > from + 1 && sub < self.resolved.len() {
                out.push(Cow::Owned(join_command("git", &tail(sub))));
            }
        }
        // A dashed builtin from git's exec path (`/usr/lib/git-core/git-push`) is
        // `git push`.
        if let Some(sub) = name.strip_prefix("git-")
            && !sub.is_empty()
        {
            out.push(Cow::Owned(join_command(
                &format!("git {sub}"),
                &tail(from + 1),
            )));
        }
        out
    }
}

/// A command word as a deny or ask rule sees it: the basename, a leading `=`
/// dropped (zsh expands `=git` to the path of `git`), and lower case where the file
/// system is case-insensitive (`GIT push` runs `git`).
fn norm_command(word: &str) -> String {
    let word = word.strip_prefix('=').unwrap_or(word);
    let base = basename(word);
    if cfg!(any(target_os = "macos", windows)) {
        base.to_lowercase()
    } else {
        base.to_string()
    }
}

/// The index of the first word after git's global options, starting from the
/// word after `from` (`git -C x -c k=v push` -> the index of `push`). A `--` in
/// that position is skipped too: rtk's argument parser drops it, so `rtk git --
/// push` runs `git push`.
fn git_subcommand_index(resolved: &[String], raw: &[&str], from: usize) -> usize {
    git_globals(resolved, raw, from, &mut Vec::new())
}

/// [`git_subcommand_index`], also collecting the (lower-cased) keys of `-c` and
/// `--config-env` options on the way.
fn git_globals(resolved: &[String], raw: &[&str], from: usize, keys: &mut Vec<String>) -> usize {
    const WITH_VALUE: &[&str] = &[
        "-C",
        "--git-dir",
        "--work-tree",
        "--namespace",
        "--super-prefix",
    ];
    const FLAGS: &[&str] = &[
        "-p",
        "-P",
        "--paginate",
        "--no-pager",
        "--bare",
        "--no-replace-objects",
        "--literal-pathspecs",
        "--glob-pathspecs",
        "--noglob-pathspecs",
        "--icase-pathspecs",
        "--no-optional-locks",
        "--exec-path",
        "--no-advice",
    ];
    const WITH_EQUALS: &[&str] = &[
        "--git-dir=",
        "--work-tree=",
        "--namespace=",
        "--exec-path=",
        "--super-prefix=",
    ];
    let key_of = |at: usize, text: &str| {
        // A key that is itself computed is as bad as one that names a program.
        if raw.get(at).is_some_and(|word| is_expanding(word)) {
            "alias.?".to_string()
        } else {
            text.split('=').next().unwrap_or("").to_lowercase()
        }
    };
    let mut i = from + 1;
    while let Some(word) = resolved.get(i) {
        if is_expanding(raw[i]) {
            break;
        }
        let word = word.as_str();
        if word == "--" {
            i += 1;
        } else if word == "-c" || word == "--config-env" {
            if let Some(value) = resolved.get(i + 1) {
                keys.push(key_of(i + 1, value));
            }
            i += 2;
        } else if let Some(value) = word.strip_prefix("--config-env=") {
            keys.push(key_of(i, value));
            i += 1;
        } else if WITH_VALUE.contains(&word) {
            i += 2;
        } else if FLAGS.contains(&word) || WITH_EQUALS.iter().any(|opt| word.starts_with(opt)) {
            i += 1;
        } else {
            break;
        }
    }
    i.min(resolved.len())
}

/// A git config key that names a program to run (an alias, a pager, a helper...).
fn names_a_program(key: &str) -> bool {
    key.starts_with("alias.")
        || key.starts_with("protocol.")
        || matches!(
            key,
            "core.pager"
                | "core.editor"
                | "core.sshcommand"
                | "core.fsmonitor"
                | "core.hookspath"
                | "core.askpass"
                | "diff.external"
                | "credential.helper"
                | "sequence.editor"
                | "gpg.program"
                | "uploadpack.packobjectshook"
        )
        || [
            ".command",
            ".cmd",
            ".textconv",
            ".driver",
            ".helper",
            ".program",
            ".pager",
            ".editor",
            ".sshcommand",
        ]
        .iter()
        .any(|suffix| key.ends_with(suffix))
}

/// An environment variable that makes git run a program, or read config the
/// command line does not show.
fn is_git_exec_env(name: &str) -> bool {
    name.starts_with("GIT_CONFIG_KEY_")
        || name.starts_with("GIT_CONFIG_VALUE_")
        || matches!(
            name,
            "GIT_CONFIG_COUNT"
                | "GIT_CONFIG_PARAMETERS"
                | "GIT_CONFIG_GLOBAL"
                | "GIT_CONFIG_SYSTEM"
                | "GIT_CONFIG"
                | "GIT_EXTERNAL_DIFF"
                | "GIT_SSH_COMMAND"
                | "GIT_SSH"
                | "GIT_PAGER"
                | "GIT_EDITOR"
                | "GIT_SEQUENCE_EDITOR"
                | "GIT_ASKPASS"
                | "GIT_PROXY_COMMAND"
                | "GIT_EXEC_PATH"
        )
}

/// Whether a segment's words make git run something its command line does not
/// name: a `-c`/`--config-env` that defines an alias or a program, or an
/// assignment of an environment variable that does the same.
fn git_config_exec(resolved: &[String], raw: &[&str]) -> bool {
    for (at, word) in resolved.iter().enumerate() {
        if let Some((name, _)) = word.split_once('=')
            && is_assignment(word)
            && is_git_exec_env(name)
        {
            return true;
        }
        if norm_command(word) == "git" {
            let mut keys = Vec::new();
            git_globals(resolved, raw, at, &mut keys);
            if keys.iter().any(|key| names_a_program(key)) {
                return true;
            }
        }
    }
    false
}

/// Programs whose operands are data (names, patterns, refs), not commands.
fn operands_are_data(name: &str) -> bool {
    READ_COMMANDS.contains(&name) || matches!(name, "git" | "echo" | "printf")
}

struct Scanner<'a> {
    rules: &'a GateRules,
    reads: Option<ReadMatcher>,
    read_asks: Option<ReadMatcher>,
    env: EnvFlags,
    /// Any Bash deny or ask rule exists.
    bash_rules: bool,
    ask: bool,
    defer: bool,
    /// The work budget ran out: part of the command was not read.
    exhausted: bool,
    /// An rtk, wrapper or payload expansion was met.
    expanded: bool,
    budget: usize,
}

impl<'a> Scanner<'a> {
    fn new(rules: &'a GateRules, cwd: Option<&Path>, cmd: &str) -> Self {
        let (home_changed, cdpath_changed) = command_env_flags(cmd);
        let cdpath_set = cdpath_changed
            || crate::core::user_env::var("CDPATH").is_some_and(|value| !value.is_empty());
        let env = EnvFlags {
            home: home_changed || cdpath_set,
            cdpath: cdpath_set,
        };
        let matcher = |patterns: &[String]| {
            (!patterns.is_empty()).then(|| {
                ReadMatcher::new(
                    patterns,
                    user_dirs::home(),
                    &rules.project_roots,
                    cwd,
                    cfg!(any(target_os = "macos", windows)),
                )
                .with_unstable_home(env.home)
            })
        };
        Self {
            rules,
            reads: matcher(&rules.read_deny),
            read_asks: matcher(&rules.read_ask),
            env,
            bash_rules: !(rules.deny.is_empty() && rules.ask.is_empty()),
            ask: false,
            defer: false,
            exhausted: false,
            expanded: false,
            budget: WORK_BUDGET,
        }
    }

    /// Whether any Read rule is in play.
    fn reading(&self) -> bool {
        self.reads.is_some() || self.read_asks.is_some()
    }

    /// Spend `cost` units of work. `false` once the budget is gone: the caller
    /// stops reading and the answer degrades to an ask (or a defer, see
    /// [`evaluate`]).
    fn spend(&mut self, cost: usize) -> bool {
        if self.exhausted {
            return false;
        }
        if self.budget < cost {
            self.exhausted = true;
            self.budget = 0;
            return false;
        }
        self.budget -= cost;
        true
    }

    /// Something the gate cannot read was met: inside an rtk expansion the hook
    /// asks, inside the payload of a top-level exec action the command is left to
    /// the host unrewritten.
    fn opaque(&mut self, ctx: Ctx) {
        if ctx.in_rtk {
            self.ask = true;
        } else if ctx.payload {
            self.defer = true;
        }
    }

    fn scan_text(
        &mut self,
        text: &str,
        depth: usize,
        ctx: Ctx,
        cwds: &mut Cwds,
    ) -> Result<(), Denied> {
        // The segmenter cuts `rtk git $(echo push)` at the `(`, so a substitution
        // is looked for in the whole text. Inside an rtk expansion it is opaque
        // outright; at top level it is opaque to the rtk segments of the text.
        let substitution =
            lexer::contains_substitution(text) || lexer::ansi_c_quote_defeats_lexer(text);
        if ctx.in_rtk && !ctx.soft && substitution {
            self.ask = true;
        }
        // Inside rtk, a rule's own words anywhere in the text (a quoted script, an
        // argument of a program that runs its arguments) ask: the precise match
        // above would have denied what it could read.
        if ctx.in_rtk
            && !ctx.soft
            && self.bash_rules
            && self.spend(1 + text.len() / 16)
            && self.rules.embeds_head(text)
        {
            self.ask = true;
        }
        if self.reading() && !ctx.soft {
            self.check_input_redirects(text, ctx, cwds)?;
        }
        // A redirect can stand anywhere among a command's words (`git >/dev/null
        // push` runs `git push`): each segment is read with its redirects left out.
        for segment in lexer::split_for_wrappers(text) {
            let segment = without_redirects(segment);
            self.scan_segment(&segment, depth, ctx, substitution, cwds)?;
        }
        Ok(())
    }

    fn scan_segment(
        &mut self,
        segment: &str,
        depth: usize,
        ctx: Ctx,
        substitution: bool,
        cwds: &mut Cwds,
    ) -> Result<(), Denied> {
        let segment = segment.trim();
        if segment.is_empty() {
            return Ok(());
        }
        if !self.spend(SEGMENT_COST + segment.len() / 8) {
            return Ok(());
        }

        // Each segment read every way a rule may name it, as the ordinary gate
        // does: as written, without the grammar a split leaves behind, then
        // past each assignment, wrapper and piece of shell grammar in turn.
        let residue = permissions::strip_grammar_residue(segment);
        let Some(forms) = self.segment_forms(segment, residue) else {
            return Ok(());
        };
        if !self.spend(forms.len() * (1 + segment.len() / 16)) {
            return Ok(());
        }
        let parsed: Vec<Parsed> = forms.iter().map(|form| Parsed::new(form)).collect();

        let mut expansions: Vec<Expansion> = Vec::new();
        // Git told on the command line or in its environment to run something else
        // (`-c alias.p=push p`, `GIT_CONFIG_COUNT=1 ...`) under a rule about git.
        if self.rules.git_rules
            && !ctx.soft
            && git_config_exec(&parsed[1].resolved, &parsed[1].words)
        {
            if ctx.inner
                || parsed
                    .iter()
                    .any(|p| matches!(p.name.as_str(), "rtk" | "rtk.exe"))
            {
                self.ask = true;
            } else {
                self.defer = true;
            }
        }
        for p in &parsed {
            self.check_rules(p, 0, ctx, false)?;
        }
        // `uv run X` is peeled to `X`, but what follows `uv run` is a program uv
        // runs (`python script.py`), not a command typed in a shell.
        let uv_run = parsed[1].resolved.first().is_some_and(|word| word == "uv")
            && parsed[1].resolved.get(1).is_some_and(|word| word == "run");
        for (at, p) in parsed.iter().enumerate() {
            let peeled = uv_run && at >= 2;
            self.run_view(
                p,
                0,
                ctx,
                false,
                peeled,
                substitution,
                cwds,
                &mut expansions,
            )?;
        }

        // Inside an expansion the command may sit behind a launcher the peel does not
        // model (`caffeinate X`, `ssh host X`, `exec -a x X`): read every word-suffix
        // of the segment as a command of its own. What only a suffix reaches asks,
        // it never denies.
        if ctx.inner {
            let base = &parsed[1];
            for from in 1..base.words.len() {
                let rest = base.form.len() - base.spans[from];
                if !self.spend(4 + rest / 8) {
                    return Ok(());
                }
                self.check_rules(base, from, ctx, true)?;
                self.run_view(
                    base,
                    from,
                    ctx,
                    true,
                    false,
                    substitution,
                    cwds,
                    &mut expansions,
                )?;
            }
            // A quoted word that reads like a script (`ssh h 'x=push; git $x'`). After
            // a launcher it is a script for real (`npx -c 'cat secret.txt'`); after
            // anything else (`git commit -m "..."`) it is a guess, read for rules only.
            let script_is_real =
                LAUNCHERS.contains(&base.name.as_str()) && !operands_are_data(&base.name);
            if !ctx.soft {
                for (raw, text) in base.words.iter().zip(&base.resolved).skip(1) {
                    let quoted = raw.contains(['\'', '"']);
                    let script =
                        text.contains(|c: char| c.is_whitespace() || matches!(c, ';' | '&' | '|'));
                    if quoted && script {
                        push_expansion(
                            &mut expansions,
                            Expansion {
                                text: text.clone(),
                                via_rtk: false,
                                payload: false,
                                soft: !script_is_real,
                                demote: true,
                                open: false,
                            },
                        );
                    }
                }
            }
        }

        if self.reading() {
            apply_cd(residue, cwds, self.env);
        }

        for expansion in expansions {
            self.expanded = true;
            if depth >= MAX_DEPTH {
                self.ask = true;
                continue;
            }
            let nested_ctx = Ctx {
                inner: true,
                in_rtk: ctx.in_rtk || expansion.via_rtk,
                payload: ctx.payload || expansion.payload,
                soft: ctx.soft || expansion.soft,
                demote: ctx.demote || expansion.demote || ctx.soft || expansion.soft,
                open: expansion.open,
            };
            // A `cd` inside the expansion runs in a process of its own.
            let mut nested = cwds.clone();
            match self.scan_text(&expansion.text, depth + 1, nested_ctx, &mut nested) {
                // Only guessed or launched to run: a deny found there is a question.
                Err(Denied) if nested_ctx.demote => self.ask = true,
                other => other?,
            }
        }
        Ok(())
    }

    /// The forms of one segment: as written, past the shell's grammar and past each
    /// assignment or wrapper. `None` once the work budget is gone. Every step is
    /// charged, so a crafted run of `time time time ...` costs a bounded amount.
    fn segment_forms<'s>(&mut self, segment: &'s str, residue: &'s str) -> Option<Vec<&'s str>> {
        if !self.spend(1 + residue.len() / 2) {
            return None;
        }
        let peeled = registry::matching_readings(residue);
        if !peeled.complete {
            self.ask = true;
        }
        if !self.spend(1 + residue.len() / 2) {
            return None;
        }
        let positions = registry::shell_wrapper_positions(residue, &self.rules.prefixes);
        let mut forms: Vec<&str> = vec![segment, residue];
        for form in peeled.readings.iter().copied().chain(positions) {
            if !form.is_empty() && !forms.contains(&form) {
                forms.push(form);
            }
        }
        // Past the shell's own grammar (`then git push`, `x=1 git push`, `time -p
        // git push`), one step per form, and past the wrappers behind each step.
        let mut next = 0;
        let mut stripped: Vec<&str> = Vec::new();
        while next < forms.len() && forms.len() < MAX_FORMS {
            let form = forms[next];
            next += 1;
            if !self.spend(1 + form.len() / 2) {
                return None;
            }
            let (words, spans) = lexer::words_and_spans(form);
            stripped.clear();
            grammar_step(form, &words, &spans, &mut stripped);
            for &step in &stripped {
                if step.is_empty() || forms.contains(&step) {
                    continue;
                }
                forms.push(step);
                if !self.spend(1 + step.len() / 2) {
                    return None;
                }
                for reading in registry::matching_readings(step).readings {
                    if !reading.is_empty() && !forms.contains(&reading) {
                        forms.push(reading);
                    }
                }
            }
        }
        if forms.len() >= MAX_FORMS {
            self.ask = true;
        }
        Some(forms)
    }

    /// The Bash rules against one reading of a segment: the words from `from` on.
    /// `suffix` is a reading that only a launcher's argument list reaches, where a
    /// deny becomes an ask.
    fn check_rules(
        &mut self,
        p: &Parsed,
        from: usize,
        ctx: Ctx,
        suffix: bool,
    ) -> Result<(), Denied> {
        if p.words.len() <= from {
            return Ok(());
        }
        // A word that expands at run time after a literal prefix a rule can still
        // extend (`git $x`, `git pu*h`, `$cmd push`), or options between a rule's
        // words (`git -C x push`): the rule may apply to what runs.
        if self.bash_rules
            && (view_extends(self.rules, p, from, ctx)
                || self.rules.subsequence_hit(&p.resolved[from..]))
        {
            if ctx.inner {
                self.ask = true;
            } else {
                self.defer = true;
            }
        }
        // Rules are matched against the text as written, as the shell reads it once
        // the quotes are gone (`git 'push'` is `git push`), with the command word
        // reduced to its basename, and past git's global options.
        for reading in p.readings_at(from) {
            if reading.is_empty() {
                continue;
            }
            if self
                .rules
                .deny
                .iter()
                .any(|pattern| permissions::command_matches_pattern(&reading, pattern))
            {
                if suffix {
                    self.ask = true;
                    continue;
                }
                return Err(Denied);
            }
            if self
                .rules
                .ask
                .iter()
                .any(|pattern| permissions::command_matches_pattern(&reading, pattern))
            {
                self.ask = true;
            }
        }
        Ok(())
    }

    /// What one reading of a segment runs: Read rules, opaque commands, and the
    /// texts it hands to another command, which are queued in `expansions`.
    #[allow(clippy::too_many_arguments)]
    fn run_view(
        &mut self,
        p: &Parsed,
        from: usize,
        ctx: Ctx,
        suffix: bool,
        peeled: bool,
        substitution: bool,
        cwds: &Cwds,
        expansions: &mut Vec<Expansion>,
    ) -> Result<(), Denied> {
        let words = &p.words[from..];
        if words.is_empty() {
            return Ok(());
        }
        let name_owned = p.name_at(from);
        let name = name_owned.as_str();
        let args = &words[1..];
        let spans = &p.spans[from..];
        // Text only guessed to be a script is matched against the rules and no more.
        let quiet = ctx.soft;
        // The words after a program that takes data (`grep python src`) are not
        // commands, so a suffix is not asked about for being an interpreter.
        let data = suffix && operands_are_data(&p.name);
        let guess = !quiet && !data;
        let launcher = LAUNCHERS.contains(&p.name.as_str());
        // What a launcher's argument list runs is read like any command, except that
        // a deny found in it asks; after a data program it is read for rules only.
        let launched = |text: String, via_rtk: bool, payload: bool, open: bool| Expansion {
            text,
            via_rtk,
            payload,
            soft: data,
            demote: suffix,
            open,
        };
        // Compound commands the segmenter leaves whole.
        if guess && ctx.in_rtk && matches!(name, "for" | "select" | "case" | "[[" | "function") {
            self.opaque(ctx);
        }
        if !quiet && !suffix && self.reading() {
            if READ_COMMANDS.contains(&name) {
                self.check_read(name, args, ctx, cwds, false)?;
            } else if ctx.in_rtk && !is_assignment(name) {
                // Anything rtk runs can read a file: sed, awk, cp, jq...
                self.check_read(name, args, ctx, cwds, true)?;
            }
        }
        if guess
            && (ctx.in_rtk || ctx.payload)
            && is_opaque_command(name, words, suffix || peeled, launcher)
        {
            self.opaque(ctx);
        }
        if SHELLS.contains(&name)
            && let Some(script) = shell_wrapper_script(words)
        {
            push_expansion(expansions, launched(script, false, false, false));
        }
        if let Some(inner) = rtk_inner(p.form, words, spans) {
            if !quiet {
                self.ask |= inner.opaque || (substitution && !suffix);
            }
            for text in inner.texts {
                push_expansion(expansions, launched(text, true, false, false));
            }
        }
        let payloads = exec_payloads(name, p.form, words, spans);
        if payloads.unattested && !quiet {
            self.opaque(Ctx {
                payload: true,
                ..ctx
            });
        }
        for text in payloads.texts {
            push_expansion(expansions, launched(text, false, true, payloads.open));
        }
        Ok(())
    }

    /// The Read rules against one command's non-option words. `generic` is a
    /// command outside [`READ_COMMANDS`], met inside an rtk expansion: a denied
    /// path is still a deny, but an unresolvable one adds no prompt.
    fn check_read(
        &mut self,
        name: &str,
        args: &[&str],
        ctx: Ctx,
        cwds: &Cwds,
        generic: bool,
    ) -> Result<(), Denied> {
        if !self.spend(1 + args.iter().map(|arg| 1 + arg.len() / 16).sum::<usize>()) {
            return Ok(());
        }
        let denied = match &self.reads {
            Some(reads) => reads.check_command(name, args, cwds, generic),
            None => ReadOutcome::Clear,
        };
        if denied == ReadOutcome::Denied {
            return Err(Denied);
        }
        let asked = match &self.read_asks {
            Some(asks) => asks.check_command(name, args, cwds, generic),
            None => ReadOutcome::Clear,
        };
        self.apply_read(denied, asked, ctx, generic);
        Ok(())
    }

    /// Fold what the Read rules said. An unresolvable path, or a path an ask rule
    /// names, leaves a top-level command to the host unrewritten and makes an
    /// expansion ask.
    fn apply_read(&mut self, denied: ReadOutcome, asked: ReadOutcome, ctx: Ctx, generic: bool) {
        let unresolved = denied == ReadOutcome::Uncertain || asked == ReadOutcome::Uncertain;
        let matched_ask = asked == ReadOutcome::Denied;
        if generic {
            // A word that expands, or a path an ask rule covers, asks.
            self.ask |= matched_ask || unresolved;
        } else if unresolved || (matched_ask && ctx.inner) {
            // A path the gate cannot resolve (`$x`, a glob) may be the denied file,
            // at top level too (round 6: a defer let `Bash(rtk *)` decide).
            self.ask = true;
        } else if matched_ask {
            // A Read ask rule names it: the host asks about the original itself.
            self.defer = true;
        }
    }

    /// `< file` (and `<> file`) reads the file whatever the command is.
    fn check_input_redirects(&mut self, text: &str, ctx: Ctx, cwds: &Cwds) -> Result<(), Denied> {
        // The whole text at once: the segmenter steps over a redirect with no
        // command word before it (`< secret.txt cat`, `{ cat; } < secret.txt`).
        // After a `cd` anywhere in it a relative operand may name any directory.
        let tokens = lexer::tokenize(text);
        let moved = tokens.iter().any(|token| {
            matches!(
                resolve_word_text(&token.value).as_str(),
                "cd" | "pushd" | "popd"
            )
        });
        let mut unknown = cwds.clone();
        if moved {
            unknown.apply(&CdTarget::Unknown);
        }
        let cwds = &unknown;
        {
            let segment = text;
            for (i, token) in tokens.iter().enumerate() {
                let is_input = token.kind == TokenKind::Redirect
                    && token.value.trim_start_matches(|c: char| c.is_ascii_digit()) == "<";
                if !is_input {
                    continue;
                }
                // `<>` (read and write) lexes as `<` then `>`, glued together.
                let glued_write = tokens.get(i + 1).is_some_and(|next| {
                    next.kind == TokenKind::Redirect
                        && next.value == ">"
                        && next.offset == token.offset + token.value.len()
                });
                // The operand is every token glued together from there (`secr?t.txt`
                // lexes as `secr`, `?`, `t.txt`; `*.txt` opens with a glob).
                let operand = i + 1 + usize::from(glued_write);
                let stops = |kind: TokenKind| {
                    matches!(
                        kind,
                        TokenKind::Operator | TokenKind::Pipe(_) | TokenKind::Redirect
                    )
                };
                let Some(first) = tokens.get(operand).filter(|t| !stops(t.kind)) else {
                    continue;
                };
                let mut end = first.offset + first.value.len();
                for next in &tokens[operand + 1..] {
                    if stops(next.kind) || next.offset != end {
                        break;
                    }
                    end = next.offset + next.value.len();
                }
                let raw = &segment[first.offset..end];
                // A word that expands keeps its `$`/glob so the matcher sees it is
                // uncertain.
                let word = if is_uncertain_word(raw) {
                    raw.to_string()
                } else {
                    resolve_word_text(raw)
                };
                if word == "/dev/null" {
                    continue;
                }
                if !self.spend(1 + word.len() / 16) {
                    return Ok(());
                }
                let denied = match &self.reads {
                    Some(reads) => ReadOutcome::from(reads.check_word(&word, cwds, false)),
                    None => ReadOutcome::Clear,
                };
                if denied == ReadOutcome::Denied {
                    return Err(Denied);
                }
                let asked = match &self.read_asks {
                    Some(asks) => ReadOutcome::from(asks.check_word(&word, cwds, false)),
                    None => ReadOutcome::Clear,
                };
                self.apply_read(denied, asked, ctx, false);
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// The shared parser (RG1): what the guard says runs
// ---------------------------------------------------------------------------

/// rtk subcommands that read their operands as files, read as `cat` too.
const RTK_READS: &[&str] = &["read", "json", "log", "smart", "deps"];

/// The home `anthology_shell::analyze` expands `~` and `$HOME` with. Only the
/// guard's own path fields use it; the gate resolves paths itself.
fn shell_home() -> String {
    user_dirs::home()
        .map(|home| home.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Whether the entry was reached through an `rtk` word (`rtk read x`, `nice rtk
/// git push`): its `raw` words hold one before the program the analysis found.
fn entry_via_rtk(entry: &ShellEntry) -> bool {
    entry.raw.iter().any(|word| {
        matches!(
            basename(word).to_ascii_lowercase().as_str(),
            "rtk" | "rtk.exe"
        )
    })
}

/// The entry's command word as a rule sees it ([`norm_command`]). The analysis
/// lowercases its program name; where the program word stands in `raw` just before
/// the arguments, its case is kept, so a case-sensitive file system reads `GIT`
/// as the different program it is.
fn entry_command_word(entry: &ShellEntry) -> String {
    let raw = &entry.raw;
    let args = &entry.args;
    if raw.len() > args.len() {
        let at = raw.len() - args.len() - 1;
        if raw[at + 1..] == args[..] && anthology_shell::prog_name(&raw[at]) == entry.prog {
            return norm_command(&raw[at]);
        }
    }
    norm_command(&entry.prog)
}

/// A literal word as the gate's word reader takes it: single-quoted (`'` as
/// `'\''`) when it holds anything the shell would expand or unquote, except a
/// leading `~` (the home), which stays as it is.
fn quote_literal(word: &str) -> String {
    let special = |c: char| !(c.is_ascii_alphanumeric() || "-_./:,=+%@~^#".contains(c));
    if word.starts_with('~') || !word.contains(special) {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

/// Every way a Bash rule may name what one entry runs: its program and arguments,
/// its words as typed (wrappers included), past git's global options, a dashed git
/// builtin as `git <sub>`, and an rtk read subcommand as `cat`.
fn entry_readings(entry: &ShellEntry, words: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    push_unique(&mut out, words.join(" "));
    push_unique(&mut out, entry.raw.join(" "));
    let prog = words[0].as_str();
    let tail = |k: usize| words.get(k..).unwrap_or_default().join(" ");
    if prog == "git" {
        let raw: Vec<&str> = words.iter().map(String::as_str).collect();
        let sub = git_subcommand_index(words, &raw, 0);
        if sub > 1 && sub < words.len() {
            push_unique(&mut out, join_command("git", &tail(sub)));
        }
    }
    if let Some(sub) = prog.strip_prefix("git-")
        && !sub.is_empty()
    {
        push_unique(&mut out, join_command(&format!("git {sub}"), &tail(1)));
    }
    if entry_via_rtk(entry) {
        if RTK_READS.contains(&prog) {
            push_unique(&mut out, join_command("cat", &tail(1)));
        } else if matches!(prog, "grep" | "rg") {
            push_unique(&mut out, join_command("grep", &tail(1)));
            push_unique(&mut out, join_command("rg", &tail(1)));
        }
        // clap drops a `--` before the nested command: `rtk cargo -- test`.
        if let Some(at) = words.iter().skip(1).position(|word| word == "--") {
            let mut kept = words.to_vec();
            kept.remove(at + 1);
            push_unique(&mut out, kept.join(" "));
        }
    }
    out
}

impl Scanner<'_> {
    /// The command as the guard reads it: `anthology_shell::analyze` (bounded)
    /// flattens everything that runs (rtk runner and passthrough forms, wrappers,
    /// shells' scripts, substitutions, heredocs and here-strings fed to a shell,
    /// `eval`, `find -exec`, docker shells) into entries, and each entry meets the
    /// rules here. This pass only adds a deny or an ask to what the rule scan
    /// found, never a pass or a defer, so the gate's answer can only get stricter:
    ///
    /// - a deny rule that matches an entry denies; an ask rule asks;
    /// - an analysis cut short (`incomplete`: a regex or work limit) asks;
    /// - a `Read()`-denied path an entry names, or reads through `<`, denies; a
    ///   path that cannot be resolved asks;
    /// - in a line that may run rtk, an entry the analysis could not see into
    ///   (`opaque`), a word a rule's head can still extend, a path a `Read()` ask
    ///   rule names, or a relative path after a `cd` the analysis followed asks.
    ///   Elsewhere these add nothing: a rewrite into an rtk line is judged again.
    fn scan_shared(&mut self, cmd: &str, initial: Option<&Path>) -> Result<(), Denied> {
        if !self.spend(SEGMENT_COST + cmd.len() / 16) {
            return Ok(());
        }
        self.scan_analysis(cmd, &analyze(cmd, &shell_home()), initial)
    }

    /// [`Scanner::scan_shared`] for an analysis already made (a test passes one
    /// made with lower limits).
    fn scan_analysis(
        &mut self,
        cmd: &str,
        analysis: &Analysis,
        initial: Option<&Path>,
    ) -> Result<(), Denied> {
        if analysis.incomplete {
            self.ask = true;
        }
        let rtk_line = mentions_rtk(cmd);
        // One simple command with nothing the shell expands outside single quotes.
        let literal = plain_words(cmd).is_some();
        for entry in &analysis.entries {
            let size: usize = entry.raw.iter().map(|word| 1 + word.len()).sum();
            if !self.spend(SEGMENT_COST + size / 8) {
                return Ok(());
            }
            if rtk_line && !entry.opaque.is_empty() {
                self.ask = true;
            }
            if entry.prog.is_empty() {
                continue;
            }
            let words: Vec<String> = std::iter::once(entry_command_word(entry))
                .chain(entry.args.iter().cloned())
                .collect();
            self.shared_rules(entry, &words, rtk_line)?;
            if self.reading() {
                self.shared_reads(entry, &words, rtk_line, literal, initial)?;
            }
        }
        Ok(())
    }

    /// What the gate cannot settle for an entry: ask in a line that may run rtk.
    /// Elsewhere nothing is added: the rule scan has its own say, and a rewrite
    /// into an rtk line is judged again as one. (A defer here would be looser:
    /// the hook stops at a defer, before that second judgement can ask.)
    fn unsettled(&mut self, rtk_line: bool) {
        if rtk_line {
            self.ask = true;
        }
    }

    /// The Bash rules against one entry.
    fn shared_rules(
        &mut self,
        entry: &ShellEntry,
        words: &[String],
        rtk_line: bool,
    ) -> Result<(), Denied> {
        if !self.bash_rules {
            return Ok(());
        }
        let readings = entry_readings(entry, words);
        if !self.spend(readings.iter().map(|r| 1 + r.len() / 16).sum()) {
            return Ok(());
        }
        for reading in &readings {
            if self
                .rules
                .deny
                .iter()
                .any(|pattern| permissions::command_matches_pattern(reading, pattern))
            {
                return Err(Denied);
            }
            if self
                .rules
                .ask
                .iter()
                .any(|pattern| permissions::command_matches_pattern(reading, pattern))
            {
                self.ask = true;
            }
        }
        // A word that expands at run time after literal words a rule can still
        // extend (`git $x`), or options between a rule's words (`git -C . push`).
        let extends = match words.iter().position(|word| is_expanding(word)) {
            Some(0) => self.rules.extendable(&[]),
            Some(at) => self.rules.extendable(&words[..at]),
            None => false,
        };
        if extends || self.rules.subsequence_hit(words) {
            self.unsettled(rtk_line);
        }
        Ok(())
    }

    /// The `Read()` rules against one entry's operands and input redirects.
    fn shared_reads(
        &mut self,
        entry: &ShellEntry,
        words: &[String],
        rtk_line: bool,
        literal: bool,
        initial: Option<&Path>,
    ) -> Result<(), Denied> {
        let via_rtk = entry_via_rtk(entry);
        let name = match words[0].as_str() {
            prog if via_rtk && RTK_READS.contains(&prog) => "cat",
            prog => prog,
        };
        let generic = !READ_COMMANDS.contains(&name);
        // Outside a line that may run rtk only the commands rtk rewrites into reads
        // are read (as the rule scan does); a `cd` or `-C` the analysis followed
        // leaves a relative path unresolved.
        let cwds = if entry.cwd.is_some() {
            Cwds { states: vec![None] }
        } else {
            Cwds::new(initial.map(Path::to_path_buf))
        };
        let mut outcomes: Vec<(ReadOutcome, ReadOutcome)> = Vec::new();
        for redirect in &entry.redirs {
            if matches!(redirect.op.as_str(), "<" | "<>") && redirect.target != "/dev/null" {
                if redirect.target.contains(['\'', '"', '\\']) {
                    outcomes.push((ReadOutcome::Uncertain, ReadOutcome::Clear));
                    continue;
                }
                let check = |m: &Option<ReadMatcher>| match m {
                    Some(m) => ReadOutcome::from(m.check_word(&redirect.target, &cwds, false)),
                    None => ReadOutcome::Clear,
                };
                outcomes.push((check(&self.reads), check(&self.read_asks)));
            }
        }
        if !generic || rtk_line {
            let args = &words[1..];
            if !self.spend(1 + args.iter().map(|arg| 1 + arg.len() / 16).sum::<usize>()) {
                return Ok(());
            }
            // The analysis took the quotes off. In a line of literal words (see
            // [`plain_words`]) each word is quoted back, so `'$(id)'` stays a name;
            // otherwise a quote or backslash left in a word is one the gate would
            // read differently, and a `$` or glob in it may expand.
            let quoted: Vec<String>;
            let args: Vec<&str> = if literal {
                quoted = args.iter().map(|arg| quote_literal(arg)).collect();
                quoted.iter().map(String::as_str).collect()
            } else {
                if args.iter().any(|arg| arg.contains(['\'', '"', '\\'])) {
                    outcomes.push((ReadOutcome::Uncertain, ReadOutcome::Clear));
                }
                args.iter().map(String::as_str).collect()
            };
            let check = |m: &Option<ReadMatcher>| match m {
                Some(m) => m.check_command(name, &args, &cwds, generic),
                None => ReadOutcome::Clear,
            };
            outcomes.push((check(&self.reads), check(&self.read_asks)));
        }
        for (denied, asked) in outcomes {
            if denied == ReadOutcome::Denied {
                return Err(Denied);
            }
            if denied == ReadOutcome::Uncertain && entry.cwd.is_none() {
                // A path that cannot be resolved asks, at top level too (round 6).
                self.ask = true;
            } else if denied == ReadOutcome::Uncertain || asked != ReadOutcome::Clear {
                // Only the directory is unknown (the rule scan follows a `cd`
                // itself), or a `Read()` ask rule names the path.
                self.unsettled(rtk_line);
            }
        }
        Ok(())
    }
}

/// `NAME=value`: an assignment prefix, not a command word.
fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && !name.starts_with(|c: char| c.is_ascii_digit())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// Whether `word` appears in `text` as a whole identifier (`HOME`, `$HOME`,
/// `HOME=/x`, `export HOME`, but not `HOMEDIR`).
fn mentions_word(text: &str, word: &str) -> bool {
    let bytes = text.as_bytes();
    let ident = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    text.match_indices(word).any(|(at, _)| {
        let before = at == 0 || !ident(bytes[at - 1]);
        let after = bytes.get(at + word.len()).is_none_or(|byte| !ident(*byte));
        before && after
    })
}

/// A command whose effect the gate cannot read from its words: it evaluates a
/// string, reads a script from elsewhere, runs inline code, or hands the real
/// command to another program. Inside an rtk expansion the hook asks.
///
/// `suffix` is a word met after another program's own words (`ruff check .`): a
/// bare `.`, a shell or an interpreter there is an operand unless the program
/// before it is a `launcher`, or the word carries inline code or reads stdin.
fn is_opaque_command(name: &str, words: &[&str], suffix: bool, launcher: bool) -> bool {
    if is_assignment(name) {
        return false;
    }
    let first = words
        .first()
        .map(|w| resolve_word_text(w))
        .unwrap_or_default();
    // `[` and `[[` are the test builtin; any other glob or expansion in command
    // position names a program the gate cannot see.
    if first.contains(['$', '`', '*', '?', '[', '{']) && !matches!(first.as_str(), "[" | "[[") {
        return !suffix || launcher;
    }
    match name {
        // They evaluate a string; a trailing `.` sources nothing.
        "eval" | "source" | "." | "trap" | "alias" => !suffix || (launcher && words.len() > 1),
        "busybox" | "toybox" | "sudo" | "doas" => !suffix || launcher,
        // A shell with no `-c` reads stdin or a script file.
        shell if SHELLS.contains(&shell) => {
            shell_wrapper_script(words).is_none() && (!suffix || launcher || reads_stdin(words))
        }
        // `rg --pre CMD` runs CMD on every file.
        "rg" | "grep"
            if words[1..].iter().any(|word| {
                let word = resolve_word_text(word);
                word == "--pre" || word.starts_with("--pre=")
            }) =>
        {
            true
        }
        other => {
            is_interpreter(other)
                && (!suffix || launcher || has_inline_code(words) || reads_stdin(words))
        }
    }
}

/// No operand but options (or `-`): a shell or interpreter reading its stdin.
fn reads_stdin(words: &[&str]) -> bool {
    words[1..]
        .iter()
        .all(|word| resolve_word_text(word).starts_with('-'))
}

/// An option that carries code (`-c`, `-e`, `--eval`...).
fn has_inline_code(words: &[&str]) -> bool {
    words[1..]
        .iter()
        .any(|word| INLINE_CODE.contains(&resolve_word_text(word).as_str()))
}

/// An interpreter (or PowerShell) by basename, any version suffix allowed.
fn is_interpreter(name: &str) -> bool {
    let lower = name.to_lowercase();
    let base = lower.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.' || c == '-');
    INTERPRETERS.contains(&base)
}

/// Whether a raw word (quotes still in it) expands at run time: `$` or a backtick
/// (also inside double quotes), an unquoted `*`, `?` or `[`, a brace expansion
/// (`{a,b}`, `{1..3}`), or `~user`.
fn is_expanding(word: &str) -> bool {
    if matches!(word, "[" | "[[" | "]" | "]]" | "{}") {
        return false;
    }
    let mut quote: Option<char> = None;
    let mut chars = word.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        match quote {
            Some('\'') => {
                if c == '\'' {
                    quote = None;
                }
            }
            Some(_) => match c {
                '\\' => {
                    chars.next();
                }
                '"' => quote = None,
                '$' | '`' => return true,
                _ => {}
            },
            None => match c {
                '\\' => {
                    chars.next();
                }
                '\'' => quote = Some('\''),
                '"' => quote = Some('"'),
                '$' | '`' | '*' | '?' | '[' => return true,
                '{' => {
                    let window = &word[at + 1..];
                    let window = &window[..window.len().min(256)];
                    if let Some(end) = window.find('}')
                        && (window[..end].contains(',') || window[..end].contains(".."))
                    {
                        return true;
                    }
                }
                '~' if at == 0 && chars.peek().is_some_and(|(_, next)| *next != '/') => {
                    return true;
                }
                _ => {}
            },
        }
    }
    false
}

/// Whether the command that starts at word `from` of `p` has literal leading words
/// a Bash rule can still extend once a later word expands (`git $x`, `git pu*h`,
/// `git -C . $x`), or whose last arguments arrive at run time (`xargs git`,
/// `find -exec git {} \\;`).
fn view_extends(rules: &GateRules, p: &Parsed, from: usize, ctx: Ctx) -> bool {
    let words = &p.words[from..];
    let resolved = &p.resolved[from..];
    if words.is_empty() || is_assignment(&resolved[0]) {
        return false;
    }
    let expands = |word: &str| is_expanding(word) || (ctx.payload && word.contains("{}"));
    let at = match words.iter().position(|word| expands(word)) {
        Some(at) => at,
        None if from == 0 && ctx.open => words.len(),
        None => return false,
    };
    if at == 0 {
        // The command word itself expands. For a word inside a longer command,
        // only a later literal word of a rule's head makes that worth a question
        // (`caffeinate g$x push`), or `rtk ls src/*.rs` would ask.
        return if from == 0 {
            rules.extendable(&[])
        } else {
            rules.tail_word_in(resolved)
        };
    }
    rules.extendable(&resolved[..at])
}

/// How many forms of one segment the gate keeps.
const MAX_FORMS: usize = 128;

/// The shell grammar that can open a command without being part of it.
const GRAMMAR_LEADERS: &[&str] = &[
    "if", "then", "else", "elif", "while", "until", "do", "!", "{", "(",
];

/// One step of peeling the shell's grammar off the front of `form`: a reserved
/// word, `time [-p]`, `coproc [NAME]`, `function NAME`, or a `name=value`
/// assignment in any case. The remainders are pushed to `out`; `words` and `spans`
/// are `form` lexed once.
fn grammar_step<'s>(form: &'s str, words: &[&str], spans: &[usize], out: &mut Vec<&'s str>) {
    let Some(first) = words.first() else {
        return;
    };
    let first = resolve_word_text(first);
    let drop = match first.as_str() {
        word if GRAMMAR_LEADERS.contains(&word) => 1,
        "time" => {
            if words.get(1).is_some_and(|w| resolve_word_text(w) == "-p") {
                2
            } else {
                1
            }
        }
        "coproc" => {
            // `coproc NAME { ...; }` or `coproc cmd`: read both.
            if let Some(&at) = spans.get(2) {
                out.push(form[at..].trim_start());
            }
            1
        }
        "function" => 2,
        word if is_assignment(word) => 1,
        _ => return,
    };
    if let Some(&at) = spans.get(drop) {
        out.push(form[at..].trim_start());
    }
}

/// Whether the command changes `HOME` or `CDPATH` (or may, by computing a name
/// for `export` and its kin), as `(home, cdpath)`: `~` and a bare `cd` mean
/// something else after it. Read over the text with its quotes and backslashes
/// gone, since `export HO"ME"=/` sets `HOME`.
fn command_env_flags(cmd: &str) -> (bool, bool) {
    let stripped: String = cmd
        .chars()
        .filter(|c| !matches!(c, '\'' | '"' | '\\'))
        .collect();
    let expands_name = |word: &str| {
        word.split('=')
            .next()
            .unwrap_or(word)
            .contains(['$', '`', '*', '?', '[', '{'])
    };
    let computed = stripped
        .split([';', '&', '|', '\n', '(', ')'])
        .any(|statement| {
            let words: Vec<&str> = statement.split_whitespace().collect();
            words.iter().enumerate().any(|(at, word)| match *word {
                "export" | "declare" | "typeset" | "readonly" | "local" | "read" => {
                    words[at + 1..].iter().any(|w| expands_name(w))
                }
                "printf" => {
                    words.get(at + 1) == Some(&"-v")
                        && words.get(at + 2).is_some_and(|w| expands_name(w))
                }
                _ => false,
            })
        });
    let cdpath = computed || mentions_word(&stripped, "CDPATH");
    (cdpath || mentions_word(&stripped, "HOME"), cdpath)
}

/// The commands an exec action runs.
#[derive(Debug, Default, PartialEq, Eq)]
struct Payloads {
    texts: Vec<String>,
    /// The action's command could not be read.
    unattested: bool,
    /// The command's last arguments come from stdin or a file (`xargs`, `parallel`).
    open: bool,
}

/// What `form` hands to another command: the payload of `find -exec`,
/// `-execdir`, `-ok` and `-okdir`, the command `xargs` runs, and the command
/// `env` runs, past each one's options.
fn exec_payloads(name: &str, form: &str, words: &[&str], spans: &[usize]) -> Payloads {
    let slice = |from: usize, to: usize| -> String {
        form[spans[from]..spans[to] + words[to].len()].to_string()
    };
    let resolved = |at: usize| resolve_word_text(words[at]);
    let mut out = Payloads::default();
    match name {
        "find" => {
            let mut i = 1;
            while i < words.len() {
                if !matches!(
                    resolved(i).as_str(),
                    "-exec" | "-execdir" | "-ok" | "-okdir"
                ) {
                    i += 1;
                    continue;
                }
                let start = i + 1;
                let end = (start..words.len()).find(|&k| matches!(resolved(k).as_str(), ";" | "+"));
                match end {
                    Some(end) if end > start => {
                        out.texts.push(slice(start, end - 1));
                        i = end + 1;
                    }
                    Some(end) => {
                        out.unattested = true;
                        i = end + 1;
                    }
                    None => {
                        out.unattested = true;
                        if start < words.len() {
                            out.texts.push(slice(start, words.len() - 1));
                        }
                        break;
                    }
                }
            }
        }
        "parallel" => {
            out.open = true;
            const WITH_VALUE: &[&str] = &[
                "-j",
                "-n",
                "-N",
                "-S",
                "-a",
                "-L",
                "-I",
                "-E",
                "-d",
                "-C",
                "-P",
                "--jobs",
                "--sshlogin",
                "--arg-file",
                "--max-args",
                "--colsep",
                "--delimiter",
                "--results",
                "--joblog",
            ];
            let mut i = 1;
            while i < words.len() {
                let word = resolved(i);
                if word == "--" {
                    i += 1;
                    break;
                }
                if !word.starts_with('-') || word == "-" {
                    break;
                }
                i += if WITH_VALUE.contains(&word.as_str()) {
                    2
                } else {
                    1
                };
            }
            let end = (i..words.len())
                .find(|&k| matches!(resolved(k).as_str(), ":::" | "::::" | ":::+" | "::::+"))
                .unwrap_or(words.len());
            if i < end {
                out.texts.push(slice(i, end - 1));
            }
        }
        "xargs" => {
            out.open = true;
            // Options that take the next word as their value.
            const WITH_VALUE: &[&str] = &["-n", "-I", "-L", "-P", "-s", "-d", "-E", "-a"];
            const LONG_WITH_VALUE: &[&str] = &[
                "--max-args",
                "--max-procs",
                "--delimiter",
                "--arg-file",
                "--max-chars",
                "--process-slot-var",
            ];
            const NO_VALUE: &[&str] = &[
                "-0",
                "-t",
                "-r",
                "-p",
                "-x",
                "-o",
                "--null",
                "--verbose",
                "--no-run-if-empty",
                "--interactive",
                "--open-tty",
                "--exit",
                "--show-limits",
            ];
            let mut i = 1;
            while i < words.len() {
                let word = resolved(i);
                if word == "--" {
                    i += 1;
                    break;
                }
                if !word.starts_with('-') || word == "-" {
                    break;
                }
                if WITH_VALUE.contains(&word.as_str()) || LONG_WITH_VALUE.contains(&word.as_str()) {
                    i += 2;
                } else if NO_VALUE.contains(&word.as_str())
                    || word.contains('=') && word.starts_with("--")
                    || WITH_VALUE
                        .iter()
                        .any(|opt| word.len() > 2 && word.starts_with(opt))
                    || matches!(word.get(..2), Some("-e" | "-i" | "-l"))
                    || word.starts_with("--replace")
                    || word.starts_with("--eof")
                    || word.starts_with("--max-lines")
                    || (!word.starts_with("--") && word[1..].chars().all(|c| "0trpxo".contains(c)))
                {
                    i += 1;
                } else {
                    out.unattested = true;
                    return out;
                }
            }
            if i < words.len() {
                out.texts.push(slice(i, words.len() - 1));
            }
        }
        "env" => {
            let mut i = 1;
            while i < words.len() {
                let word = resolved(i);
                if word == "--" {
                    i += 1;
                    break;
                }
                if matches!(
                    word.as_str(),
                    "-i" | "-" | "--ignore-environment" | "-0" | "--null" | "-v" | "--debug"
                ) || is_assignment(&word)
                {
                    i += 1;
                } else if matches!(word.as_str(), "-u" | "--unset" | "-C" | "--chdir" | "-P") {
                    i += 2;
                } else if word.starts_with("--unset=")
                    || word.starts_with("--chdir=")
                    || (word.starts_with("-u") || word.starts_with("-C") || word.starts_with("-P"))
                        && word.len() > 2
                {
                    i += 1;
                } else if word == "-S" || word == "--split-string" {
                    // The string is split into the command and its arguments.
                    if let Some(next) = words.get(i + 1) {
                        let mut text = resolve_word_text(next);
                        if i + 2 < words.len() {
                            text = format!("{text} {}", slice(i + 2, words.len() - 1));
                        }
                        out.texts.push(text);
                    } else {
                        out.unattested = true;
                    }
                    return out;
                } else if let Some(split) = word
                    .strip_prefix("--split-string=")
                    .or_else(|| word.strip_prefix("-S").filter(|rest| !rest.is_empty()))
                {
                    let mut text = split.to_string();
                    if i + 1 < words.len() {
                        text = format!("{text} {}", slice(i + 1, words.len() - 1));
                    }
                    out.texts.push(text);
                    return out;
                } else if word.starts_with('-') {
                    out.unattested = true;
                    return out;
                } else {
                    break;
                }
            }
            if i < words.len() {
                out.texts.push(slice(i, words.len() - 1));
            }
        }
        _ => {}
    }
    out
}

fn push_unique<T: PartialEq>(items: &mut Vec<T>, item: T) {
    if !items.contains(&item) {
        items.push(item);
    }
}

fn basename(word: &str) -> &str {
    word.rsplit(['/', '\\']).next().unwrap_or(word)
}

// ---------------------------------------------------------------------------
// Shell wrappers and rtk's inner commands
// ---------------------------------------------------------------------------

/// The script of `<shell> [options] -c <script>`, unquoted. `words` is the shell
/// word first. `None` when the shell is not given a command string.
fn shell_wrapper_script(words: &[&str]) -> Option<String> {
    let mut command_string = false;
    let mut i = 1;
    while let Some(raw) = words.get(i) {
        let word = resolve_word_text(raw);
        if word == "--" {
            return if command_string {
                words.get(i + 1).map(|w| resolve_word_text(w))
            } else {
                None
            };
        }
        if let Some(script) = word.strip_prefix("--command=") {
            return Some(script.to_string());
        }
        if word == "--command" {
            return words.get(i + 1).map(|w| resolve_word_text(w));
        }
        if word.starts_with("--") {
            // `--rcfile FILE` and `--init-file FILE` take a separate value.
            i += if matches!(word.as_str(), "--rcfile" | "--init-file") {
                2
            } else {
                1
            };
            continue;
        }
        if let Some(flags) = word.strip_prefix('-')
            && !flags.is_empty()
        {
            command_string |= flags.contains('c');
            // `-o NAME` and `-O NAME` take a separate value.
            i += if matches!(word.as_str(), "-o" | "-O") {
                2
            } else {
                1
            };
            continue;
        }
        if word.starts_with('+') && word.len() > 1 {
            i += if matches!(word.as_str(), "+o" | "+O") {
                2
            } else {
                1
            };
            continue;
        }
        return command_string.then_some(word);
    }
    None
}

/// The commands an `rtk ...` segment runs.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct RtkInner {
    /// Texts to scan as commands of their own.
    pub(crate) texts: Vec<String>,
    /// The inner command cannot be read: the hook asks.
    pub(crate) opaque: bool,
}

fn is_global_flag(word: &str) -> bool {
    let word = resolve_word_text(word);
    word == "--verbose"
        || word == "--skip-env"
        || word
            .strip_prefix('-')
            .is_some_and(|vs| !vs.is_empty() && vs.chars().all(|c| c == 'v'))
}

fn join_command(name: &str, rest: &str) -> String {
    if rest.is_empty() {
        name.to_string()
    } else {
        format!("{name} {rest}")
    }
}

/// `rest` as a command text, plus the text with a leading `--` dropped (clap
/// swallows it).
fn push_rest(texts: &mut Vec<String>, rest: &str) {
    let rest = rest.trim();
    if rest.is_empty() {
        return;
    }
    push_unique(texts, rest.to_string());
    if let Some(stripped) = rest.strip_prefix("--").map(str::trim_start)
        && !stripped.is_empty()
    {
        push_unique(texts, stripped.to_string());
    }
}

/// The inner command of an `rtk` segment (`form` is the segment text; `words` and
/// `spans` its words and their byte offsets), or `None` when its command word is
/// not `rtk`.
pub(crate) fn rtk_inner(form: &str, words: &[&str], spans: &[usize]) -> Option<RtkInner> {
    let first = resolve_word_text(words.first()?);
    if !matches!(basename(&first), "rtk" | "rtk.exe") {
        return None;
    }
    let mut inner = RtkInner::default();
    let mut i = 1;
    while words.get(i).is_some_and(|word| is_global_flag(word)) {
        i += 1;
    }
    let Some(sub_raw) = words.get(i) else {
        return Some(inner);
    };
    let sub = resolve_word_text(sub_raw);
    let rest_from = |j: usize| -> &str { spans.get(j).map_or("", |&at| form[at..].trim()) };
    let rest = rest_from(i + 1);

    match sub.as_str() {
        "run" => run_inner(words, spans, form, i + 1, &mut inner),
        "err" | "test" | "summary" => shell_option_inner(words, spans, form, i + 1, &mut inner),
        "bypass" | "proxy" => {
            push_rest(&mut inner.texts, rest);
            lone_script(&mut inner, words, i + 1);
        }
        "read" | "json" | "log" | "smart" | "deps" => {
            push_unique(&mut inner.texts, join_command(&sub, rest));
            push_unique(&mut inner.texts, join_command("cat", rest));
        }
        "grep" | "rg" => {
            push_unique(&mut inner.texts, join_command("grep", rest));
            push_unique(&mut inner.texts, join_command("rg", rest));
        }
        _ => {
            push_unique(&mut inner.texts, join_command(&sub, rest));
            // `--skip-env` is global: rtk accepts it after the subcommand too, so
            // `rtk git --skip-env push` runs `git push`.
            let without: Vec<&str> = words[i + 1..]
                .iter()
                .copied()
                .filter(|word| resolve_word_text(word) != "--skip-env")
                .collect();
            if without.len() != words.len() - (i + 1) {
                push_unique(&mut inner.texts, join_command(&sub, &without.join(" ")));
            }
            // clap drops a `--` before the nested command: `rtk git -- push` runs
            // `git push`.
            let forwarded = &words[i + 1..];
            if let Some(at) = forwarded
                .iter()
                .position(|word| resolve_word_text(word) == "--")
            {
                let mut kept: Vec<&str> = forwarded.to_vec();
                kept.remove(at);
                push_unique(&mut inner.texts, join_command(&sub, &kept.join(" ")));
            }
            // `rtk 'git push'`: rtk's fallback runs a lone word through `sh -c`.
            if words.len() == i + 1 {
                push_script(&mut inner, None, sub.clone());
            }
        }
    }
    Some(inner)
}

/// `rtk run`: `-c S` / `-cS` / `--command S` / `--command=S` with an optional
/// `--shell X`, else the remaining argv.
fn run_inner(words: &[&str], spans: &[usize], form: &str, from: usize, inner: &mut RtkInner) {
    let mut shell: Option<String> = None;
    let mut script: Option<String> = None;
    let mut j = from;
    while let Some(raw) = words.get(j) {
        let word = resolve_word_text(raw);
        if word == "--skip-env" {
            j += 1;
        } else if let Some(value) = word.strip_prefix("--shell=") {
            shell = Some(value.to_string());
            j += 1;
        } else if word == "--shell" {
            shell = words.get(j + 1).map(|w| resolve_word_text(w));
            j += 2;
        } else if let Some(value) = word.strip_prefix("--command=") {
            script = Some(value.to_string());
            j += 1;
        } else if word == "--command" || word == "-c" {
            script = words.get(j + 1).map(|w| resolve_word_text(w));
            j += 2;
        } else if word.len() > 2 && word.starts_with("-c") && !word.starts_with("--") {
            script = Some(word[2..].to_string());
            j += 1;
        } else {
            break;
        }
    }
    match script {
        Some(script) => push_script(inner, shell.as_deref(), script),
        None => {
            if let Some(&at) = spans.get(j) {
                push_rest(&mut inner.texts, &form[at..]);
            }
            lone_script(inner, words, j);
        }
    }
}

/// A single argv word that is not a program is run through `sh -c` by rtk
/// (`core::shell::direct_command`), so it is also a script. Read it as one
/// whatever it holds: being conservative costs a scan.
fn lone_script(inner: &mut RtkInner, words: &[&str], from: usize) {
    if words.len() == from + 1 {
        push_script(inner, None, resolve_word_text(words[from]));
    }
}

/// `rtk err|test|summary [--shell X] ...`: the rest is the command; with a
/// shell, a single unquoted argument is a script for it.
fn shell_option_inner(
    words: &[&str],
    spans: &[usize],
    form: &str,
    from: usize,
    inner: &mut RtkInner,
) {
    let mut shell: Option<String> = None;
    let mut j = from;
    while let Some(raw) = words.get(j) {
        let word = resolve_word_text(raw);
        if word == "--skip-env" {
            j += 1;
        } else if let Some(value) = word.strip_prefix("--shell=") {
            shell = Some(value.to_string());
            j += 1;
        } else if word == "--shell" {
            shell = words.get(j + 1).map(|w| resolve_word_text(w));
            j += 2;
        } else {
            break;
        }
    }
    let Some(&at) = spans.get(j) else {
        return;
    };
    let rest = form[at..].trim();
    match shell {
        Some(shell) => {
            push_rest(&mut inner.texts, rest);
            let parts = lexer::shell_split(rest);
            push_script(inner, Some(&shell), parts.join(" "));
        }
        None => {
            push_rest(&mut inner.texts, rest);
            lone_script(inner, words, j);
        }
    }
}

/// A script for `shell` (the POSIX `sh` when none is named). A shell the lexer
/// does not model also adds the `<shell> -c <script>` reading and makes the inner
/// command opaque.
fn push_script(inner: &mut RtkInner, shell: Option<&str>, script: String) {
    if let Some(shell) = shell
        && !POSIX_SHELLS.contains(&basename(shell))
    {
        inner.opaque = true;
        push_unique(&mut inner.texts, format!("{shell} -c {script}"));
    }
    if !script.trim().is_empty() {
        push_unique(&mut inner.texts, script);
    }
}

// ---------------------------------------------------------------------------
// The plain shape (see "The plain shape" in the module doc)
// ---------------------------------------------------------------------------

/// How the operands of a plain subcommand are read.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Operands {
    /// Every operand is a path.
    Paths,
    /// The first operand is a pattern (unless `-e` gave one), the rest are paths.
    PatternThenPaths,
    /// Git revisions and paths: no `:` (`REV:path` is a blob), every one checked
    /// as a path, and a `rev~N`/`rev^` suffix allowed.
    Git,
    /// Names the program looks up itself (test filters), not files.
    Names,
    /// No operands.
    None,
}

/// One rtk subcommand that passes without a prompt, with the options it may take.
struct PlainSub {
    /// The words after `rtk` (`["git", "status"]`).
    words: &'static [&'static str],
    /// Options without a value. Short ones may cluster (`-rn`); `--name=v` is
    /// read as a value.
    flags: &'static [&'static str],
    /// Options that take a value: the next word, `--name=v`, or glued to a short
    /// option (`-A3`).
    values: &'static [&'static str],
    /// Values that are patterns, not files: not checked as paths.
    patterns: &'static [&'static str],
    operands: Operands,
    /// Options whose value is optional and taken only when glued (`--pretty=x`,
    /// `-U5`, `-uno`): a bare one never takes the next word.
    glued: &'static [&'static str],
    /// Options whose value is free text, never an operand (`commit -m`): it may
    /// hold a `:` in a git command.
    free: &'static [&'static str],
}

impl PlainSub {
    const fn glued(mut self, glued: &'static [&'static str]) -> Self {
        self.glued = glued;
        self
    }

    const fn free(mut self, free: &'static [&'static str]) -> Self {
        self.free = free;
        self
    }
}

const GIT_LOG_FLAGS: &[&str] = &[
    "--oneline",
    "--graph",
    "--decorate",
    "--no-decorate",
    "--all",
    "--stat",
    "--shortstat",
    "--numstat",
    "-p",
    "--patch",
    "-s",
    "--no-patch",
    "--no-merges",
    "--merges",
    "--reverse",
    "--name-only",
    "--name-status",
    "--follow",
    "--abbrev-commit",
    "--first-parent",
    "--color",
    "--no-color",
    "-i",
    "--regexp-ignore-case",
    "--date-order",
    "--topo-order",
    // Optional values, glued only: `--pretty=oneline`, `--format=%h`.
    "--pretty",
    "--format",
];
const GIT_LOG_VALUES: &[&str] = &[
    "-n",
    "--max-count",
    "--skip",
    "--author",
    "--committer",
    "--grep",
    "--since",
    "--until",
    "--after",
    "--before",
    "--date",
];
const GIT_DIFF_FLAGS: &[&str] = &[
    "--stat",
    "--shortstat",
    "--numstat",
    "--name-only",
    "--name-status",
    "--cached",
    "--staged",
    "--color",
    "--no-color",
    "-w",
    "--ignore-all-space",
    "-b",
    "--word-diff",
    "--no-ext-diff",
    "-M",
    "-R",
    "--check",
    "--summary",
    "--exit-code",
    "--quiet",
    "--minimal",
    "--patience",
    "--histogram",
];

const fn sub(
    words: &'static [&'static str],
    flags: &'static [&'static str],
    values: &'static [&'static str],
    operands: Operands,
) -> PlainSub {
    PlainSub {
        words,
        flags,
        values,
        patterns: &[],
        operands,
        glued: &[],
        free: &[],
    }
}

const CARGO_FLAGS: &[&str] = &[
    "-r",
    "--release",
    "--all",
    "--workspace",
    "--lib",
    "--bins",
    "--tests",
    "--benches",
    "--examples",
    "--all-targets",
    "--all-features",
    "--no-default-features",
    "-q",
    "--quiet",
    "-v",
    "--verbose",
    "--locked",
    "--offline",
    "--frozen",
    "--doc",
    "--no-run",
    "--no-fail-fast",
    "--keep-going",
];
const CARGO_VALUES: &[&str] = &[
    "-p",
    "--package",
    "--exclude",
    "-F",
    "--features",
    "--test",
    "--bin",
    "--example",
    "--bench",
    "-j",
    "--jobs",
    "--message-format",
    "--color",
    "--profile",
];

const GREP_FLAGS: &[&str] = &[
    "-r",
    "-R",
    "--recursive",
    "-n",
    "--line-number",
    "-i",
    "--ignore-case",
    "-l",
    "--files-with-matches",
    "-L",
    "-c",
    "--count",
    "-w",
    "--word-regexp",
    "-v",
    "--invert-match",
    "-E",
    "-F",
    "--fixed-strings",
    "-H",
    "-h",
    "-o",
    "-s",
    "-q",
    "-x",
    "-I",
    "-S",
    "--smart-case",
    "--hidden",
    "--no-heading",
    "--color",
];
const GREP_VALUES: &[&str] = &[
    "-A",
    "-B",
    "-C",
    "--after-context",
    "--before-context",
    "--context",
    "-m",
    "--max-count",
    "-e",
    "--regexp",
    "--include",
    "--exclude",
    "--exclude-dir",
    "-g",
    "--glob",
    "-t",
    "--type",
    "--max-depth",
];
const GREP_PATTERNS: &[&str] = &[
    "-e",
    "--regexp",
    "--include",
    "--exclude",
    "--exclude-dir",
    "-g",
    "--glob",
];

/// The rtk subcommands that pass without a prompt when any rule exists: reads of
/// the tree and the repository's own build and test, with only the options listed.
/// What is not here asks: kubectl, oc, docker, dotnet, go, prettier, rspec,
/// npm/npx/pnpm/yarn/bun, pip/uv, gh/glab, the runners (`bypass`, `run`, `proxy`,
/// `err`, `test`, `summary`), and every program that loads configuration or runs
/// code a flag names.
const PLAIN: &[PlainSub] = &[
    sub(
        &["ls"],
        &[
            "-l", "-a", "-A", "-h", "-R", "-1", "-t", "-r", "-S", "-d", "-F", "-p", "--color",
        ],
        &[],
        Operands::Paths,
    ),
    sub(
        &["tree"],
        &[
            "-a",
            "-d",
            "-f",
            "-i",
            "-C",
            "-n",
            "--gitignore",
            "--noreport",
            "--dirsfirst",
        ],
        &["-L", "-I", "-P", "--filelimit"],
        Operands::Paths,
    ),
    sub(
        &["read"],
        &["-n", "--line-numbers"],
        &[
            "-l",
            "--level",
            "-m",
            "--max-lines",
            "--head-lines",
            "--tail-lines",
        ],
        Operands::Paths,
    ),
    sub(
        &["wc"],
        &["-l", "-w", "-c", "-m", "-L"],
        &[],
        Operands::Paths,
    ),
    sub(&["diff"], &[], &[], Operands::Paths),
    PlainSub {
        words: &["grep"],
        flags: GREP_FLAGS,
        values: GREP_VALUES,
        patterns: GREP_PATTERNS,
        operands: Operands::PatternThenPaths,
        glued: &[],
        free: &[],
    },
    PlainSub {
        words: &["rg"],
        flags: GREP_FLAGS,
        values: GREP_VALUES,
        patterns: GREP_PATTERNS,
        operands: Operands::PatternThenPaths,
        glued: &[],
        free: &[],
    },
    sub(
        &["git", "status"],
        &[
            "-s",
            "--short",
            "-b",
            "--branch",
            "--porcelain",
            "--long",
            "-v",
            "--ignored",
            "--show-stash",
            "-z",
            "--untracked-files",
        ],
        &[],
        Operands::Git,
    )
    .glued(&["-u"]),
    sub(
        &["git", "log"],
        GIT_LOG_FLAGS,
        GIT_LOG_VALUES,
        Operands::Git,
    ),
    sub(
        &["git", "diff"],
        GIT_DIFF_FLAGS,
        &["--diff-filter"],
        Operands::Git,
    )
    .glued(&["-U", "--unified"]),
    sub(
        &["git", "show"],
        &[
            "--stat",
            "--shortstat",
            "--numstat",
            "--name-only",
            "--name-status",
            "--oneline",
            "-s",
            "--no-patch",
            "--color",
            "--no-color",
            "--abbrev-commit",
        ],
        &[],
        Operands::Git,
    )
    .glued(&["--format", "--pretty", "-U", "--unified"]),
    sub(
        &["git", "branch"],
        &[
            "-a",
            "--all",
            "-r",
            "--remotes",
            "-v",
            "--verbose",
            "--list",
            "--show-current",
            "--color",
            "--no-color",
            "--merged",
            "--no-merged",
            "--contains",
        ],
        &["--sort"],
        Operands::Git,
    ),
    sub(
        &["git", "blame"],
        &[
            "-w",
            "-M",
            "-C",
            "-e",
            "-s",
            "-l",
            "-p",
            "--porcelain",
            "--line-porcelain",
            "-c",
            "--root",
            "-f",
        ],
        &["-L", "--date"],
        Operands::Git,
    ),
    sub(
        &["git", "add"],
        &[
            "-A",
            "--all",
            "-u",
            "--update",
            "-n",
            "--dry-run",
            "-v",
            "--verbose",
            "-N",
            "--intent-to-add",
        ],
        &[],
        Operands::Git,
    ),
    sub(
        &["git", "commit"],
        &[
            "-a",
            "--all",
            "--amend",
            "--no-edit",
            "-q",
            "--quiet",
            "--allow-empty",
            "-s",
            "--signoff",
            "--no-verify",
        ],
        &["-m", "--message"],
        Operands::Git,
    )
    .free(&["-m", "--message"]),
    sub(
        &["git", "checkout"],
        &[
            "-q",
            "--quiet",
            "-f",
            "--force",
            "--detach",
            "-t",
            "--track",
            "--no-track",
        ],
        &["-b", "-B"],
        Operands::Git,
    ),
    sub(
        &["git", "switch"],
        &[
            "-q",
            "--quiet",
            "-f",
            "--discard-changes",
            "--detach",
            "-t",
            "--track",
            "--no-track",
        ],
        &["-c", "-C", "--create"],
        Operands::Git,
    ),
    sub(
        &["git", "rev-parse"],
        &[
            "--show-toplevel",
            "--abbrev-ref",
            "--short",
            "--git-dir",
            "--absolute-git-dir",
            "--is-inside-work-tree",
            "--verify",
            "--symbolic-full-name",
            "-q",
            "--quiet",
            "--show-prefix",
            "--show-cdup",
        ],
        &[],
        Operands::Git,
    ),
    sub(
        &["git", "ls-files"],
        &[
            "-m",
            "--modified",
            "-o",
            "--others",
            "--exclude-standard",
            "-c",
            "--cached",
            "-d",
            "--deleted",
            "-z",
            "-s",
            "--stage",
            "--directory",
        ],
        &[],
        Operands::Git,
    ),
    PlainSub {
        words: &["cargo", "test"],
        flags: CARGO_FLAGS,
        values: CARGO_VALUES,
        patterns: &[],
        operands: Operands::Names,
        glued: &[],
        free: &[],
    },
    sub(
        &["cargo", "build"],
        CARGO_FLAGS,
        CARGO_VALUES,
        Operands::None,
    ),
    sub(
        &["cargo", "check"],
        CARGO_FLAGS,
        CARGO_VALUES,
        Operands::None,
    ),
    PlainSub {
        words: &["cargo", "clippy"],
        flags: CARGO_FLAGS,
        values: CARGO_VALUES,
        patterns: &[],
        operands: Operands::None,
        glued: &[],
        free: &[],
    },
    PlainSub {
        words: &["cargo", "fmt"],
        flags: &["--all", "--check", "-q", "--quiet", "-v", "--verbose"],
        values: &["-p", "--package"],
        patterns: &[],
        operands: Operands::None,
        glued: &[],
        free: &[],
    },
];

/// `find`'s expression words: without a value, and with one.
const FIND_FLAGS: &[&str] = &[
    "-print", "-print0", "-empty", "-prune", "-not", "!", "-o", "-or", "-a", "-and", "(", ")",
];
const FIND_VALUES: &[&str] = &[
    "-name",
    "-iname",
    "-path",
    "-ipath",
    "-type",
    "-maxdepth",
    "-mindepth",
    "-size",
    "-mtime",
    "-mmin",
];
const FIND_PATTERNS: &[&str] = &["-name", "-iname", "-path", "-ipath"];

/// One word of a plain line, quotes removed.
struct PlainWord {
    text: String,
    /// It opens with an unquoted `~` (alone or `~/...`): the shell puts the home
    /// there.
    tilde: bool,
    /// It has an unquoted `^`, `#` or a `~` after its first character: under zsh's
    /// `EXTENDED_GLOB` those are glob operators. Allowed only as a git revision of
    /// the strict shape [`strict_rev`], or in a commit message.
    revish: bool,
    /// Some of it is quoted.
    quoted: bool,
}

/// The words of `text` when it is one simple command with nothing the shell
/// expands or runs besides it, else `None`. Outside quotes only letters, digits,
/// spaces and `-_./:,=+%@` stand, plus a leading `~` or `~/` and the zsh glob
/// operators `^`, `#` and a later `~` (marked [`PlainWord::revish`] for the caller
/// to refuse or admit). Inside single quotes everything printable is literal;
/// inside double quotes `$`, a backquote, a backslash and `!` are refused, so what
/// a quote holds is literal in every POSIX shell and zsh.
fn plain_words(text: &str) -> Option<Vec<PlainWord>> {
    let mut words = Vec::new();
    let mut current: Option<PlainWord> = None;
    let mut quote: Option<char> = None;
    let mut prev: Option<char> = None;
    for c in text.chars() {
        if !(c == ' ' || c == '\t' || c.is_ascii_graphic()) {
            return None;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else if q == '"' && matches!(c, '$' | '`' | '\\' | '!') {
                return None;
            } else {
                current.as_mut()?.text.push(c);
            }
            prev = Some(c);
            continue;
        }
        if c == ' ' || c == '\t' {
            words.extend(current.take());
            prev = None;
            continue;
        }
        let word = current.get_or_insert_with(|| PlainWord {
            text: String::new(),
            tilde: false,
            revish: false,
            quoted: false,
        });
        let at_start = prev.is_none();
        match c {
            '\'' | '"' => {
                // A quote inside `~...` before its `/` turns the expansion off.
                if word.tilde && word.text == "~" {
                    return None;
                }
                word.quoted = true;
                quote = Some(c);
                prev = Some(c);
                continue;
            }
            '~' if at_start => word.tilde = true,
            // A `#` opening a word starts a comment.
            '#' if at_start => return None,
            '~' | '^' | '#' => word.revish = true,
            '=' if at_start || prev == Some('=') => return None,
            c if c.is_ascii_alphanumeric() || "-_./:,=+%@".contains(c) => {}
            _ => return None,
        }
        if word.tilde && word.text == "~" && c != '/' {
            return None;
        }
        word.text.push(c);
        prev = Some(c);
    }
    if quote.is_some() {
        return None;
    }
    words.extend(current.take());
    Some(words)
}

/// What git's pathspec matching reads as a wildcard, quoted on the line or not.
const GIT_WILDCARDS: &[char] = &['*', '?', '[', '\\'];

/// A git revision word zsh's `EXTENDED_GLOB` cannot turn into a path: `REF`, then
/// any of `~N`, `^N`, `~`, `^` and nothing after; or two of those joined by `..`
/// or `...`. `REF` is letters, digits, `_` and `-` (not first): no `/`, no `.`.
fn strict_rev(text: &str) -> bool {
    let one = |rev: &str| {
        let end = rev.find(['~', '^']).unwrap_or(rev.len());
        let (name, mut suffix) = rev.split_at(end);
        if name.is_empty()
            || name.starts_with('-')
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
        {
            return false;
        }
        while let Some(rest) = suffix.strip_prefix(['~', '^']) {
            suffix = rest.trim_start_matches(|c: char| c.is_ascii_digit());
        }
        suffix.is_empty()
    };
    match text.split_once("...").or_else(|| text.split_once("..")) {
        Some((from, to)) => one(from) && one(to),
        None => one(text),
    }
}

/// A value or operand to check as a path ([`plain_path`]).
struct Check {
    text: String,
    tilde: bool,
    revish: bool,
    quoted: bool,
}

impl Check {
    fn of(word: &PlainWord) -> Self {
        Self {
            text: word.text.clone(),
            tilde: word.tilde,
            revish: word.revish,
            quoted: word.quoted,
        }
    }

    /// A value glued to its option (`--name=v`, `-Av`) in `word`.
    fn value(text: &str, word: &PlainWord) -> Self {
        Self {
            text: text.to_string(),
            tilde: false,
            revish: word.revish,
            quoted: word.quoted,
        }
    }
}

/// What [`plain`] says about a line that may run rtk: `Pass` when it has the plain
/// shape (one `rtk` command from [`PLAIN`] with listed options and literal paths
/// no `Read()` rule names), `Deny` when one of its paths is `Read()`-denied, else
/// `Ask`.
pub(crate) fn plain(cmd: &str, rules: &GateRules, cwd: Option<&str>) -> Gate {
    if rules.literal_in(cmd) {
        return Gate::Ask;
    }
    let Some(words) = plain_words(cmd) else {
        return Gate::Ask;
    };
    // `@FILE` is an argument file to rustc, rustdoc and many other parsers.
    if words.iter().any(|word| word.text.starts_with('@')) {
        return Gate::Ask;
    }
    let Some((first, rest)) = words.split_first() else {
        return Gate::Ask;
    };
    if first.text != "rtk" || first.tilde || first.revish {
        return Gate::Ask;
    }
    if !shared_parser_agrees(cmd, &words) {
        return Gate::Ask;
    }
    let mut checks = Vec::new();
    let found = if rest.first().is_some_and(|word| word.text == "find") {
        find_args(&rest[1..], &mut checks)
    } else {
        PLAIN.iter().any(|spec| {
            spec.words.len() <= rest.len()
                && spec
                    .words
                    .iter()
                    .zip(rest)
                    .all(|(want, word)| *want == word.text && !word.revish && !word.tilde)
                && {
                    checks.clear();
                    plain_args(spec, &rest[spec.words.len()..], &mut checks)
                }
        })
    };
    if !found || checks.iter().any(|check| check.text.starts_with('@')) {
        return Gate::Ask;
    }
    let cwd = cwd
        .filter(|cwd| !cwd.is_empty())
        .map(PathBuf::from)
        .or_else(|| user_dirs::current_dir().ok())
        .map(|dir| normalize(&dir));
    let fold = cfg!(any(target_os = "macos", windows));
    let matcher = |patterns: &[String]| {
        (!patterns.is_empty()).then(|| {
            ReadMatcher::new(
                patterns,
                user_dirs::home(),
                &rules.project_roots,
                cwd.as_deref(),
                fold,
            )
        })
    };
    let reads = matcher(&rules.read_deny);
    let read_asks = matcher(&rules.read_ask);
    let mut gate = Gate::Pass;
    for check in &checks {
        let mut texts = vec![(check.text.clone(), check.tilde)];
        if check.revish {
            // Under zsh's `EXTENDED_GLOB`, `^`, `#` and a later `~` are glob
            // operators (`c^1./.env` matches `config/.env`): only a strict git
            // revision stays, and since `x~1`, `x^1` and `x~1..y` still match
            // names that start with `x` (`secret^1` matches `secret.txt`), every
            // such name in the directory is checked too.
            if check.quoted || check.tilde || !strict_rev(&check.text) {
                return Gate::Ask;
            }
            let stem = check.text.split(['~', '^']).next().unwrap_or_default();
            match stem_matches(stem, cwd.as_deref(), fold, STEM_SCAN_CAP) {
                Some(names) => texts.extend(names.into_iter().map(|t| (t, false))),
                None => return Gate::Ask,
            }
        }
        for (text, tilde) in texts {
            let verdict = plain_path(&text, tilde, cwd.as_deref(), fold, |path| {
                let verdict = |m: &Option<ReadMatcher>| {
                    m.as_ref()
                        .map_or(WordVerdict::Allowed, |m| m.check_literal(path))
                };
                match (verdict(&reads), verdict(&read_asks)) {
                    (WordVerdict::Denied, _) => Gate::Deny,
                    (_, WordVerdict::Denied) => Gate::Ask,
                    _ => Gate::Pass,
                }
            });
            gate = match (gate, verdict) {
                (Gate::Deny, _) | (_, Gate::Deny) => Gate::Deny,
                (Gate::Ask, _) | (_, Gate::Ask) => Gate::Ask,
                _ => Gate::Pass,
            };
        }
    }
    gate
}

/// Whether the guard's parser reads a plain line as [`plain_words`] does: one
/// complete analysis, one entry with nothing opaque and no redirect, and the
/// same words. Two lexers that disagree on a line the gate would let through is
/// a reason to ask (design G-2).
fn shared_parser_agrees(cmd: &str, words: &[PlainWord]) -> bool {
    let analysis = analyze(cmd, &shell_home());
    let [entry] = analysis.entries.as_slice() else {
        return false;
    };
    !analysis.incomplete
        && entry.opaque.is_empty()
        && entry.redirs.is_empty()
        && entry.raw.len() == words.len()
        && entry
            .raw
            .iter()
            .zip(words)
            .all(|(raw, word)| *raw == word.text)
}

/// One literal path: resolved lexically against the working directory (`..`
/// normalised; a leading `~` is `$HOME`; an unset, empty or relative `HOME` asks), asked about
/// under `/dev`, `/proc` or `/sys` (case folded where the file system folds it),
/// and when it is `-` (standard input); else what `read` says about the absolute
/// path (which also resolves symlinks).
fn plain_path(
    text: &str,
    tilde: bool,
    cwd: Option<&Path>,
    fold: bool,
    read: impl Fn(&str) -> Gate,
) -> Gate {
    if text.is_empty() {
        return Gate::Pass;
    }
    if text == "-" {
        return Gate::Ask;
    }
    let path = if tilde {
        // The shell expands `~` from `$HOME` as it stands, relative or not.
        match crate::core::user_env::var("HOME").filter(|home| !home.is_empty()) {
            Some(home) => {
                PathBuf::from(home).join(text.trim_start_matches('~').trim_start_matches('/'))
            }
            None => return Gate::Ask,
        }
    } else if Path::new(text).is_absolute() {
        PathBuf::from(text)
    } else {
        match cwd {
            Some(dir) => dir.join(text),
            None => return Gate::Ask,
        }
    };
    // A relative `HOME` or working directory names nothing a rule can anchor to.
    if !path.is_absolute() {
        return Gate::Ask;
    }
    let lexical = normalize(&path);
    let special = |path: &Path| {
        path.components()
            .find_map(|component| match component {
                std::path::Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
                _ => None,
            })
            .is_some_and(|first| {
                let first = if fold { first.to_lowercase() } else { first };
                matches!(first.as_str(), "dev" | "proc" | "sys")
            })
    };
    // The kernel resolves `link/..` through the link, the lexical form does not:
    // both are checked (the matcher normalises and resolves `path` itself).
    if special(&lexical)
        || std::fs::canonicalize(&path).is_ok_and(|real| special(&real))
        || std::fs::canonicalize(&lexical).is_ok_and(|real| special(&real))
    {
        return Gate::Ask;
    }
    match (
        read(&path.to_string_lossy()),
        read(&lexical.to_string_lossy()),
    ) {
        (Gate::Deny, _) | (_, Gate::Deny) => Gate::Deny,
        (Gate::Pass, Gate::Pass) => Gate::Pass,
        _ => Gate::Ask,
    }
}

/// How many directory entries [`stem_matches`] reads before it gives up (asks).
const STEM_SCAN_CAP: usize = 10_000;

/// The stem itself and every entry of its directory whose name starts with the
/// stem's last component (case folded where the file system folds it); `None`
/// when the directory holds more than `cap` entries, so a name may go unchecked.
fn stem_matches(stem: &str, cwd: Option<&Path>, fold: bool, cap: usize) -> Option<Vec<String>> {
    let mut out = vec![stem.to_string()];
    let (dir, prefix) = match stem.rsplit_once('/') {
        Some((dir, prefix)) => (if dir.is_empty() { "/" } else { dir }, prefix),
        None => (".", stem),
    };
    let base = if Path::new(dir).is_absolute() {
        PathBuf::from(dir)
    } else {
        cwd?.join(dir)
    };
    let prefix = if fold {
        prefix.to_lowercase()
    } else {
        prefix.to_string()
    };
    if let Ok(entries) = std::fs::read_dir(&base) {
        for (seen, entry) in entries.flatten().enumerate() {
            if seen >= cap {
                return None;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let folded = if fold {
                name.to_lowercase()
            } else {
                name.clone()
            };
            if folded.starts_with(&prefix) {
                out.push(if dir == "." {
                    name
                } else {
                    format!("{dir}/{name}")
                });
            }
        }
    }
    Some(out)
}

/// Whether `args` pass `spec`: every option listed; each value and operand that
/// is not a pattern queued in `checks`. A value that may be a separate word never
/// opens with `-` (it might be an option the program reads as one), and in a git
/// command no word but a free-text value ([`PlainSub::free`]) holds a `:`.
fn plain_args(spec: &PlainSub, args: &[PlainWord], checks: &mut Vec<Check>) -> bool {
    let git = spec.operands == Operands::Git;
    // A value with a zsh glob operator in it: a strict git revision only, a
    // pattern value included (its expansion may add operands).
    // git reads its pathspecs as globs, quoted or not (`'secre[t].txt'`), and a
    // value may be one: in a git command no word holds a wildcard.
    let value_rev_ok = |value: &Check| {
        (!value.revish || (git && !value.quoted && !value.tilde && strict_rev(&value.text)))
            && !(git && value.text.contains(GIT_WILDCARDS))
    };
    let colon_ok =
        |name: &str, value: &str| !git || !value.contains(':') || spec.free.contains(&name);
    let mut operands: Vec<&PlainWord> = Vec::new();
    let mut pattern_given = false;
    let mut i = 0;
    while let Some(word) = args.get(i) {
        i += 1;
        let text = word.text.as_str();
        if word.revish && !git {
            return false;
        }
        if text == "--" {
            // cargo hands what follows to rustc, rustdoc or the test binary.
            if spec.words.first() == Some(&"cargo") && i < args.len() {
                return false;
            }
            if !git && args[i..].iter().any(|word| word.revish) {
                return false;
            }
            operands.extend(&args[i..]);
            break;
        }
        if let Some(long) = text.strip_prefix("--").filter(|long| !long.is_empty()) {
            let (name, inline) = match long.split_once('=') {
                Some((name, value)) => (format!("--{name}"), Some(value)),
                None => (text.to_string(), None),
            };
            let name = name.as_str();
            let takes_next = spec.values.contains(&name);
            if !(takes_next || spec.flags.contains(&name) || spec.glued.contains(&name)) {
                return false;
            }
            let pattern = spec.patterns.contains(&name);
            pattern_given |= pattern && name == "--regexp";
            let value = match inline {
                Some(value) => Some(Check::value(value, word)),
                None if takes_next => {
                    let Some(value) = args.get(i).filter(|value| !value.text.starts_with('-'))
                    else {
                        return false;
                    };
                    i += 1;
                    Some(Check::of(value))
                }
                None => None,
            };
            if let Some(value) = value {
                // A commit message is no path: neither checked nor refused.
                if spec.free.contains(&name) {
                    continue;
                }
                if !colon_ok(name, &value.text) || !value_rev_ok(&value) {
                    return false;
                }
                if !pattern {
                    checks.push(value);
                }
            }
            continue;
        }
        if let Some(short) = text.strip_prefix('-').filter(|short| !short.is_empty()) {
            if spec.words == ["git", "log"] && short.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            for (at, c) in short.char_indices() {
                let opt = format!("-{c}");
                let opt = opt.as_str();
                let glued = &short[at + c.len_utf8()..];
                if spec.glued.contains(&opt) {
                    // An optional value, taken only when glued (`-U5`, `-uno`).
                    if !colon_ok(opt, glued) || !value_rev_ok(&Check::value(glued, word)) {
                        return false;
                    }
                    if !glued.is_empty() {
                        checks.push(Check::value(glued, word));
                    }
                    break;
                }
                if spec.flags.contains(&opt) {
                    continue;
                }
                if !spec.values.contains(&opt) {
                    return false;
                }
                let pattern = spec.patterns.contains(&opt);
                pattern_given |= pattern && opt == "-e";
                let value = if glued.is_empty() {
                    let Some(value) = args.get(i).filter(|value| !value.text.starts_with('-'))
                    else {
                        return false;
                    };
                    i += 1;
                    Check::of(value)
                } else {
                    Check::value(glued, word)
                };
                if spec.free.contains(&opt) {
                    break;
                }
                if !colon_ok(opt, &value.text) || !value_rev_ok(&value) {
                    return false;
                }
                if !pattern {
                    checks.push(value);
                }
                break;
            }
            continue;
        }
        operands.push(word);
    }
    operands_pass(spec, &operands, pattern_given, checks)
}

fn operands_pass(
    spec: &PlainSub,
    operands: &[&PlainWord],
    pattern_given: bool,
    checks: &mut Vec<Check>,
) -> bool {
    let skip = match spec.operands {
        Operands::None => return operands.is_empty(),
        Operands::Names => return true,
        Operands::Paths => 0,
        Operands::Git
            if operands
                .iter()
                .any(|word| word.text.contains(':') || word.text.contains(GIT_WILDCARDS)) =>
        {
            return false;
        }
        Operands::Git => 0,
        Operands::PatternThenPaths if pattern_given => 0,
        Operands::PatternThenPaths if operands.is_empty() => return false,
        Operands::PatternThenPaths => 1,
    };
    checks.extend(operands.iter().skip(skip).map(|word| Check::of(word)));
    true
}

/// `rtk find PATH... EXPRESSION`: the paths up to the first expression word, then
/// only the words of [`FIND_FLAGS`] and [`FIND_VALUES`].
fn find_args(args: &[PlainWord], checks: &mut Vec<Check>) -> bool {
    if args.iter().any(|word| word.revish) {
        return false;
    }
    let mut i = 0;
    while let Some(word) = args.get(i) {
        if word.text.starts_with('-') || matches!(word.text.as_str(), "(" | ")" | "!") {
            break;
        }
        checks.push(Check::of(word));
        i += 1;
    }
    while let Some(word) = args.get(i) {
        i += 1;
        let text = word.text.as_str();
        if FIND_FLAGS.contains(&text) {
            continue;
        }
        if !FIND_VALUES.contains(&text) {
            return false;
        }
        let Some(value) = args.get(i).filter(|value| !value.text.starts_with('-')) else {
            return false;
        };
        i += 1;
        if !FIND_PATTERNS.contains(&text) {
            checks.push(Check::of(value));
        }
    }
    true
}

/// Whether `text` may run rtk anywhere: `rtk` in it once quotes and backslashes are
/// removed (any case), or a `$'...'` that may spell it. Such a line passes only in
/// the plain shape ([`plain`]); a defer would leave it to the `Bash(rtk *)` allow.
pub(crate) fn mentions_rtk(text: &str) -> bool {
    let unquoted: String = text
        .chars()
        .filter(|c| !matches!(c, '\'' | '"' | '\\'))
        .collect();
    unquoted.to_ascii_lowercase().contains("rtk") || text.contains("$'")
}

/// The words of one segment, as written, with each redirect and its operand left
/// out: the shell takes a redirect anywhere among a command's words, so `git
/// >/dev/null push` runs `git push`.
fn command_words(segment: &str) -> Vec<&str> {
    let tokens = lexer::tokenize(segment);
    let mut words = Vec::new();
    let mut i = 0;
    let stops = |kind: TokenKind| {
        matches!(
            kind,
            TokenKind::Operator | TokenKind::Pipe(_) | TokenKind::Redirect
        )
    };
    while let Some(tok) = tokens.get(i) {
        if matches!(tok.kind, TokenKind::Operator | TokenKind::Pipe(_)) {
            i += 1;
            continue;
        }
        let mut end = tok.offset + tok.value.len();
        i += 1;
        if tok.kind == TokenKind::Redirect {
            // A here-string arrives as `<<` then `<`.
            if tok.value.ends_with("<<")
                && let Some(next) = tokens.get(i)
                && next.kind == TokenKind::Redirect
                && next.value == "<"
                && next.offset == end
            {
                end += 1;
                i += 1;
            }
            if !lexer::redirect_takes_operand(&tok.value) {
                continue;
            }
            // The operand: the next run of glued tokens, glued to the operator or not.
            let mut first = true;
            while let Some(next) = tokens.get(i) {
                if stops(next.kind) || (!first && next.offset != end) {
                    break;
                }
                first = false;
                end = next.offset + next.value.len();
                i += 1;
            }
            continue;
        }
        // A word: this token and every token glued to it, up to a redirect.
        let start = tok.offset;
        while let Some(next) = tokens.get(i) {
            if stops(next.kind) || next.offset != end {
                break;
            }
            end = next.offset + next.value.len();
            i += 1;
        }
        words.push(&segment[start..end]);
    }
    words
}

/// `segment` with its redirects left out ([`command_words`]), the other words as
/// written and joined by single spaces.
fn without_redirects(segment: &str) -> Cow<'_, str> {
    if lexer::tokenize(segment)
        .iter()
        .all(|tok| tok.kind != TokenKind::Redirect)
    {
        return Cow::Borrowed(segment);
    }
    Cow::Owned(command_words(segment).join(" "))
}

// ---------------------------------------------------------------------------
// Working directory
// ---------------------------------------------------------------------------

/// Where the command may be running: the working directory and every directory a
/// `cd` earlier in the same command may have left it in (a `cd` can fail, sit in
/// a subshell, or be conditional, so each earlier state stays possible). `None`
/// is a directory that cannot be known.
#[derive(Clone)]
struct Cwds {
    states: Vec<Option<PathBuf>>,
}

/// More states than this collapse into "unknown".
const MAX_CWD_STATES: usize = 16;

enum CdTarget {
    Absolute(PathBuf),
    Relative(String),
    Unknown,
}

impl Cwds {
    fn new(initial: Option<PathBuf>) -> Self {
        Self {
            states: vec![initial.map(|dir| normalize(&dir))],
        }
    }

    fn apply(&mut self, target: &CdTarget) {
        let moved: Vec<Option<PathBuf>> = self
            .states
            .iter()
            .map(|state| match target {
                CdTarget::Absolute(path) => Some(normalize(path)),
                CdTarget::Relative(rel) => state.as_ref().map(|dir| normalize(&dir.join(rel))),
                CdTarget::Unknown => None,
            })
            .collect();
        for state in moved {
            push_unique(&mut self.states, state);
        }
        if self.states.len() > MAX_CWD_STATES {
            self.states.truncate(MAX_CWD_STATES);
            push_unique(&mut self.states, None);
        }
    }
}

/// Track `cd`/`pushd`/`popd` for the segment `segment`.
fn apply_cd(segment: &str, cwds: &mut Cwds, env: EnvFlags) {
    let (words, _) = lexer::words_and_spans(segment);
    let Some(first) = words.first() else {
        return;
    };
    let target = match basename(&resolve_word_text(first)) {
        "cd" => cd_target(&words[1..], true, env),
        "pushd" => cd_target(&words[1..], false, env),
        "popd" => CdTarget::Unknown,
        _ => return,
    };
    cwds.apply(&target);
}

fn cd_target(args: &[&str], empty_is_home: bool, env: EnvFlags) -> CdTarget {
    let mut operand = None;
    for raw in args {
        let word = resolve_word_text(raw);
        if word == "-" {
            return CdTarget::Unknown;
        }
        if word == "--" {
            continue;
        }
        if word.starts_with('-') {
            continue;
        }
        operand = Some(word);
        break;
    }
    let Some(operand) = operand else {
        return match (empty_is_home && !env.home, user_dirs::home()) {
            (true, Some(home)) => CdTarget::Absolute(home),
            _ => CdTarget::Unknown,
        };
    };
    if is_uncertain_word(&operand) {
        return CdTarget::Unknown;
    }
    if env.home && operand.starts_with('~') {
        return CdTarget::Unknown;
    }
    if operand == "~" || operand.starts_with("~/") {
        return match user_dirs::home() {
            Some(home) => CdTarget::Absolute(
                home.join(operand.trim_start_matches("~/").trim_start_matches('~')),
            ),
            None => CdTarget::Unknown,
        };
    }
    if operand.starts_with('~') {
        return CdTarget::Unknown;
    }
    if Path::new(&operand).is_absolute() {
        CdTarget::Absolute(PathBuf::from(operand))
    } else if env.cdpath
        && !(matches!(operand.as_str(), "." | "..")
            || operand.starts_with("./")
            || operand.starts_with("../"))
    {
        // With `CDPATH` set a bare relative operand may land anywhere.
        CdTarget::Unknown
    } else {
        CdTarget::Relative(operand)
    }
}

/// Lexical `.` and `..` normalisation (no filesystem access).
fn normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // Popping at the root stays at the root.
                if !out.pop() {
                    out.push(component.as_os_str());
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Read() deny rules
// ---------------------------------------------------------------------------

/// What a `READ` command's paths come to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadOutcome {
    Clear,
    Uncertain,
    Denied,
}

impl From<WordVerdict> for ReadOutcome {
    fn from(verdict: WordVerdict) -> Self {
        match verdict {
            WordVerdict::Allowed => Self::Clear,
            WordVerdict::Uncertain | WordVerdict::Expanding => Self::Uncertain,
            WordVerdict::Denied => Self::Denied,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WordVerdict {
    Allowed,
    /// The directory it is relative to, or the home, is not known.
    Uncertain,
    /// The word expands at run time (`$x`, a glob, a brace, `~user`).
    Expanding,
    Denied,
}

/// A word the gate cannot turn into one path: it expands at run time.
fn is_uncertain_word(word: &str) -> bool {
    word != "{}" && word.contains(['$', '`', '*', '?', '[', '{'])
}

/// One component of a rule's path pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Comp {
    Literal(String),
    Glob(Vec<GlobItem>),
    /// `**`: any number of components, including none.
    Any,
}

impl Comp {
    fn parse(text: &str) -> Self {
        if text == "**" {
            Self::Any
        } else if text.contains(['*', '?', '[', '\\']) {
            Self::Glob(compile_glob(text))
        } else {
            Self::Literal(text.to_string())
        }
    }

    fn matches(&self, name: &str) -> bool {
        match self {
            Self::Literal(literal) => literal == name,
            Self::Glob(glob) => {
                let name: Vec<char> = name.chars().collect();
                glob_match(glob, &name)
            }
            Self::Any => true,
        }
    }
}

/// A rule, resolved to the patterns that can match an absolute path: a pattern
/// matches when it matches the path or one of its ancestors.
#[derive(Debug)]
enum Rule {
    /// Matches every read.
    All,
    /// A pattern with no `/` (bar a trailing one): the basename at any depth.
    AnyDepth(Comp),
    /// Anchored patterns, one per base directory the anchor may mean.
    Anchored(Vec<Vec<Comp>>),
}

struct ReadMatcher {
    rules: Vec<Rule>,
    case_insensitive: bool,
    /// The command changes `HOME` or `CDPATH`: `~` names nothing the gate knows.
    home_unstable: bool,
}

impl ReadMatcher {
    /// `patterns` are the raw `Read(...)` contents. The anchors follow Claude
    /// Code's: `//abs` is `/abs`, `~/p` the home, `/p` the project root, and `p`
    /// or `./p` the working directory. A base that cannot be resolved becomes a
    /// leading `**`, which can only match more.
    fn new(
        patterns: &[String],
        home: Option<PathBuf>,
        project_roots: &[PathBuf],
        cwd: Option<&Path>,
        case_insensitive: bool,
    ) -> Self {
        let bases = |dirs: &[PathBuf]| -> Vec<Vec<Comp>> {
            let mut out: Vec<Vec<Comp>> = Vec::new();
            for dir in dirs {
                push_unique(&mut out, dir_comps(dir));
                if let Ok(real) = std::fs::canonicalize(dir) {
                    push_unique(&mut out, dir_comps(&real));
                }
            }
            if out.is_empty() {
                out.push(vec![Comp::Any]);
            }
            out
        };
        let cwd_dirs: Vec<PathBuf> = cwd.map(Path::to_path_buf).into_iter().collect();
        let home_dirs: Vec<PathBuf> = home.into_iter().collect();
        let project_dirs: Vec<PathBuf> = if project_roots.is_empty() {
            cwd_dirs.clone()
        } else {
            project_roots.to_vec()
        };

        let fold = |text: &str| {
            if case_insensitive {
                text.to_lowercase()
            } else {
                text.to_string()
            }
        };
        let anchored = |base: Vec<Vec<Comp>>, rest: &str| -> Rule {
            let tail: Vec<Comp> = rest
                .split('/')
                .filter(|part| !part.is_empty() && *part != ".")
                .map(|part| Comp::parse(&fold(part)))
                .collect();
            Rule::Anchored(
                base.into_iter()
                    .map(|mut base| {
                        base.extend(tail.iter().cloned());
                        base
                    })
                    .collect(),
            )
        };
        let fold_bases = |bases: Vec<Vec<Comp>>| -> Vec<Vec<Comp>> {
            bases
                .into_iter()
                .map(|base| {
                    base.into_iter()
                        .map(|comp| match comp {
                            Comp::Literal(text) => Comp::Literal(fold(&text)),
                            other => other,
                        })
                        .collect()
                })
                .collect()
        };

        let rules = patterns
            .iter()
            .map(|raw| {
                let pattern = raw.trim();
                if matches!(pattern, "" | "*" | "**" | "**/*" | "/**" | "//**") {
                    Rule::All
                } else if let Some(rest) = pattern.strip_prefix("//") {
                    anchored(vec![Vec::new()], rest)
                } else if let Some(rest) = pattern.strip_prefix("~/") {
                    anchored(fold_bases(bases(&home_dirs)), rest)
                } else if let Some(rest) = pattern.strip_prefix('/') {
                    anchored(fold_bases(bases(&project_dirs)), rest)
                } else if let Some(rest) = pattern.strip_prefix("./") {
                    anchored(fold_bases(bases(&cwd_dirs)), rest)
                } else {
                    let bare = pattern.trim_end_matches('/');
                    if bare.contains('/') {
                        anchored(fold_bases(bases(&cwd_dirs)), bare)
                    } else {
                        Rule::AnyDepth(Comp::parse(&fold(bare)))
                    }
                }
            })
            .collect();
        Self {
            rules,
            case_insensitive,
            home_unstable: false,
        }
    }

    fn with_unstable_home(mut self, unstable: bool) -> Self {
        self.home_unstable = unstable;
        self
    }

    /// The outcome for one command: `args` are its words after the command.
    /// `generic` is a command that is not one of [`READ_COMMANDS`]: a bare `Read`
    /// rule (every read) does not make everything it runs a read.
    fn check_command(&self, name: &str, args: &[&str], cwds: &Cwds, generic: bool) -> ReadOutcome {
        if !generic && self.rules.iter().any(|rule| matches!(rule, Rule::All)) {
            return ReadOutcome::Denied;
        }
        let mut uncertain = false;
        let mut expanding = false;
        for candidate in read_candidates(name, args) {
            match self.check_word_as(&candidate.text, cwds, generic, !candidate.expands) {
                WordVerdict::Denied => return ReadOutcome::Denied,
                WordVerdict::Uncertain if !candidate.pattern => uncertain = true,
                WordVerdict::Expanding if !candidate.pattern && (candidate.expands || !generic) => {
                    expanding = true
                }
                WordVerdict::Uncertain | WordVerdict::Expanding | WordVerdict::Allowed => {}
            }
        }
        // A plain unknown directory is no reason to prompt for a program that is
        // no read; a word that expands (`secre*`, `$f`) is.
        if expanding || (uncertain && !generic) {
            ReadOutcome::Uncertain
        } else {
            ReadOutcome::Clear
        }
    }

    /// Whether one absolute, literal path (no expansion: [`plain`] built it) is
    /// denied, as written or with its symlinks resolved. A `*` or `$` in it is a
    /// character of the name.
    fn check_literal(&self, path: &str) -> WordVerdict {
        let path = Path::new(path);
        if self.denied(&normalize(path), true)
            || std::fs::canonicalize(path).is_ok_and(|real| self.denied(&real, true))
        {
            WordVerdict::Denied
        } else {
            WordVerdict::Allowed
        }
    }

    /// Whether one path word is denied, allowed or cannot be resolved.
    fn check_word(&self, word: &str, cwds: &Cwds, generic: bool) -> WordVerdict {
        self.check_word_as(word, cwds, generic, false)
    }

    /// [`ReadMatcher::check_word`] for a word the shell takes literally when
    /// `literal` (quoted or escaped as written): a `$`, `*` or `{` in it is part
    /// of the name, not an expansion.
    fn check_word_as(&self, word: &str, cwds: &Cwds, generic: bool, literal: bool) -> WordVerdict {
        if word.is_empty() {
            return WordVerdict::Allowed;
        }
        if !generic && self.rules.iter().any(|rule| matches!(rule, Rule::All)) {
            return WordVerdict::Denied;
        }
        if !literal && is_uncertain_word(word) {
            return WordVerdict::Expanding;
        }
        // `/proc/self/cwd/x`, `/proc/1/root/...`, `/dev/fd/3/x`: another name for a
        // directory the gate cannot see.
        if word.starts_with("/proc/") || word.starts_with("/dev/fd/") {
            return WordVerdict::Uncertain;
        }
        let mut candidates: Vec<PathBuf> = Vec::new();
        let mut unknown = false;
        if word.starts_with('~') && self.home_unstable {
            unknown = true;
        } else if word == "~" || word.starts_with("~/") {
            match user_dirs::home() {
                Some(home) => {
                    candidates.push(home.join(word.trim_start_matches('~').trim_start_matches('/')))
                }
                None => unknown = true,
            }
        } else if word.starts_with('~') {
            // `~user`: whoever that is.
            return WordVerdict::Expanding;
        } else if Path::new(word).is_absolute() {
            candidates.push(PathBuf::from(word));
        } else {
            for state in &cwds.states {
                match state {
                    Some(dir) => candidates.push(dir.join(word)),
                    None => unknown = true,
                }
            }
        }
        for path in &candidates {
            if self.denied(&normalize(path), generic) {
                return WordVerdict::Denied;
            }
            if let Ok(real) = std::fs::canonicalize(path)
                && self.denied(&real, generic)
            {
                return WordVerdict::Denied;
            }
        }
        if unknown {
            WordVerdict::Uncertain
        } else {
            WordVerdict::Allowed
        }
    }

    /// Whether the path, or an ancestor of it, matches a rule.
    fn denied(&self, path: &Path, ignore_all: bool) -> bool {
        let comps: Vec<String> = path
            .components()
            .filter_map(|component| match component {
                std::path::Component::Normal(name) => Some(if self.case_insensitive {
                    name.to_string_lossy().to_lowercase()
                } else {
                    name.to_string_lossy().into_owned()
                }),
                _ => None,
            })
            .collect();
        self.rules.iter().any(|rule| match rule {
            Rule::All => !ignore_all,
            Rule::AnyDepth(comp) => comps.iter().any(|name| comp.matches(name)),
            Rule::Anchored(patterns) => {
                patterns.iter().any(|pattern| match_prefix(pattern, &comps))
            }
        })
    }
}

fn dir_comps(dir: &Path) -> Vec<Comp> {
    normalize(dir)
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(name) => {
                Some(Comp::Literal(name.to_string_lossy().into_owned()))
            }
            _ => None,
        })
        .collect()
}

/// Whether `pattern` matches `path` or an ancestor of it, so a pattern consumed
/// in full matches whatever lies below. Iterative, with `**` backtracking to the
/// last one only.
fn match_prefix(pattern: &[Comp], path: &[String]) -> bool {
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    loop {
        match pattern.get(p) {
            None => return true,
            Some(Comp::Any) => {
                star = Some((p, t));
                p += 1;
                continue;
            }
            Some(comp) if path.get(t).is_some_and(|name| comp.matches(name)) => {
                p += 1;
                t += 1;
                continue;
            }
            Some(_) => {}
        }
        match star {
            Some((star_p, star_t)) if star_t < path.len() => {
                star = Some((star_p, star_t + 1));
                p = star_p + 1;
                t = star_t + 1;
            }
            _ => return false,
        }
    }
}

/// One element of a compiled glob.
#[derive(Debug, Clone, PartialEq, Eq)]
enum GlobItem {
    /// `*`: any run of characters within the component.
    Star,
    /// `?`
    Any,
    /// `[...]`
    Class(CharClass),
    Literal(char),
}

impl GlobItem {
    fn matches(&self, c: char) -> bool {
        match self {
            Self::Star => true,
            Self::Any => true,
            Self::Class(class) => class.matches(c),
            Self::Literal(literal) => *literal == c,
        }
    }
}

/// gitignore-style glob for one path component: `*`, `?`, `[...]`, `\x`.
fn compile_glob(pattern: &str) -> Vec<GlobItem> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut items = Vec::new();
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        match c {
            '*' => {
                if items.last() != Some(&GlobItem::Star) {
                    items.push(GlobItem::Star);
                }
                i += 1;
            }
            '?' => {
                items.push(GlobItem::Any);
                i += 1;
            }
            '[' => match parse_class(&chars[i..]) {
                Some((class, used)) => {
                    items.push(GlobItem::Class(class));
                    i += used;
                }
                None => {
                    items.push(GlobItem::Literal('['));
                    i += 1;
                }
            },
            '\\' if i + 1 < chars.len() => {
                items.push(GlobItem::Literal(chars[i + 1]));
                i += 2;
            }
            other => {
                items.push(GlobItem::Literal(other));
                i += 1;
            }
        }
    }
    items
}

/// Whether the whole of `text` matches `pattern`. Iterative: on a mismatch it
/// falls back to the last `*` alone, so the cost is the pattern's length times
/// the text's at worst, never exponential in the number of stars.
fn glob_match(pattern: &[GlobItem], text: &[char]) -> bool {
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        match pattern.get(p) {
            Some(GlobItem::Star) => {
                star = Some((p, t));
                p += 1;
                continue;
            }
            Some(item) if item.matches(text[t]) => {
                p += 1;
                t += 1;
                continue;
            }
            _ => {}
        }
        match star {
            Some((star_p, star_t)) => {
                star = Some((star_p, star_t + 1));
                p = star_p + 1;
                t = star_t + 1;
            }
            None => return false,
        }
    }
    pattern[p..].iter().all(|item| *item == GlobItem::Star)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CharClass {
    negated: bool,
    ranges: Vec<(char, char)>,
}

impl CharClass {
    fn matches(&self, c: char) -> bool {
        self.ranges.iter().any(|&(lo, hi)| lo <= c && c <= hi) != self.negated
    }
}

/// A `[...]` class at the start of `pattern`, and how many characters it spans.
fn parse_class(pattern: &[char]) -> Option<(CharClass, usize)> {
    let mut i = 1;
    let negated = matches!(pattern.get(i), Some('!' | '^'));
    if negated {
        i += 1;
    }
    let mut ranges = Vec::new();
    let mut first = true;
    while let Some(&c) = pattern.get(i) {
        if c == ']' && !first {
            return Some((CharClass { negated, ranges }, i + 1));
        }
        first = false;
        if pattern.get(i + 1) == Some(&'-') && pattern.get(i + 2).is_some_and(|e| *e != ']') {
            ranges.push((c, pattern[i + 2]));
            i += 3;
        } else {
            ranges.push((c, c));
            i += 1;
        }
    }
    None
}

/// An operand, and the value of a `key=value` operand too (`dd if=secret.txt`),
/// each also without a leading `@` (`curl -d @secret.txt`, `gh api -F x=@secret.txt`).
fn push_operand(out: &mut Vec<Candidate>, arg: &str, pattern: bool, expands: bool) {
    let mut push = |text: &str| {
        out.push(Candidate {
            text: text.to_string(),
            pattern,
            expands,
        });
        if let Some(file) = text.strip_prefix('@')
            && !file.is_empty()
        {
            out.push(Candidate {
                text: file.to_string(),
                pattern,
                expands,
            });
        }
    };
    push(arg);
    if let Some((key, value)) = arg.split_once('=')
        && !value.is_empty()
        && !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        push(value);
    }
}

/// One word a READ command may read.
struct Candidate {
    text: String,
    /// A grep/rg search pattern: still checked as a path, but a glob-looking one
    /// does not make the command uncertain.
    pattern: bool,
    /// The word as typed expands at run time (an unquoted glob, `$x`, ...).
    expands: bool,
}

/// Every non-option word of a READ command (and the value of `--opt=value`) is a
/// candidate path. `find`'s paths end where its expression starts; `grep` and `rg`
/// take their pattern as the first operand unless `-e`/`-f` supply it.
fn read_candidates(name: &str, args: &[&str]) -> Vec<Candidate> {
    let resolved: Vec<String> = args.iter().map(|arg| resolve_word_text(arg)).collect();
    let expanding: Vec<bool> = args.iter().map(|arg| is_expanding(arg)).collect();
    let search = matches!(name, "grep" | "rg");
    let find = name == "find";
    let explicit_pattern = search
        && resolved.iter().any(|arg| {
            matches!(arg.as_str(), "-e" | "-f" | "--regexp" | "--file")
                || arg.starts_with("--regexp=")
                || arg.starts_with("--file=")
        });
    let mut pattern_pending = search && !explicit_pattern;
    // A `git commit` message is text, not a path.
    let commit = name == "git"
        && resolved
            .iter()
            .find(|arg| !arg.starts_with('-'))
            .is_some_and(|sub| sub == "commit");
    let mut out = Vec::new();
    let mut options_done = false;
    let mut i = 0;
    while let Some(arg) = resolved.get(i) {
        let here = expanding[i];
        i += 1;
        if commit && !options_done {
            if arg == "--message" {
                i += 1;
                continue;
            }
            if arg.starts_with("--message=") {
                continue;
            }
            // `-m`, `-am`, `-qsm`, `-mtext`: boolean flags, then `m` and its value.
            if let Some(short) = arg.strip_prefix('-').filter(|s| !s.starts_with('-'))
                && let Some(at) = short.find('m')
                && short[..at].chars().all(|c| "aqsvn".contains(c))
            {
                if at + 1 == short.len() {
                    i += 1;
                }
                continue;
            }
        }
        if options_done {
            push_operand(&mut out, arg, false, here);
            continue;
        }
        if arg == "--" {
            options_done = true;
            continue;
        }
        if find && (arg == "(" || arg == "!" || (arg.starts_with('-') && arg.len() > 1)) {
            if matches!(arg.as_str(), "-H" | "-L" | "-P") {
                continue;
            }
            break;
        }
        if let Some(stripped) = arg.strip_prefix("--")
            && let Some((option, value)) = stripped.split_once('=')
        {
            let option = format!("--{option}");
            // A search option's value is a pattern or a glob, except the pattern
            // file, which is read.
            let not_a_path = search && SEARCH_VALUE_OPTIONS.contains(&option.as_str());
            if option == "--file" || !not_a_path {
                out.push(Candidate {
                    text: value.to_string(),
                    pattern: false,
                    expands: here,
                });
            }
            continue;
        }
        if arg.starts_with('-') && arg.len() > 1 {
            // A short option glued to its value (`grep -fsecret.txt`, `curl
            // -d@secret.txt`): the value may name a file.
            if !arg.starts_with("--")
                && let Some((at, _)) = arg.char_indices().nth(2)
            {
                push_operand(&mut out, &arg[at..], false, here);
            }
            if search && matches!(arg.as_str(), "-f" | "--file") {
                if let Some(value) = resolved.get(i) {
                    out.push(Candidate {
                        text: value.clone(),
                        pattern: false,
                        expands: expanding[i],
                    });
                }
                i += 1;
            } else if search && SEARCH_VALUE_OPTIONS.contains(&arg.as_str()) {
                i += 1;
            }
            continue;
        }
        let is_pattern = pattern_pending;
        pattern_pending = false;
        push_operand(&mut out, arg, is_pattern, here);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wall-clock bound for the cost tests. They guard against superlinear blowups
    /// (the release build answers these inputs in well under 100 ms); a debug build on a
    /// slow CI runner (the release workflow tests unoptimised) is given 15x the room.
    fn time_bound(secs: u64) -> std::time::Duration {
        let factor = if cfg!(debug_assertions) { 15 } else { 1 };
        std::time::Duration::from_secs(secs * factor)
    }
    use crate::core::test_isolation;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn inner(cmd: &str) -> Option<RtkInner> {
        let (words, spans) = lexer::words_and_spans(cmd);
        rtk_inner(cmd, &words, &spans)
    }

    fn texts(cmd: &str) -> Vec<String> {
        inner(cmd).expect("an rtk command").texts
    }

    // --- rtk inner extraction ---

    #[test]
    fn rtk_inner_table() {
        // (command, inner texts, opaque)
        let cases: &[(&str, &[&str], bool)] = &[
            ("rtk git push", &["git push"], false),
            ("rtk -v git push", &["git push"], false),
            ("rtk -vvv --skip-env git push", &["git push"], false),
            ("rtk --verbose git push", &["git push"], false),
            ("/some/dir/rtk git push", &["git push"], false),
            (
                "rtk git push --force origin main",
                &["git push --force origin main"],
                false,
            ),
            ("rtk curl http://x", &["curl http://x"], false),
            (
                "rtk kubectl apply -f x.yaml",
                &["kubectl apply -f x.yaml"],
                false,
            ),
            ("rtk bypass git push --force", &["git push --force"], false),
            ("rtk proxy git push", &["git push"], false),
            ("rtk run git push", &["git push"], false),
            ("rtk run -c 'git push'", &["git push"], false),
            ("rtk run -c\"git push\"", &["git push"], false),
            ("rtk run -cgit", &["git"], false),
            ("rtk run --command 'git push'", &["git push"], false),
            ("rtk run --command='git push'", &["git push"], false),
            ("rtk run --shell bash -c 'git push'", &["git push"], false),
            ("rtk run --shell=zsh -c 'git push'", &["git push"], false),
            ("rtk run -c 'git push' --shell sh", &["git push"], false),
            ("rtk run rtk run git push", &["rtk run git push"], false),
            ("rtk err git push", &["git push"], false),
            ("rtk test git push", &["git push"], false),
            ("rtk summary git push", &["git push"], false),
            (
                "rtk err --shell bash 'git push'",
                &["'git push'", "git push"],
                false,
            ),
            (
                "rtk test --shell=sh 'git push'",
                &["'git push'", "git push"],
                false,
            ),
            (
                "rtk read secret.txt",
                &["read secret.txt", "cat secret.txt"],
                false,
            ),
            ("rtk json a.json", &["json a.json", "cat a.json"], false),
            ("rtk log app.log", &["log app.log", "cat app.log"], false),
            ("rtk smart f.rs", &["smart f.rs", "cat f.rs"], false),
            ("rtk deps .", &["deps .", "cat ."], false),
            (
                "rtk grep foo a.txt",
                &["grep foo a.txt", "rg foo a.txt"],
                false,
            ),
            (
                "rtk rg foo a.txt",
                &["grep foo a.txt", "rg foo a.txt"],
                false,
            ),
            ("rtk diff a b", &["diff a b"], false),
            ("rtk wc -l a", &["wc -l a"], false),
            ("rtk ls -la", &["ls -la"], false),
            ("rtk find . -name x", &["find . -name x"], false),
            ("rtk tree src", &["tree src"], false),
            (
                "rtk git --skip-env push",
                &["git --skip-env push", "git push"],
                false,
            ),
            (
                "rtk run --shell fish -c 'git push'",
                &["fish -c git push", "git push"],
                true,
            ),
            (
                "rtk run --shell=pwsh -c 'git push'",
                &["pwsh -c git push", "git push"],
                true,
            ),
            ("rtk", &[], false),
            ("rtk -v", &[], false),
        ];
        for (cmd, expected, opaque) in cases {
            let got = inner(cmd).unwrap_or_else(|| panic!("{cmd} is an rtk command"));
            assert_eq!(got.texts, strings(expected), "texts of {cmd}");
            assert_eq!(got.opaque, *opaque, "opaque of {cmd}");
        }
    }

    #[test]
    fn rtk_inner_ignores_other_commands() {
        for cmd in ["git push", "echo rtk git push", "rtkx git push", "cat rtk"] {
            assert_eq!(inner(cmd), None, "{cmd}");
        }
    }

    // --- shell wrapper scripts ---

    #[test]
    fn shell_wrapper_script_forms() {
        let script = |cmd: &str| {
            let (words, _) = lexer::words_and_spans(cmd);
            shell_wrapper_script(&words)
        };
        assert_eq!(script("bash -c 'git push'").as_deref(), Some("git push"));
        assert_eq!(script("bash -lc \"git push\"").as_deref(), Some("git push"));
        assert_eq!(script("sh -e -c 'a; b'").as_deref(), Some("a; b"));
        assert_eq!(script("zsh -o pipefail -c x").as_deref(), Some("x"));
        assert_eq!(
            script("bash --norc -c 'git push'").as_deref(),
            Some("git push")
        );
        assert_eq!(
            script("fish --command 'git push'").as_deref(),
            Some("git push")
        );
        assert_eq!(script("bash -c -- 'git push'").as_deref(), Some("git push"));
        assert_eq!(script("bash script.sh"), None);
        assert_eq!(script("bash"), None);
        assert_eq!(script("bash -x script.sh"), None);
    }

    // --- the gate over Bash rules ---

    fn rules(deny: &[&str], ask: &[&str], read: &[&str]) -> GateRules {
        rules_with_read_ask(deny, ask, read, &[])
    }

    fn rules_with_read_ask(
        deny: &[&str],
        ask: &[&str],
        read: &[&str],
        read_ask: &[&str],
    ) -> GateRules {
        GateRules::new(
            strings(deny),
            strings(ask),
            strings(read),
            strings(read_ask),
            &[],
            Vec::new(),
        )
    }

    fn gate(cmd: &str, rules: &GateRules) -> Gate {
        evaluate(
            cmd,
            rules,
            PermissionVerdict::Default,
            Some("/work/project"),
        )
    }

    #[test]
    fn deny_reaches_through_every_rtk_form() {
        let r = rules(
            &["git push:*", "curl:*"],
            &["kubectl apply:*", "aws:*"],
            &[],
        );
        for cmd in [
            "git push",
            "curl x",
            "rtk curl x",
            "rtk git push --force origin main",
            "rtk bypass git push --force",
            "rtk proxy git push",
            "rtk run git push",
            "rtk run -c 'git push'",
            "rtk run --shell bash -c 'git push'",
            "rtk err git push",
            "rtk test git push",
            "rtk summary git push",
            "rtk -v git push",
            "rtk --skip-env git push",
            "FOO=1 rtk git push",
            "timeout 5 rtk git push",
            "git status && rtk git push",
            "/some/dir/rtk git push",
            "bash -c 'git push'",
            "rtk run rtk run git push",
            "rtk run -c \"bash -c 'git push'\"",
            "bash -c \"rtk git push\"",
            "sh -c 'echo a; git push'",
            "env HUSKY=0 rtk git push",
            "nohup rtk git push",
            "rtk git --skip-env push",
            "( rtk git push )",
            "{ rtk git push; }",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Deny, "{cmd}");
        }
    }

    #[test]
    fn ask_rule_reaches_through_rtk_and_wrappers() {
        let r = rules(&["git push:*"], &["kubectl apply:*", "aws:*"], &[]);
        for cmd in [
            "kubectl apply -f x.yaml",
            "rtk kubectl apply -f x.yaml",
            "aws s3 rm s3://b --recursive",
            "rtk run aws s3 ls",
            "bash -c 'kubectl apply -f x'",
            "rtk bypass aws s3 ls",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Ask, "{cmd}");
        }
        // A deny beats an ask anywhere in the command.
        assert_eq!(gate("aws s3 ls && rtk git push", &r), Gate::Deny);
    }

    #[test]
    fn unrelated_commands_pass() {
        let r = rules(&["git push:*"], &["kubectl apply:*"], &["./secret.txt"]);
        for cmd in [
            "git status",
            "rtk git status",
            "rtk run git status",
            "rtk run -c 'git log'",
            "bash -c 'git status'",
            "echo hi",
            "cat ok.txt",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Pass, "{cmd}");
        }
    }

    #[test]
    fn inert_without_any_rule() {
        let r = rules(&[], &[], &[]);
        for cmd in ["curl x", "rtk run -c 'git $(echo push)'", "cat secret.txt"] {
            assert_eq!(gate(cmd, &r), Gate::Pass, "{cmd}");
        }
    }

    #[test]
    fn a_deny_inside_a_wrapper_verdict_is_a_deny() {
        let r = rules(&[], &[], &[]);
        assert_eq!(
            evaluate("x", &r, PermissionVerdict::Defer, None),
            Gate::Deny
        );
        assert_eq!(evaluate("x", &r, PermissionVerdict::Deny, None), Gate::Deny);
    }

    #[test]
    fn opaque_inner_commands_ask_when_a_rule_exists() {
        let r = rules(&["git push:*"], &[], &[]);
        for cmd in [
            "rtk run -c 'git $(echo push)'",
            "rtk run -c 'git `echo push`'",
            "rtk bypass echo <(date)",
            "rtk run --shell fish -c 'git status'",
            "rtk run --shell=pwsh -c 'git status'",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Ask, "{cmd}");
        }
        // Plain file redirects are not opaque.
        assert_eq!(gate("rtk run -c 'git status > out.txt'", &r), Gate::Pass);
        assert_eq!(gate("rtk git log > out.txt", &r), Gate::Pass);
        // A top-level substitution keeps today's behaviour.
        assert_eq!(gate("echo $(date)", &r), Gate::Pass);
        assert_eq!(gate("bash -c 'echo $(date)'", &r), Gate::Pass);
    }

    #[test]
    fn depth_cap_asks() {
        let r = rules(&["git push:*"], &[], &[]);
        let plain = |n: usize| format!("{}git status", "rtk run ".repeat(n));
        assert_eq!(gate(&plain(2), &r), Gate::Pass);
        assert_eq!(gate(&plain(4), &r), Gate::Pass);
        assert_eq!(gate(&plain(5), &r), Gate::Ask);
        // A deny is still found at the deepest level the gate reads.
        assert_eq!(
            gate(&format!("{}git push", "rtk run ".repeat(4)), &r),
            Gate::Deny
        );
    }

    #[test]
    fn fail_closed_ask_star_asks_every_command() {
        let r = rules(&[], &["*"], &[]);
        assert_eq!(gate("git status", &r), Gate::Ask);
    }

    // --- apply ---

    #[test]
    fn apply_turns_every_rewrite_into_a_hook_ask() {
        let allow = HookDecision::AllowRewrite("rtk git status".into());
        assert_eq!(
            apply(Gate::Ask, "git status", allow, &rules(&[], &[], &[])),
            HookDecision::AskRewrite("rtk git status".into(), AskOwner::Hook)
        );
        let host = HookDecision::AskRewrite("rtk git status".into(), AskOwner::Host);
        assert_eq!(
            apply(Gate::Ask, "git status", host, &rules(&[], &[], &[])),
            HookDecision::AskRewrite("rtk git status".into(), AskOwner::Hook)
        );
        assert_eq!(
            apply(
                Gate::Ask,
                "htop",
                HookDecision::Defer,
                &rules(&[], &[], &[])
            ),
            HookDecision::AskRewrite("htop".into(), AskOwner::Hook)
        );
        assert_eq!(
            apply(
                Gate::Pass,
                "htop",
                HookDecision::Defer,
                &rules(&[], &[], &[])
            ),
            HookDecision::Defer
        );
        assert_eq!(
            apply(Gate::Ask, "x", HookDecision::Deny, &rules(&[], &[], &[])),
            HookDecision::Deny
        );
    }

    #[test]
    fn deny_output_has_no_updated_input() {
        let out = deny_output();
        assert_eq!(
            out.to_string(),
            r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"RTK: a deny rule in the user's, project's or managed Claude Code settings matches the command or a command it runs."}}"#
        );
    }

    // --- Read() rules: anchors ---

    fn matcher(patterns: &[&str], ci: bool) -> ReadMatcher {
        ReadMatcher::new(
            &strings(patterns),
            Some(PathBuf::from("/home/u")),
            &[PathBuf::from("/proj")],
            Some(Path::new("/proj/sub")),
            ci,
        )
    }

    fn denied(m: &ReadMatcher, path: &str) -> bool {
        m.denied(&normalize(Path::new(path)), false)
    }

    #[test]
    fn read_anchor_double_slash_is_absolute() {
        let m = matcher(&["//etc/shadow"], false);
        assert!(denied(&m, "/etc/shadow"));
        assert!(!denied(&m, "/proj/etc/shadow"));
        assert!(!denied(&m, "/etc/passwd"));
    }

    #[test]
    fn read_anchor_tilde_is_home() {
        let m = matcher(&["~/.ssh/**"], false);
        assert!(denied(&m, "/home/u/.ssh/id_rsa"));
        assert!(denied(&m, "/home/u/.ssh/keys/a"));
        assert!(!denied(&m, "/home/other/.ssh/id_rsa"));
        assert!(!denied(&m, "/proj/.ssh/id_rsa"));
    }

    #[test]
    fn read_anchor_slash_is_project_root() {
        let m = matcher(&["/secrets/*.json"], false);
        assert!(denied(&m, "/proj/secrets/a.json"));
        assert!(!denied(&m, "/proj/sub/secrets/a.json"));
        assert!(!denied(&m, "/proj/secrets/a.txt"));
    }

    #[test]
    fn read_anchor_relative_is_cwd() {
        let m = matcher(&["./secret.txt", "conf/key.pem"], false);
        assert!(denied(&m, "/proj/sub/secret.txt"));
        assert!(!denied(&m, "/proj/secret.txt"));
        assert!(denied(&m, "/proj/sub/conf/key.pem"));
        assert!(!denied(&m, "/proj/conf/key.pem"));
    }

    #[test]
    fn read_pattern_without_a_slash_matches_the_basename_at_any_depth() {
        let m = matcher(&[".env", "*.pem", "secrets/"], false);
        assert!(denied(&m, "/anywhere/deep/.env"));
        assert!(denied(&m, "/x/a.pem"));
        assert!(denied(&m, "/x/secrets/file"));
        assert!(!denied(&m, "/x/.env.example"));
        assert!(!denied(&m, "/x/a.pem.txt"));
    }

    #[test]
    fn read_double_star_crosses_directories() {
        let m = matcher(&["**/.env", "/src/**/gen/*.rs"], false);
        assert!(denied(&m, "/proj/sub/.env"));
        assert!(denied(&m, "/proj/sub/a/b/.env"));
        assert!(!denied(&m, "/proj/other/.env"));
        assert!(denied(&m, "/proj/src/a/b/gen/x.rs"));
        assert!(denied(&m, "/proj/src/gen/x.rs"));
        assert!(!denied(&m, "/proj/src/gen/x.py"));
    }

    #[test]
    fn read_denied_directory_denies_what_is_under_it() {
        let m = matcher(&["./private"], false);
        assert!(denied(&m, "/proj/sub/private"));
        assert!(denied(&m, "/proj/sub/private/a/b.txt"));
        assert!(!denied(&m, "/proj/sub/privateer"));
    }

    #[test]
    fn read_bare_read_denies_everything() {
        let m = matcher(&["**"], false);
        let cwds = Cwds::new(Some(PathBuf::from("/proj")));
        assert_eq!(
            m.check_command("cat", &["x"], &cwds, false),
            ReadOutcome::Denied
        );
        assert_eq!(
            m.check_command("ls", &[], &cwds, false),
            ReadOutcome::Denied
        );
    }

    #[test]
    fn read_glob_classes_and_question_marks() {
        let m = matcher(&["key?.pem", "[ab]*.txt", "x[!0-9]"], false);
        assert!(denied(&m, "/p/key1.pem"));
        assert!(!denied(&m, "/p/key12.pem"));
        assert!(denied(&m, "/p/a1.txt"));
        assert!(denied(&m, "/p/b.txt"));
        assert!(!denied(&m, "/p/c.txt"));
        assert!(denied(&m, "/p/xa"));
        assert!(!denied(&m, "/p/x1"));
    }

    #[test]
    fn read_case_folds_where_the_platform_does() {
        let sensitive = matcher(&["./Secret.txt", ".ENV"], false);
        assert!(!denied(&sensitive, "/proj/sub/secret.txt"));
        assert!(!denied(&sensitive, "/x/.env"));
        let folded = matcher(&["./Secret.txt", ".ENV"], true);
        assert!(denied(&folded, "/proj/sub/secret.txt"));
        assert!(denied(&folded, "/x/.env"));
    }

    // --- Read() rules: commands ---

    fn read_rules() -> GateRules {
        rules(&[], &[], &["./secret.txt", "**/.env"])
    }

    /// `cwd` is the sandbox directory; the rules anchor to it.
    fn in_sandbox<R>(f: impl FnOnce(&Path, &GateRules) -> R) -> R {
        let tmp = test_isolation::tempdir();
        let dir = std::fs::canonicalize(tmp.path()).expect("canonical tempdir");
        std::fs::create_dir_all(dir.join("sub")).expect("mkdir sub");
        std::fs::create_dir_all(dir.join("config")).expect("mkdir config");
        std::fs::write(dir.join("secret.txt"), "s").expect("write secret");
        std::fs::write(dir.join("ok.txt"), "o").expect("write ok");
        std::fs::write(dir.join("config/.env"), "e").expect("write env");
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.join("secret.txt"), dir.join("link")).expect("symlink");
        let r = read_rules();
        f(&dir, &r)
    }

    fn gate_in(dir: &Path, cmd: &str, r: &GateRules) -> Gate {
        evaluate(
            cmd,
            r,
            PermissionVerdict::Default,
            Some(&dir.to_string_lossy()),
        )
    }

    #[test]
    fn read_commands_on_a_denied_path_are_denied() {
        in_sandbox(|dir, r| {
            for cmd in [
                "cat secret.txt",
                "head -n 5 secret.txt",
                "tail secret.txt",
                "cat ./x/../secret.txt",
                "rtk read secret.txt",
                "rtk run cat secret.txt",
                "cat config/.env",
                "grep foo secret.txt",
                "cat link",
                "cat ok.txt secret.txt",
                "cd sub && cat ../secret.txt",
                "wc -l secret.txt",
                "diff ok.txt secret.txt",
                "ls secret.txt",
                "find secret.txt -type f",
                "rtk json secret.txt",
                "rtk grep foo secret.txt",
                "bash -c 'cat secret.txt'",
                "cat --number=1 secret.txt",
                "cat -- secret.txt",
                "cat < secret.txt",
                "rtk run -c 'cat < secret.txt'",
                "cd config && cat .env",
                "(cd sub; cd ..); cat secret.txt",
            ] {
                assert_eq!(gate_in(dir, cmd, r), Gate::Deny, "{cmd}");
            }
        });
    }

    #[test]
    fn read_commands_on_other_paths_pass() {
        in_sandbox(|dir, r| {
            for cmd in [
                "wc -l < /dev/null",
                "cat ok.txt < /dev/null",
                "cat ok.txt",
                "head -n 5 ok.txt",
                "grep foo ok.txt",
                "grep 'a.*b' ok.txt",
                "rg --glob '*.rs' foo ok.txt",
                "find . -name '*.rs'",
                "ls",
                "ls -la sub",
                "rtk read ok.txt",
                "echo secret.txt",
            ] {
                assert_eq!(gate_in(dir, cmd, r), Gate::Pass, "{cmd}");
            }
            // A `cd` the shared parser followed adds nothing outside rtk; the rule
            // scan follows it itself.
            assert_eq!(gate_in(dir, "cd sub && cat ok.txt", r), Gate::Pass);
        });
    }

    #[test]
    fn uncertain_read_paths_ask_at_top_level_and_inside_a_wrapper() {
        in_sandbox(|dir, r| {
            // Round 6: a top-level defer is no longer used for an unresolvable path.
            for cmd in [
                "cat *.txt",
                "cat $F",
                "cat `x`",
                "cat {a,b}",
                "cat sec?et.txt",
                "cat [s]ecret.txt",
                "cat < sec?et.txt",
                "cat < $PWD/secret.txt",
            ] {
                assert_eq!(gate_in(dir, cmd, r), Gate::Ask, "{cmd}");
            }
            for cmd in ["rtk read *.txt", "rtk run cat *.txt", "bash -c 'cat *.txt'"] {
                assert_eq!(gate_in(dir, cmd, r), Gate::Ask, "{cmd}");
            }
            // An unknown cwd makes a relative path uncertain.
            assert_eq!(gate_in(dir, "cd $X && cat ok.txt", r), Gate::Ask);
            assert_eq!(gate_in(dir, "cd - && cat ok.txt", r), Gate::Ask);
        });
    }

    #[test]
    fn a_denied_path_beats_an_uncertain_one() {
        in_sandbox(|dir, r| {
            assert_eq!(gate_in(dir, "cat *.txt secret.txt", r), Gate::Deny);
        });
    }

    #[test]
    fn read_deny_in_a_subshell_cd_is_still_seen() {
        in_sandbox(|dir, r| {
            // After `(cd sub)` the real cwd is back at the top, so secret.txt there is denied.
            assert_eq!(gate_in(dir, "(cd sub) ; cat secret.txt", r), Gate::Deny);
            // Inside the cd'd directory a bare name is not the denied file.
            assert_eq!(gate_in(dir, "cd sub && cat other.txt", r), Gate::Pass);
        });
    }

    // --- RG1: the shared parser (anthology_shell::analyze) ---

    /// What the guard's parser sees run is denied, at top level too: heredocs and
    /// here-strings fed to a shell, `echo | sh`, `eval`, substitutions, launchers
    /// and exec payloads. The rule scan alone asked about some of these, or let the
    /// host judge the original.
    #[test]
    fn what_the_shared_parser_sees_run_is_denied() {
        let r = bash_rules();
        for cmd in [
            "bash <<EOF\ngit push\nEOF",
            "bash <<'EOF'\ngit push\nEOF",
            "sh <<< 'git push'",
            "echo 'git push' | sh",
            "printf 'git push\\n' | bash -s",
            "eval 'git push'",
            "echo $(git push)",
            "echo \"$(curl x)\"",
            "timeout 5 git push",
            "nice -n 5 git push",
            "find . -maxdepth 0 -exec git push \\;",
            "rtk run -c 'bash <<EOF\ngit push\nEOF'",
            "rtk bypass bash <<< 'git push'",
            "rtk bypass watch git push",
            "rtk git $'push'",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Deny, "{cmd}");
        }
        // Ask rules too.
        for cmd in ["bash <<EOF\nkubectl apply -f x\nEOF", "eval 'aws s3 ls'"] {
            assert_eq!(gate(cmd, &r), Gate::Ask, "{cmd}");
        }
        // Text that only looks like a command is not one.
        for cmd in [
            "echo 'git push'",
            "git commit -m 'git push later'",
            "grep -rn 'git push' docs",
        ] {
            assert_ne!(gate(cmd, &r), Gate::Deny, "{cmd}");
        }
    }

    #[test]
    fn a_read_the_shared_parser_sees_is_denied() {
        in_sandbox(|dir, r| {
            for cmd in [
                "bash <<EOF\ncat secret.txt\nEOF",
                "sh <<< 'head -1 secret.txt'",
                "echo 'cat < secret.txt' | sh",
                "eval 'cat secret.txt'",
                "timeout 5 cat secret.txt",
                "rtk run -c 'bash <<EOF\ncat config/.env\nEOF'",
            ] {
                assert_eq!(gate_in(dir, cmd, r), Gate::Deny, "{cmd}");
            }
            for cmd in ["bash <<EOF\ncat ok.txt\nEOF", "eval 'cat ok.txt'"] {
                assert_eq!(gate_in(dir, cmd, r), Gate::Pass, "{cmd}");
            }
        });
    }

    /// An analysis cut short by a limit asks, whatever the line.
    #[test]
    fn an_incomplete_analysis_asks() {
        let r = bash_rules();
        let scan = |analysis: &Analysis| {
            let mut scanner = Scanner::new(&r, None, "x");
            let denied = scanner.scan_analysis("x", analysis, None).is_err();
            (denied, scanner.ask)
        };
        let cut = Analysis {
            incomplete: true,
            ..Analysis::default()
        };
        assert_eq!(scan(&cut), (false, true));
        assert_eq!(scan(&Analysis::default()), (false, false));
        // A real one: the heredoc regex over its backtrack limit.
        let limits = anthology_shell::Limits { backtrack_limit: 0 };
        let text = "echo $(cat <<\"EOF'\nx\n)";
        assert!(!analyze(text, "/h").incomplete);
        let analysis = anthology_shell::analyze_with(text, "/h", &limits);
        assert!(analysis.incomplete);
        assert_eq!(scan(&analysis), (false, true));
    }

    /// Opaque behind rtk asks; outside rtk the host judges the original.
    #[test]
    fn an_opaque_entry_asks_only_in_a_line_that_may_run_rtk() {
        let r = bash_rules();
        for cmd in [
            "rtk bypass ssh h ls -la",
            "rtk bypass xargs git status",
            "rtk uv run python script.py",
            "rtk run -c 'FOO=$BAR true'",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Ask, "{cmd}");
        }
        for cmd in ["ssh h ls -la", "python3 script.py", "xargs git status"] {
            assert_eq!(gate(cmd, &r), Gate::Pass, "{cmd}");
        }
    }

    #[test]
    fn the_plain_shape_needs_the_shared_parser_to_agree() {
        let agrees = |line: &str, words_of: &str| {
            shared_parser_agrees(line, &plain_words(words_of).expect("plain"))
        };
        for line in [
            "rtk ls",
            "rtk git log --oneline -n 5",
            "rtk git commit -m 'feat!: x $y'",
            "rtk read \"a b\"",
            "rtk git log HEAD~5..HEAD",
            "rtk ls ~/x",
        ] {
            assert!(agrees(line, line), "{line}");
        }
        // Different words, a different count, or a second command: no.
        assert!(!agrees("rtk ls a", "rtk ls b"));
        assert!(!agrees("rtk ls a", "rtk ls a b"));
        assert!(!agrees("rtk ls a; rtk ls b", "rtk ls a"));
        assert!(!agrees("rtk ls a > x", "rtk ls a"));
    }

    #[test]
    fn a_literal_word_keeps_its_meaning_when_quoted_back() {
        for word in [
            "$(id)",
            "a b",
            "it's",
            "x*y",
            "`id`",
            "a\\b",
            "plain-word_1.txt",
        ] {
            let quoted = quote_literal(word);
            assert_eq!(resolve_word_text(&quoted), word, "{quoted}");
            assert!(!is_expanding(&quoted), "{quoted}");
        }
        assert_eq!(quote_literal("~/x"), "~/x");
        assert_eq!(quote_literal("HEAD~1"), "HEAD~1");
    }

    #[test]
    fn the_shared_parser_is_inert_without_rules() {
        let none = rules(&[], &[], &[]);
        for cmd in [
            "bash <<EOF\ngit push\nEOF",
            "eval 'git push'",
            "rtk bypass ssh h ls",
            "rtk ls '$(id)'",
        ] {
            assert_eq!(gate(cmd, &none), Gate::Pass, "{cmd}");
        }
    }

    // --- settings loading ---

    fn write(dir: &Path, name: &str, content: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&path, content).expect("write");
        path
    }

    #[test]
    fn append_read_deny_keeps_read_entries_only() {
        let value = json!([
            "Read(./secret.txt)",
            "Read",
            "Bash(git push:*)",
            "Edit(./x)",
            "Read(//etc/shadow)"
        ]);
        let mut target = Vec::new();
        append_read_rules(Some(&value), &mut target);
        assert_eq!(target, strings(&["./secret.txt", "**", "//etc/shadow"]));
        append_read_rules(None, &mut target);
        assert_eq!(target.len(), 3);
    }

    #[test]
    fn managed_paths_list_the_file_then_the_sorted_drop_ins() {
        let tmp = test_isolation::tempdir();
        write(tmp.path(), "managed-settings.d/20-b.json", "{}");
        write(tmp.path(), "managed-settings.d/10-a.json", "{}");
        write(tmp.path(), "managed-settings.d/notes.txt", "x");
        let paths = managed_settings_paths(tmp.path());
        let names: Vec<String> = paths
            .iter()
            .map(|p| {
                p.strip_prefix(tmp.path())
                    .expect("under the dir")
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(
            names,
            strings(&[
                "managed-settings.json",
                "managed-settings.d/10-a.json",
                "managed-settings.d/20-b.json"
            ])
        );
    }

    #[test]
    fn managed_rules_only_reads_the_flag() {
        let tmp = test_isolation::tempdir();
        let off = write(tmp.path(), "a.json", r#"{"permissions":{}}"#);
        let on = write(
            tmp.path(),
            "b.json",
            r#"{"allowManagedPermissionRulesOnly": true}"#,
        );
        let bad = write(tmp.path(), "c.json", "{");
        assert!(!managed_rules_only(&[off.clone(), bad.clone()]));
        assert!(managed_rules_only(&[off, on, bad]));
        assert!(!managed_rules_only(&[tmp.path().join("missing.json")]));
    }

    #[test]
    fn assemble_adds_the_env_project_only_when_absolute_and_new() {
        let managed = Path::new("/nonexistent-managed-dir");
        let base = vec![PathBuf::from("/p/.claude/settings.json")];
        let same = SettingsFiles::assemble(
            base.clone(),
            Some(PathBuf::from("/p")),
            Some(PathBuf::from("/p")),
            managed,
        );
        assert_eq!(same.other, base);
        let relative = SettingsFiles::assemble(
            base.clone(),
            Some(PathBuf::from("/p")),
            Some(PathBuf::from("rel")),
            managed,
        );
        assert_eq!(relative.other, base);
        let other = SettingsFiles::assemble(
            base.clone(),
            Some(PathBuf::from("/p")),
            Some(PathBuf::from("/q")),
            managed,
        );
        assert_eq!(other.other.len(), 3);
        assert_eq!(other.other[1], PathBuf::from("/q/.claude/settings.json"));
        assert_eq!(
            other.other[2],
            PathBuf::from("/q/.claude/settings.local.json")
        );
        assert_eq!(
            other.project_roots,
            vec![PathBuf::from("/p"), PathBuf::from("/q")]
        );
    }

    // --- settings loading, end to end under the test build's isolated dirs ---

    /// A machine in a scratch directory: managed settings, a user Claude dir and
    /// a project, all resolved through `user_dirs`/`user_env`.
    struct Machine {
        managed: PathBuf,
        claude: PathBuf,
        project: PathBuf,
    }

    fn with_machine<R>(f: impl FnOnce(&Machine) -> R) -> R {
        let tmp = test_isolation::tempdir();
        let root = tmp.path().join("home");
        let machine = Machine {
            managed: root.join("managed"),
            claude: tmp.path().join("claude-dir"),
            project: tmp.path().join("project"),
        };
        std::fs::create_dir_all(&machine.managed).expect("mkdir managed");
        std::fs::create_dir_all(&machine.claude).expect("mkdir claude");
        std::fs::create_dir_all(machine.project.join(".claude")).expect("mkdir project");
        test_isolation::with_root(&root, || {
            crate::core::user_env::with_path("CLAUDE_CONFIG_DIR", Some(&machine.claude), || {
                let _entered = test_isolation::enter(&machine.project);
                f(&machine)
            })
        })
    }

    fn settings(deny: &[&str], ask: &[&str], allow: &[&str]) -> String {
        json!({"permissions": {"deny": deny, "ask": ask, "allow": allow}}).to_string()
    }

    fn hook(cmd: &str) -> HookDecision {
        let permissions = permissions::check_claude_hook_command(cmd);
        super::super::decision::decide_for_claude_hook_at(cmd, &permissions, None)
    }

    #[test]
    fn managed_settings_deny_applies_past_an_rtk_prefix() {
        with_machine(|m| {
            write(
                &m.managed,
                "managed-settings.json",
                &settings(&["Bash(curl:*)"], &[], &[]),
            );
            write(
                &m.project.join(".claude"),
                "settings.json",
                &settings(&[], &[], &["Bash(rtk *)"]),
            );
            assert_eq!(hook("curl x"), HookDecision::Deny);
            assert_eq!(hook("rtk curl x"), HookDecision::Deny);
            assert_eq!(hook("rtk run -c 'curl x'"), HookDecision::Deny);
            assert_eq!(
                hook("git status"),
                HookDecision::AskRewrite("rtk git status".into(), AskOwner::Host)
            );
        });
    }

    #[test]
    fn managed_drop_in_applies() {
        with_machine(|m| {
            write(
                &m.managed,
                "managed-settings.d/10-net.json",
                &settings(&["Bash(wget:*)"], &["Bash(kubectl apply:*)"], &[]),
            );
            assert_eq!(hook("rtk wget x"), HookDecision::Deny);
            assert_eq!(
                hook("kubectl apply -f x"),
                HookDecision::AskRewrite("kubectl apply -f x".into(), AskOwner::Hook)
            );
        });
    }

    #[test]
    fn managed_rules_only_drops_the_other_files_rules() {
        with_machine(|m| {
            write(
                &m.managed,
                "managed-settings.json",
                r#"{"allowManagedPermissionRulesOnly": true,
                    "permissions": {"deny": ["Bash(curl:*)"]}}"#,
            );
            write(
                &m.project.join(".claude"),
                "settings.json",
                &settings(
                    &["Bash(git push:*)"],
                    &["Bash(aws:*)"],
                    &["Bash(git status)"],
                ),
            );
            write(
                &m.claude,
                "settings.json",
                &settings(&[], &[], &["Bash(git log:*)"]),
            );

            // The user's allow no longer grants anything: the default ask stands.
            assert_eq!(
                hook("git status"),
                HookDecision::AskRewrite("rtk git status".into(), AskOwner::Host)
            );
            assert_eq!(
                hook("git log -3"),
                HookDecision::AskRewrite("rtk git log -3".into(), AskOwner::Host)
            );
            // ...nor does the user's deny or ask rule bind.
            assert_ne!(hook("git push origin main"), HookDecision::Deny);
            // `aws` is not in the plain shape: its rewrite is dropped and the host judges
            // the original (and asks, by default).
            assert_eq!(hook("aws s3 ls"), HookDecision::Defer);
            // The managed deny does.
            assert_eq!(hook("curl x"), HookDecision::Deny);
        });
    }

    #[test]
    fn without_the_flag_every_file_contributes() {
        with_machine(|m| {
            write(
                &m.managed,
                "managed-settings.json",
                &settings(&["Bash(curl:*)"], &[], &[]),
            );
            write(
                &m.project.join(".claude"),
                "settings.json",
                &settings(&["Bash(git push:*)"], &[], &["Bash(git status)"]),
            );
            assert_eq!(hook("curl x"), HookDecision::Deny);
            assert_eq!(hook("rtk git push"), HookDecision::Deny);
            assert_eq!(
                hook("git status"),
                HookDecision::AllowRewrite("rtk git status".into())
            );
        });
    }

    #[test]
    fn an_unreadable_managed_file_fails_closed() {
        with_machine(|m| {
            // A directory where the file should be: it exists but cannot be read.
            std::fs::create_dir_all(m.managed.join("managed-settings.json")).expect("mkdir");
            let permissions = permissions::check_claude_hook_command("git status");
            assert_eq!(permissions.verdict, PermissionVerdict::Ask);
            assert_eq!(
                permissions.deny_rules(),
                crate::hooks::permissions::DenyRules::Unattested
            );
            assert_eq!(
                hook("git status"),
                HookDecision::AskRewrite("rtk git status".into(), AskOwner::Hook)
            );
        });
    }

    #[test]
    fn an_unparseable_managed_drop_in_fails_closed() {
        with_machine(|m| {
            write(&m.managed, "managed-settings.d/10-bad.json", "{ not json");
            let permissions = permissions::check_claude_hook_command("git status");
            assert_eq!(permissions.verdict, PermissionVerdict::Ask);
            assert_eq!(
                permissions.deny_rules(),
                crate::hooks::permissions::DenyRules::Unattested
            );
        });
    }

    #[test]
    fn claude_project_dir_adds_a_project_root() {
        with_machine(|m| {
            let other = m.project.parent().expect("parent").join("elsewhere");
            write(
                &other.join(".claude"),
                "settings.json",
                &settings(&["Bash(git push:*)", "Read(./secret.txt)"], &[], &[]),
            );
            assert_ne!(hook("rtk git push"), HookDecision::Deny);
            crate::core::user_env::with_path("CLAUDE_PROJECT_DIR", Some(&other), || {
                assert_eq!(hook("rtk git push"), HookDecision::Deny);
            });
            // A relative value is not a project root.
            crate::core::user_env::with_path("CLAUDE_PROJECT_DIR", Some("elsewhere"), || {
                assert_ne!(hook("rtk git push"), HookDecision::Deny);
            });
        });
    }

    #[test]
    fn read_deny_rules_load_from_every_source_and_gate_the_hook() {
        with_machine(|m| {
            write(
                &m.managed,
                "managed-settings.json",
                &settings(&["Read(//etc/shadow)"], &[], &[]),
            );
            write(
                &m.project.join(".claude"),
                "settings.local.json",
                &settings(
                    &["Read(./secret.txt)", "Bash(git push:*)"],
                    &[],
                    &["Bash(rtk *)"],
                ),
            );
            let permissions = permissions::check_claude_hook_command("cat x");
            assert_eq!(
                permissions.gate.read_deny,
                strings(&["//etc/shadow", "./secret.txt"])
            );
            assert_eq!(hook("cat secret.txt"), HookDecision::Deny);
            assert_eq!(hook("rtk read secret.txt"), HookDecision::Deny);
            assert_eq!(hook("cat //etc/shadow"), HookDecision::Deny);
            assert_eq!(
                hook("cat ok.txt"),
                HookDecision::AskRewrite("rtk read ok.txt".into(), AskOwner::Host)
            );
            assert_eq!(
                hook("cat *.txt"),
                HookDecision::AskRewrite("cat *.txt".into(), AskOwner::Hook)
            );
        });
    }

    // --- review round 1: every reproducer of the review ---

    fn bash_rules() -> GateRules {
        rules(
            &["git push:*", "curl:*"],
            &["kubectl apply:*", "aws:*"],
            &[],
        )
    }

    #[test]
    fn quoted_words_are_read_as_the_shell_reads_them() {
        let r = bash_rules();
        for cmd in [
            "rtk git 'push' --force origin main",
            "rtk git \"push\"",
            "rtk bypass git p'u'sh",
            "rtk run -c 'git pu\\sh'",
            // The top level too: the extra reading can only be stricter.
            "git 'push'",
            "g\"it\" push",
            "timeout 5 git 'push'",
            "bash -c \"git 'push'\"",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Deny, "{cmd}");
        }
        for cmd in [
            "rtk kubectl 'apply' -f x.yaml",
            "rtk run kubectl \"apply\" -f x.yaml",
            "kubectl \"apply\" -f x.yaml",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Ask, "{cmd}");
        }
    }

    #[test]
    fn a_single_string_argv_is_also_a_script() {
        let r = bash_rules();
        for cmd in [
            "rtk run 'git push --force'",
            "rtk test 'git push'",
            "rtk err 'git push'",
            "rtk summary 'git push'",
            "rtk run 'echo ok; curl x'",
            "rtk bypass 'git push'",
            "rtk proxy 'echo ok; git push'",
            "rtk 'git push'",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Deny, "{cmd}");
        }
        // A lone word that is only a program name stays harmless.
        assert_eq!(gate("rtk run ls", &r), Gate::Pass);
        assert_eq!(gate("rtk test cargo", &r), Gate::Pass);
    }

    #[test]
    fn exec_payloads_are_read_at_top_level_and_behind_rtk() {
        let r = bash_rules();
        for cmd in [
            "find . -exec git push \\;",
            "find . -execdir git push \\;",
            "find . -ok git push \\;",
            "find . -okdir git push {} +",
            "find . -exec curl x \\;",
            "find . -type f -exec sh -c 'git push' \\;",
            "rtk find . -exec git push \\;",
            "rtk bypass xargs -n1 git push",
            "xargs -I{} -P 4 git push",
            "xargs -0 -t -- git push",
            "rtk run env -i git push",
            "rtk run env -u X git push",
            "env -i -u X git push",
            "env FOO=1 -- git push",
            "env -C /tmp git push",
            "env -S 'git push'",
            "env --unset=X git push",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Deny, "{cmd}");
        }
        assert_eq!(
            gate("find . -exec kubectl apply -f {} \\;", &r),
            Gate::Ask,
            "an ask rule on a payload"
        );
        assert_eq!(gate("find . -exec echo hi \\;", &r), Gate::Pass);
        assert_eq!(gate("xargs", &r), Gate::Pass);
        assert_eq!(gate("env", &r), Gate::Pass);
    }

    #[test]
    fn an_exec_payload_the_gate_cannot_attest_defers_a_top_level_find() {
        let r = bash_rules();
        for cmd in [
            "find . -exec sh {} \\;",
            "find . -exec git status",
            "find . -exec \\;",
            "xargs --frobnicate git status",
            "env --frobnicate git status",
            "find . -exec python3 -c 'print(1)' \\;",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Defer, "{cmd}");
        }
        // A payload that expands may become a ruled command: the gate asks.
        assert_eq!(gate("find . -exec $CMD {} \\;", &r), Gate::Ask);
        // Behind rtk the same asks.
        assert_eq!(gate("rtk find . -exec sh {} \\;", &r), Gate::Ask);
        // Read-only rules still care: the payload may read a denied file.
        let no_bash = rules(&[], &[], &["./secret.txt"]);
        assert_eq!(gate("find . -exec sh {} \\;", &no_bash), Gate::Defer);
    }

    #[test]
    fn opaque_commands_inside_rtk_ask() {
        let r = bash_rules();
        for cmd in [
            "rtk run -c 'g=git; $g push'",
            "rtk bypass python3 -c 'import os; os.system(\"curl x\")'",
            "rtk bypass python3.11 -c 'x'",
            "rtk bypass python -Sc 'x'",
            "rtk bypass busybox sh -c 'curl x'",
            "rtk bypass sudo git status",
            "rtk bypass doas git status",
            "rtk bypass toybox sh",
            "rtk bypass node -e 'x'",
            "rtk bypass node --print 1",
            "rtk bypass perl -e 'x'",
            "rtk bypass perl -lne 'x'",
            "rtk bypass ruby -e 'x'",
            "rtk bypass php -r 'x'",
            "rtk bypass lua -e 'x'",
            "rtk bypass osascript -e 'x'",
            "rtk bypass deno eval 'x'",
            "rtk bypass bun -e 'x'",
            "rtk bypass rg --pre ./x foo .",
            "rtk bypass grep --pre=./x foo .",
            "rtk bypass source ./x.sh",
            "rtk bypass . ./x.sh",
            "rtk bypass ./x* arg",
            "rtk run -c 'bash script.sh'",
            "rtk bypass python3 script.py",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Ask, "{cmd}");
        }
        // The same constructs outside rtk keep today's behaviour, and the
        // ordinary ones inside it are not opaque.
        for cmd in [
            "eval 'git status'",
            "python3 -c 'x'",
            "bash script.sh",
            "rtk run -c '[ -f x ] && echo y'",
            "rtk run -c 'echo hi'",
        ] {
            assert_ne!(gate(cmd, &r), Gate::Ask, "{cmd}");
        }
        // No rule at all: nothing to be opaque about.
        let none = rules(&[], &[], &[]);
        assert_eq!(gate("rtk bypass python3 -c 'x'", &none), Gate::Pass);
        // RG1: anthology_shell::analyze reads what runs; stricter than before (Deny).
        for cmd in [
            "rtk run eval 'git push'",
            "rtk run -c 'eval \"git push\"'",
            "echo 'curl x' | rtk bypass bash",
            "echo 'curl x' | rtk bypass sh -s",
            "rtk bypass bash <<< 'curl x'",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Deny, "{cmd}");
        }
        // RG1: anthology_shell::analyze reads what runs; stricter than before (Ask).
        for cmd in ["rtk run -c 'FOO=$BAR true'"] {
            assert_eq!(gate(cmd, &r), Gate::Ask, "{cmd}");
        }
    }

    #[test]
    fn rtk_inner_single_word_table() {
        assert_eq!(
            inner("rtk run 'git push'").expect("rtk").texts,
            strings(&["'git push'", "git push"])
        );
        assert_eq!(
            inner("rtk test 'git push'").expect("rtk").texts,
            strings(&["'git push'", "git push"])
        );
        assert_eq!(
            inner("rtk bypass 'git push'").expect("rtk").texts,
            strings(&["'git push'", "git push"])
        );
        assert_eq!(
            inner("rtk run git push").expect("rtk").texts,
            strings(&["git push"])
        );
    }

    #[test]
    fn exec_payload_extraction_table() {
        let payloads = |cmd: &str| {
            let (words, spans) = lexer::words_and_spans(cmd);
            let name = basename(&resolve_word_text(words[0])).to_string();
            exec_payloads(&name, cmd, &words, &spans)
        };
        let p = payloads("find . -exec git push {} \\; -exec rm {} +");
        assert_eq!(p.texts, strings(&["git push {}", "rm {}"]));
        assert!(!p.unattested);
        let p = payloads("xargs -n 1 -P4 -I {} git push");
        assert_eq!(p.texts, strings(&["git push"]));
        let p = payloads("xargs");
        assert!(p.texts.is_empty() && !p.unattested);
        let p = payloads("env -i FOO=1 BAR=2 git push");
        assert_eq!(p.texts, strings(&["git push"]));
        let p = payloads("env -S 'git push' --force");
        assert_eq!(p.texts, strings(&["git push --force"]));
        assert!(payloads("env --weird git push").unattested);
        assert!(payloads("find . -exec git push").unattested);
    }

    // --- Read rules beyond the read commands ---

    #[test]
    fn read_deny_holds_for_every_program_inside_rtk() {
        in_sandbox(|dir, r| {
            for cmd in [
                "rtk sed -n p secret.txt",
                "rtk awk 1 secret.txt",
                "rtk less secret.txt",
                "rtk bypass sed -n p secret.txt",
                "rtk bypass base64 secret.txt",
                "rtk bypass xxd secret.txt",
                "rtk bypass cp secret.txt out.txt",
                "rtk bypass sort secret.txt",
                "rtk bypass jq -R . secret.txt",
                "rtk bypass sort --output=out.txt secret.txt",
                "rtk run -c 'sort <>secret.txt'",
                "sort <>secret.txt",
            ] {
                assert_eq!(gate_in(dir, cmd, r), Gate::Deny, "{cmd}");
            }
            // An operand that expands may name the file: ask. A plain relative
            // operand of a program that is no read adds no prompt, and a program
            // outside rtk is the host's own business.
            assert_eq!(gate_in(dir, "rtk bypass echo $HOME", r), Gate::Ask);
            for cmd in [
                "rtk bypass sed -n p ok.txt",
                "rtk git commit -m 'a*b'",
                "sed -n p secret.txt",
            ] {
                assert_eq!(gate_in(dir, cmd, r), Gate::Pass, "{cmd}");
            }
        });
    }

    #[test]
    fn bare_read_rule_does_not_deny_every_rtk_command() {
        let r = rules(&[], &[], &["**"]);
        assert_eq!(gate("rtk git status", &r), Gate::Pass);
        assert_eq!(gate("rtk bypass echo hi", &r), Gate::Pass);
        assert_eq!(gate("cat x", &r), Gate::Deny);
    }

    #[test]
    fn read_ask_rules_defer_at_top_level_and_ask_inside_rtk() {
        in_sandbox(|dir, _| {
            let r = rules_with_read_ask(&[], &[], &["./secret.txt"], &["./asked.txt"]);
            std::fs::write(dir.join("asked.txt"), "a").expect("write asked");
            for cmd in ["cat asked.txt", "head asked.txt", "cat < asked.txt"] {
                assert_eq!(gate_in(dir, cmd, &r), Gate::Defer, "{cmd}");
            }
            for cmd in [
                "rtk read asked.txt",
                "rtk sed -n p asked.txt",
                "bash -c 'cat asked.txt'",
                "rtk run cat asked.txt",
            ] {
                assert_eq!(gate_in(dir, cmd, &r), Gate::Ask, "{cmd}");
            }
            assert_eq!(gate_in(dir, "cat ok.txt", &r), Gate::Pass);
            assert_eq!(gate_in(dir, "cat secret.txt", &r), Gate::Deny);
            // Only an ask list of Read rules is enough to be active.
            let only_ask = rules_with_read_ask(&[], &[], &[], &["./asked.txt"]);
            assert_eq!(gate_in(dir, "cat asked.txt", &only_ask), Gate::Defer);
            assert_eq!(gate_in(dir, "rtk read *.txt", &only_ask), Gate::Ask);
        });
    }

    // --- HOME and CDPATH ---

    #[test]
    fn a_command_that_changes_home_makes_tilde_unknown() {
        let r = rules(&[], &[], &["~/.ssh/**"]);
        // The control: `~` is the home the hook sees.
        assert_eq!(gate("rtk read ~/.ssh/id_rsa", &r), Gate::Deny);
        for cmd in [
            "export HOME=/; rtk read ~/.ssh/id_rsa",
            "HOME=/ rtk run -c 'cat ~/.ssh/id_rsa'",
            "env HOME=/ rtk read ~/.ssh/id_rsa",
            "unset HOME; rtk read ~/.ssh/id_rsa",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Ask, "{cmd}");
        }
        // Round 6: an unresolvable path asks at top level too.
        assert_eq!(gate("HOME=/ cat ~/.ssh/id_rsa", &r), Gate::Ask);
        // A longer name is another variable.
        assert_eq!(gate("HOMEDIR=/ rtk read ~/.ssh/id_rsa", &r), Gate::Deny);
    }

    #[test]
    fn cdpath_makes_a_bare_relative_cd_unknown() {
        in_sandbox(|dir, r| {
            // No CDPATH anywhere: a relative cd is followed.
            assert_eq!(gate_in(dir, "cd sub && cat ../secret.txt", r), Gate::Deny);
            // Set by the command.
            assert_eq!(
                gate_in(dir, "export CDPATH=/x; cd sub && rtk read ok.txt", r),
                Gate::Ask
            );
            // Set in the hook's own environment.
            crate::core::user_env::with_path("CDPATH", Some("/x"), || {
                assert_eq!(gate_in(dir, "cd sub && rtk read ok.txt", r), Gate::Ask);
                assert_eq!(gate_in(dir, "cd sub && cat ok.txt", r), Gate::Ask);
                // `./` and `../` are never searched through CDPATH.
                assert_eq!(gate_in(dir, "cd ./sub && cat ../secret.txt", r), Gate::Deny);
                // The directory the command started in stays a possible one.
                assert_eq!(gate_in(dir, "cd .. && cat secret.txt", r), Gate::Deny);
            });
        });
    }

    // --- cost ---

    #[test]
    fn glob_matching_is_not_exponential() {
        let started = std::time::Instant::now();
        let m = matcher(&["**/*a*a*a*a*b.json", "*key*.json"], false);
        let long = "a".repeat(5000);
        let name = format!("/p/{long}");
        assert!(!denied(&m, &name));
        let long = "key".repeat(50_000);
        assert!(!denied(&m, &format!("/p/{long}")));
        assert!(denied(&m, &format!("/p/{long}.json")));
        assert!(started.elapsed() < time_bound(5), "{:?}", started.elapsed());
    }

    #[test]
    fn a_very_deep_path_does_not_overflow_the_stack() {
        let pattern = [
            Comp::Any,
            Comp::Literal("x".into()),
            Comp::Literal("y".into()),
        ];
        let path: Vec<String> = (0..200_000).map(|_| "a".to_string()).collect();
        assert!(!match_prefix(&pattern, &path));
        let mut hit = path;
        hit.push("x".into());
        hit.push("y".into());
        assert!(match_prefix(&pattern, &hit));
    }

    #[test]
    fn a_huge_read_operand_is_cheap() {
        // The review's case: a 300k-character word against a two-star rule used to
        // take over a minute.
        in_sandbox(|dir, _| {
            let r = rules(&[], &[], &["**/*key*.json"]);
            let started = std::time::Instant::now();
            let word = "key".repeat(100_000);
            assert_eq!(
                gate_in(dir, &format!("cat {word}; rtk git push"), &r),
                Gate::Pass
            );
            assert!(
                started.elapsed() < time_bound(10),
                "{:?}",
                started.elapsed()
            );
        });
    }

    #[test]
    fn an_exhausted_budget_asks_or_defers() {
        let many = "a;".repeat(5000);
        // Any Bash rule, or text that may hold an rtk form: ask.
        assert_eq!(gate(&many, &bash_rules()), Gate::Ask);
        let read_only = rules(&[], &[], &["./secret.txt"]);
        assert_eq!(
            gate(&format!("{many}rtk read secret.txt"), &read_only),
            Gate::Ask
        );
        // (Round 2: an exhausted budget asks whenever any rule exists.)
        assert_eq!(gate(&many, &read_only), Gate::Ask);
        // A few segments are fine.
        assert_eq!(gate("a;b;c", &bash_rules()), Gate::Pass);
    }

    #[test]
    fn an_unreadable_payload_asks() {
        let out = ask_output();
        assert_eq!(out["hookSpecificOutput"]["permissionDecision"], "ask");
        assert!(out["hookSpecificOutput"].get("updatedInput").is_none());
    }

    #[test]
    fn read_ask_rules_load_from_the_ask_list() {
        with_machine(|m| {
            write(
                &m.project.join(".claude"),
                "settings.json",
                &settings(
                    &["Read(./secret.txt)"],
                    &["Read(./asked.txt)", "Bash(aws:*)"],
                    &[],
                ),
            );
            let permissions = permissions::check_claude_hook_command("cat x");
            assert_eq!(permissions.gate.read_deny, strings(&["./secret.txt"]));
            assert_eq!(permissions.gate.read_ask, strings(&["./asked.txt"]));
            assert_eq!(hook("cat asked.txt"), HookDecision::Defer);
            assert_eq!(
                hook("rtk read asked.txt"),
                HookDecision::AskRewrite("rtk read asked.txt".into(), AskOwner::Hook)
            );
        });
    }

    // --- review round 2: backstops ---

    #[test]
    fn rule_heads_and_extendability() {
        assert_eq!(rule_head("git push:*"), Some(strings(&["git", "push"])));
        assert_eq!(rule_head("git push *"), Some(strings(&["git", "push"])));
        assert_eq!(rule_head("git push*"), Some(strings(&["git", "push"])));
        assert_eq!(rule_head("git push"), Some(strings(&["git", "push"])));
        assert_eq!(rule_head("aws:*"), Some(strings(&["aws"])));
        assert_eq!(rule_head("* push"), None);
        assert_eq!(rule_head("git * push"), None);
        assert_eq!(rule_head("*"), None);
        assert_eq!(rule_head(":*"), None);
        let r = bash_rules();
        assert!(r.extendable(&strings(&["git"])));
        assert!(r.extendable(&strings(&[])));
        assert!(r.extendable(&strings(&["git", "push", "--force"])));
        assert!(!r.extendable(&strings(&["git", "log", "--"])));
        assert!(!r.extendable(&strings(&["ls"])));
        assert!(rules(&[], &["*"], &[]).extendable(&strings(&["ls"])));
        assert!(rules(&["* --force"], &[], &[]).extendable(&strings(&["ls"])));
    }

    #[test]
    fn expanding_words() {
        for word in [
            "$x",
            "${X}",
            "\"$x\"",
            "`x`",
            "pu*h",
            "pus?",
            "pus[h]",
            "{push,}",
            "{1..3}",
            "$'push'",
            "$\"push\"",
            "~user",
            "a$b",
            "\"a$b\"",
        ] {
            assert!(is_expanding(word), "{word}");
        }
        for word in [
            "push", "'$x'", "'*.rs'", "\\$x", "\\*", "~", "~/x", "{}", "[", "[[", "a{b}c",
            "'{a,b}'", "-x", "--opt=v",
        ] {
            assert!(!is_expanding(word), "{word}");
        }
    }

    #[test]
    fn a_word_that_may_expand_into_a_rule_is_asked_or_deferred() {
        let r = bash_rules();
        for cmd in [
            "x=push; rtk git $x",
            "rtk git $X",
            "rtk git ${X}",
            "rtk git \"$x\"",
            "x=push rtk git $x",
            "rtk run git $x",
            "rtk bypass git $x",
            "rtk run -c 'x=push; git $x'",
            "rtk run -c 'git ${x:-push}'",
            "rtk run -c 'git pu${x}sh'",
            "rtk git {push,}",
            "rtk bypass git {push,}",
            "rtk git $'\\x70ush'",
            "rtk kubectl $'apply' -f x",
            "rtk kubectl $a -f x.yaml",
            "rtk git pu*h",
            "rtk git pus?",
            "rtk git pus[h]",
            "rtk $g push",
            "bash -c 'git $x'",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Ask, "{cmd}");
        }
        // At top level the host judges the original: no rewrite.
        for cmd in ["git $x", "git pu*h", "git {push,}", "$g push", "kubectl $a"] {
            assert_eq!(gate(cmd, &r), Gate::Defer, "{cmd}");
        }
        // Wildcard rules make any expansion extendable.
        let wild = rules(&["* --force"], &[], &[]);
        assert_eq!(gate("ls $x", &wild), Gate::Defer);
        // RG1: anthology_shell::analyze reads what runs; stricter than before (Deny).
        for cmd in [
            "rtk git $'push'",
            "rtk git $\"push\"",
            "rtk bypass git $'push'",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Deny, "{cmd}");
        }
    }

    #[test]
    fn expansions_that_no_rule_can_extend_keep_their_rewrite() {
        let only_push = rules(&["git push:*"], &[], &[]);
        for cmd in [
            "ls src/*.rs",
            "git log -- *.rs",
            "grep 'a.*b' f",
            "cat README.md",
            "git status",
            "git commit -m \"fix $x\"",
            "echo $HOME",
            "rtk git log -- *.rs",
            "rtk ls src/*.rs",
        ] {
            assert_eq!(gate(cmd, &only_push), Gate::Pass, "{cmd}");
        }
    }

    #[test]
    fn a_rule_head_embedded_anywhere_inside_rtk_asks() {
        let r = bash_rules();
        for cmd in [
            "rtk bypass xcrun git push",
            "rtk bypass script -q /dev/null git push",
            "rtk bypass sandbox-exec -p x git push",
            "rtk bypass flock /tmp/l git push",
            "rtk bypass chrt 0 git push",
            "rtk bypass taskset 1 git push",
            "rtk bypass su -c 'git push'",
            "rtk bypass ssh localhost git push",
            "rtk grep \"git push\" docs/",
            "rtk bypass echo curl x",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Ask, "{cmd}");
        }
        // The same words make the host judge a top-level rewrite itself.
        let rewrite =
            HookDecision::AskRewrite("rtk git log --grep='git push'".into(), AskOwner::Host);
        assert_eq!(
            apply(Gate::Pass, "git log --grep='git push'", rewrite, &r),
            HookDecision::Defer
        );
        let allow = HookDecision::AllowRewrite("rtk git log".into());
        assert_eq!(
            apply(Gate::Pass, "ssh h git push", allow, &r),
            HookDecision::Defer
        );
        assert_eq!(
            apply(
                Gate::Pass,
                "git status",
                HookDecision::AskRewrite("rtk git status".into(), AskOwner::Host),
                &r
            ),
            HookDecision::AskRewrite("rtk git status".into(), AskOwner::Host)
        );
        // RG1: anthology_shell::analyze reads what runs; stricter than before (Deny).
        for cmd in [
            "rtk bypass caffeinate git push",
            "rtk bypass caffeinate -i git push",
            "rtk bypass arch -arm64 git push",
            "rtk bypass watch git push",
            "rtk bypass setsid git push",
            "rtk bypass unbuffer git push",
            "rtk bypass ionice -c3 git push",
            "rtk bypass gtimeout 5 git push",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Deny, "{cmd}");
        }
    }

    #[test]
    fn shell_grammar_and_lowercase_assignments_are_peeled() {
        let r = bash_rules();
        for cmd in [
            "rtk run -c 'if true; then git push; fi'",
            "rtk run -c 'if git push; then :; fi'",
            "rtk run -c 'while git push; do break; done'",
            "rtk run -c 'until git push; do :; done'",
            "rtk run -c 'for i in 1; do git push; done'",
            "rtk run -c 'select i in 1; do git push; done'",
            "rtk run -c 'function f { git push; }; f'",
            "rtk run -c 'coproc git push'",
            "rtk run -c 'coproc NAME { git push; }'",
            "rtk run -c 'if true; then curl x; fi'",
            "rtk bypass bash -c 'if true; then git push; fi'",
            "rtk run -c 'x=1 git push'",
            "rtk run -c 'a=1 b=2 git push'",
            "rtk run -c 'A=1 b=2 git push'",
            "rtk run -c 'x=1 curl x'",
            "rtk run -c 'time -p git push'",
            "rtk run -c '! git push'",
            // Top level.
            "if true; then rtk git push; fi",
            "while true; do rtk git push; break; done",
            "for i in 1; do rtk git push; done",
            "until false; do rtk git push; break; done",
            "if rtk git push; then :; fi",
            "function f { rtk git push; }; f",
            "coproc rtk git push",
            "x=1 rtk git push",
            "a_b=1 rtk git push",
            "if true; then git push; fi",
            "x=1 git push",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Deny, "{cmd}");
        }
        assert_eq!(
            gate("rtk run -c 'if true; then kubectl apply -f x; fi'", &r),
            Gate::Ask
        );
        // A compound the segmenter leaves whole is opaque inside rtk.
        for cmd in [
            "rtk run -c 'case x in x) echo;; esac'",
            "rtk run -c '[[ -f x ]] && echo y'",
            "rtk run -c 'for i in 1 2; do echo $i; done'",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Ask, "{cmd}");
        }
    }

    /// Every remainder `grammar_step` reaches from `form`, step by step.
    fn grammar_peeled(form: &str) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        let mut at = 0;
        let mut queue = vec![form];
        while at < queue.len() && at < 32 {
            let current = queue[at];
            at += 1;
            let (words, spans) = lexer::words_and_spans(current);
            let mut step = Vec::new();
            grammar_step(current, &words, &spans, &mut step);
            for next in step {
                if !out.contains(&next) {
                    out.push(next);
                    queue.push(next);
                }
            }
        }
        out
    }

    #[test]
    fn grammar_peel_table() {
        assert_eq!(grammar_peeled("then git push"), vec!["git push"]);
        assert_eq!(
            grammar_peeled("x=1 y=2 time -p git push"),
            vec!["y=2 time -p git push", "time -p git push", "git push"]
        );
        assert_eq!(
            grammar_peeled("function f { git push"),
            vec!["{ git push", "git push"]
        );
        let coproc = grammar_peeled("coproc NAME { git push");
        assert!(coproc.contains(&"{ git push") && coproc.contains(&"git push"));
        assert!(grammar_peeled("git push").is_empty());
    }

    #[test]
    fn every_interpreter_and_odd_shell_is_opaque_inside_rtk() {
        let r = bash_rules();
        for cmd in [
            "rtk bypass python3 -X dev -c 'import os; os.system(\"git push\")'",
            "rtk bypass python3 -W ignore -c 'x'",
            "rtk bypass perl -I lib -e 'system(\"git push\")'",
            "rtk bypass ruby -I lib -e 'x'",
            "rtk bypass node -r fs -e 'x'",
            "rtk bypass php -d x=1 -r 'x'",
            "rtk bypass python3 <<< 'import os; os.system(\"git push\")'",
            "echo 'x' | rtk bypass python3 -",
            "echo 'x' | rtk bypass python3",
            "rtk bypass perl <<< 'system(\"git push\")'",
            "rtk bypass python3 script.py",
            "rtk bypass tcsh",
            "rtk bypass csh -f script.csh",
            "rtk bypass /bin/tcsh -f x",
            "rtk bypass pwsh -c 'git push'",
            "rtk bypass powershell -c 'x'",
            "rtk bypass expect -c 'spawn git push'",
            "rtk bypass tclsh <<< 'exec git push'",
            "rtk bypass Rscript -e 'system(\"git push\")'",
            "rtk bypass R -e 'x'",
            "rtk bypass julia -e 'x'",
            "rtk bypass trap 'git push' EXIT",
            "rtk bypass alias g=git",
            "rtk bypass python2.7 x.py",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Ask, "{cmd}");
        }
        // A parseable `-c` script is read like any shell's.
        for cmd in [
            "rtk bypass tcsh -c 'git push'",
            "rtk bypass csh -c 'git push'",
            "rtk bypass /bin/tcsh -c 'git push'",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Deny, "{cmd}");
        }
        // Read-only rules are rules too: an interpreter can read a denied file.
        let read_only = rules(&[], &[], &["./secret.txt"]);
        assert_eq!(gate("rtk bypass python3 -c 'x'", &read_only), Gate::Ask);
        assert_eq!(gate("find . -exec sh {} \\;", &read_only), Gate::Defer);
        // Outside rtk the same programs are left alone.
        assert_eq!(gate("python3 script.py", &r), Gate::Pass);
    }

    #[test]
    fn home_and_cdpath_changes_are_found_through_quotes_and_computed_names() {
        let r = rules(&[], &[], &["~/.ssh/**"]);
        for cmd in [
            "export HO\"ME\"=/; rtk read ~/.ssh/id_rsa",
            "export H\\OME=/; rtk read ~/.ssh/id_rsa",
            "v=HO; export ${v}ME=/; rtk read ~/.ssh/id_rsa",
            "declare ${v}ME=/; rtk read ~/.ssh/id_rsa",
            "typeset $n=/; rtk read ~/.ssh/id_rsa",
            "readonly \"$n\"=/; rtk read ~/.ssh/id_rsa",
            "local $n=/; rtk read ~/.ssh/id_rsa",
            "read $n; rtk read ~/.ssh/id_rsa",
            "printf -v $n /; rtk read ~/.ssh/id_rsa",
            "export HO'ME'=/ && rtk run -c 'cat ~/.ssh/id_rsa'",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Ask, "{cmd}");
        }
        // Ordinary exports do not.
        assert_eq!(gate("export FOO=1; rtk read ~/.ssh/id_rsa", &r), Gate::Deny);
        assert_eq!(gate("read line; rtk read ~/.ssh/id_rsa", &r), Gate::Deny);
        in_sandbox(|dir, r| {
            assert_eq!(
                gate_in(dir, "export CD\"PATH\"=/x; cd sub && rtk read ok.txt", r),
                Gate::Ask
            );
            assert_eq!(
                gate_in(dir, "export CD\\PATH=/x; cd sub && rtk read ok.txt", r),
                Gate::Ask
            );
            assert_eq!(
                gate_in(dir, "export ${v}=/x; cd sub && rtk read ok.txt", r),
                Gate::Ask
            );
        });
    }

    #[test]
    fn an_exhausted_budget_asks_whenever_a_rule_exists() {
        let many = "true;".repeat(5000);
        let read_only = rules(&[], &[], &["./secret.txt"]);
        for tail in [
            "\"r\"tk read secret.txt",
            "r\\tk read secret.txt",
            "rtk read secret.txt",
            "true",
        ] {
            assert_eq!(
                gate(&format!("{many}{tail}"), &read_only),
                Gate::Ask,
                "{tail}"
            );
        }
        assert_eq!(gate(&many, &bash_rules()), Gate::Ask);
    }

    #[test]
    fn a_non_read_program_asks_about_an_operand_that_expands() {
        in_sandbox(|dir, r| {
            for cmd in [
                "rtk bypass base64 secret.tx?",
                "rtk bypass base64 secre*",
                "rtk bypass base64 {secret,x}.txt",
                "f=secret.txt; rtk bypass base64 $f",
                "rtk bypass sed -n p \"$PWD/secret.txt\"",
                "rtk bypass base64 ~root/.ssh/id_rsa",
            ] {
                assert_eq!(gate_in(dir, cmd, r), Gate::Ask, "{cmd}");
            }
            // No expansion, no prompt: an unknown directory alone adds none.
            assert_eq!(gate_in(dir, "rtk bypass base64 ok.txt", r), Gate::Pass);
            assert_eq!(gate_in(dir, "rtk bypass echo '{}'", r), Gate::Pass);
            // `key=value` operands are paths too.
            assert_eq!(gate_in(dir, "rtk bypass dd if=secret.txt", r), Gate::Deny);
            assert_eq!(
                gate_in(dir, "rtk bypass dd of=/dev/stdout if=ok.txt", r),
                Gate::Pass
            );
            assert_eq!(gate_in(dir, "rtk bypass dd if=$x", r), Gate::Ask);
            // RG1: anthology_shell::analyze reads what runs; stricter than before (Deny).
            for cmd in ["rtk bypass base64 $'secret.txt'"] {
                assert_eq!(gate_in(dir, cmd, r), Gate::Deny, "{cmd}");
            }
        });
    }

    #[test]
    fn every_backstop_is_inert_without_rules() {
        let none = rules(&[], &[], &[]);
        for cmd in [
            "x=push; rtk git $x",
            "rtk bypass caffeinate git push",
            "if true; then rtk git push; fi",
            "rtk bypass python3 script.py",
            "rtk bypass base64 secre*",
            "rtk run -c 'case x in x) echo;; esac'",
            "export HO\"ME\"=/; rtk read ~/.ssh/id_rsa",
        ] {
            assert_eq!(gate(cmd, &none), Gate::Pass, "{cmd}");
        }
        let allow = HookDecision::AllowRewrite("rtk git log".into());
        assert_eq!(
            apply(Gate::Pass, "git log --grep='git push'", allow, &none),
            HookDecision::AllowRewrite("rtk git log".into())
        );
        // Allow-only configuration has no heads either.
        assert!(!none.embeds_head("git push"));
    }

    // --- review round 3 ---

    fn push_only() -> GateRules {
        rules(&["git push:*"], &[], &[])
    }

    #[test]
    fn command_words_are_normalized() {
        assert_eq!(norm_command("/usr/bin/git"), "git");
        assert_eq!(norm_command("=git"), "git");
        assert_eq!(norm_command("./x/git"), "git");
        assert_eq!(norm_command("git"), "git");
        if cfg!(any(target_os = "macos", windows)) {
            assert_eq!(norm_command("GIT"), "git");
        } else {
            assert_eq!(norm_command("GIT"), "GIT");
        }
    }

    #[test]
    fn git_global_options_are_skipped_to_the_subcommand() {
        let at = |text: &str| {
            let (words, _) = lexer::words_and_spans(text);
            let resolved: Vec<String> = words.iter().map(|w| resolve_word_text(w)).collect();
            git_subcommand_index(&resolved, &words, 0)
        };
        assert_eq!(at("git push"), 1);
        assert_eq!(at("git -C . push"), 3);
        assert_eq!(at("git -c a=b -C x push"), 5);
        assert_eq!(at("git --git-dir=.git --no-pager push"), 3);
        assert_eq!(at("git --git-dir .git push"), 3);
        assert_eq!(at("git -p -P --bare --paginate push"), 5);
        assert_eq!(at("git --exec-path push"), 2);
        assert_eq!(at("git -C $d push"), 3);
        assert_eq!(at("git $x push"), 1);
        assert_eq!(at("git"), 1);
        assert_eq!(at("git -C"), 2);
    }

    #[test]
    fn options_between_a_rules_words() {
        let r = bash_rules();
        let words = |text: &str| -> Vec<String> {
            let (w, _) = lexer::words_and_spans(text);
            w.iter().map(|w| resolve_word_text(w)).collect()
        };
        for hit in [
            "git -C x push",
            "git -c k=v push",
            "git --git-dir=x push",
            "kubectl -n prod apply -f x",
            "kubectl --context c apply",
            "/usr/bin/git -C . push",
            "=git -C . push",
        ] {
            assert!(r.subsequence_hit(&words(hit)), "{hit}");
        }
        for miss in [
            "git commit -m 'push fix'",
            "git checkout push-x",
            "git branch push",
            "git stash push -m wip",
            "git log --grep push",
            "git log -- push",
            "git status",
            "kubectl get pods",
            "echo git push",
            "git",
        ] {
            assert!(!r.subsequence_hit(&words(miss)), "{miss}");
        }
        // The same test over a whole text, from any word.
        for hit in [
            "ssh h kubectl -n x apply",
            "caffeinate git -C . push",
            "echo hi; env A=1 git -c a=b push",
        ] {
            assert!(r.embeds_head(hit), "{hit}");
        }
        for miss in [
            "git log -- *.rs",
            "git commit -m \"fix push\"",
            "grep 'a.*b' f",
            "cat README.md",
            "git status",
        ] {
            assert!(!r.embeds_head(miss), "{miss}");
        }
        // `extendable` skips options and their values.
        let prefix = |text: &str| words(text);
        assert!(r.extendable(&prefix("git -C .")));
        assert!(r.extendable(&prefix("git -c a=b")));
        assert!(r.extendable(&prefix("kubectl -n prod")));
        assert!(!r.extendable(&prefix("git commit -m")));
        assert!(!r.extendable(&prefix("git log --")));
        assert!(!r.extendable(&prefix("ls")));
    }

    #[test]
    fn embedded_heads_are_read_through_quotes_and_odd_separators() {
        let r = push_only();
        for text in [
            "caffeinate g''it push",
            "caffeinate gi\\t push",
            "caffeinate \"g\"it push",
            "ssh h \"g\"\"it push\"",
            "git -c 'alias.p=!git push' p",
            "x:git push",
            "/usr/bin/git push",
            "=git push",
        ] {
            assert!(r.embeds_head(text), "{text}");
        }
        assert!(!r.embeds_head("git status"));
        assert!(!r.embeds_head("echo gi t push"));
    }

    #[test]
    fn options_before_a_denied_subcommand_do_not_hide_it() {
        let r = bash_rules();
        for cmd in [
            "rtk git -C . push",
            "rtk -v git -C . push",
            "rtk git -c x.y=z push",
            "rtk git --no-pager push",
            "rtk git --git-dir=.git push",
            "rtk bypass git -C . push",
            "git -C . push origin main --force",
            "git -C sub push",
            "git --git-dir=.git push",
            "git --work-tree=. push",
            "git -c x.y=z push",
            "git --no-pager push",
            "GIT_DIR=.git git -C . push",
            "cd sub && git -C .. push",
            "git -C a -c b=c --no-pager push",
            "git --git-dir .git push",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Deny, "{cmd}");
        }
        for cmd in [
            "rtk kubectl --context c apply -f x.yaml",
            "rtk kubectl -n prod apply -f x.yaml",
            "rtk run -c 'kubectl -n prod apply -f x'",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Ask, "{cmd}");
        }
        assert_eq!(gate("kubectl -n prod apply -f x.yaml", &r), Gate::Defer);
        assert_eq!(gate("rtk git -C . $x", &push_only()), Gate::Ask);
        assert_eq!(gate("git -C . $x", &push_only()), Gate::Defer);
        // What no rule can reach stays as it was.
        let only = push_only();
        for cmd in [
            "git commit -m \"push fix\"",
            "git checkout push-x",
            "git -C . status",
            "rtk git -C . status",
            "rtk git commit -m \"docs: push\"",
            "rtk git log -- *.rs",
            "git log -- *.rs",
            "rtk ls src/*.rs",
            "rtk git commit -m $msg",
        ] {
            assert_eq!(gate(cmd, &only), Gate::Pass, "{cmd}");
        }
        // `apply` leaves a rewrite whose text holds options between a rule's words.
        let allow = HookDecision::AllowRewrite("rtk ssh h".into());
        assert_eq!(
            apply(Gate::Pass, "ssh h kubectl -n x apply", allow, &r),
            HookDecision::Defer
        );
        assert_eq!(
            apply(
                Gate::Pass,
                "git log -- *.rs",
                HookDecision::AllowRewrite("rtk git log -- *.rs".into()),
                &only
            ),
            HookDecision::AllowRewrite("rtk git log -- *.rs".into())
        );
    }

    #[test]
    fn a_crafted_run_of_grammar_words_costs_a_bounded_amount() {
        let r = bash_rules();
        let timed = |cmd: &str| {
            let started = std::time::Instant::now();
            let said = gate(cmd, &r);
            (said, started.elapsed())
        };
        let two_seconds = time_bound(2);
        let inside = format!(
            "rtk run -c '{}git push {}'",
            "time ".repeat(20_000),
            "x ".repeat(20_000)
        );
        let (said, took) = timed(&inside);
        assert_ne!(said, Gate::Pass);
        assert!(took < two_seconds, "inside rtk run -c: {took:?}");
        let top = format!(
            "{}rtk git status {}",
            "time ".repeat(20_000),
            "x ".repeat(20_000)
        );
        let (said, took) = timed(&top);
        assert_ne!(said, Gate::Pass);
        assert!(took < two_seconds, "top level: {took:?}");
        for cmd in [
            format!("{}git push", "then ".repeat(60_000)),
            format!("{}rtk git push", "a=1 ".repeat(60_000)),
            format!("rtk run -c '{}git push'", "! ".repeat(60_000)),
        ] {
            let (said, took) = timed(&cmd);
            assert_ne!(said, Gate::Pass);
            assert!(took < two_seconds, "{took:?}");
        }
        // The input cap, 1 MiB.
        let (said, took) = timed(&format!("{}git push", "time ".repeat(200_000)));
        assert_ne!(said, Gate::Pass);
        assert!(took < time_bound(10), "{took:?}");
        let (said, took) = timed(&format!("rtk run -c '{}git push'", "time ".repeat(200_000)));
        assert_ne!(said, Gate::Pass);
        assert!(took < time_bound(10), "{took:?}");
        // A long argument list is no reason to stall either.
        let (_, took) = timed(&format!("rtk bypass echo {}", "word ".repeat(100_000)));
        assert!(took < time_bound(10), "{took:?}");
    }

    #[test]
    fn a_command_word_is_read_by_its_basename() {
        let r = bash_rules();
        for cmd in [
            "rtk /usr/bin/git push",
            "rtk bypass /usr/bin/git push",
            "rtk =git push",
            "rtk bypass =git push",
            "rtk /usr/bin/curl x",
            "/usr/bin/git push",
            "=git push",
            "rtk run -c '/usr/bin/git push'",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Deny, "{cmd}");
        }
        // Where the file system folds case the program name is read folded.
        let folded = [
            "rtk GIT push",
            "rtk Git push",
            "rtk bypass GIT push",
            "rtk CURL x",
        ];
        for cmd in folded {
            let expected = if cfg!(any(target_os = "macos", windows)) {
                Gate::Deny
            } else {
                Gate::Pass
            };
            assert_eq!(gate(cmd, &r), expected, "{cmd}");
        }
        // RG1: anthology_shell::analyze reads what runs; stricter than before (Deny).
        for cmd in [
            "rtk bypass caffeinate =git push",
            "rtk bypass caffeinate /usr/bin/git push",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Deny, "{cmd}");
        }
    }

    #[test]
    fn a_denied_command_behind_an_unmodelled_launcher_asks() {
        let r = push_only();
        for cmd in [
            "rtk bypass ssh h \"g\"\"it push\"",
            "x=it; rtk bypass caffeinate g$x push",
            "rtk bypass caffeinate {git,} push",
            "rtk bypass caffeinate $'\\x67it' push",
            "rtk bypass caffeinate git pu*h",
            "rtk bypass caffeinate bash -c 'x=push; git $x'",
            "echo 'git push' | rtk bypass caffeinate python3",
            "rtk bypass ssh h 'x=push; git $x'",
            "rtk git -c 'alias.p=!git push' p",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Ask, "{cmd}");
        }
        // What the launcher runs is a question, never a deny, but an ordinary
        // command in the same place passes.
        for cmd in [
            "rtk bypass caffeinate git status",
            "rtk grep python src/",
            "rtk grep -rn node src/",
            "rtk git commit -m 'use python and node here'",
            "rtk git log --oneline -n 5",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Pass, "{cmd}");
        }
        // RG1: anthology_shell::analyze reads what runs; stricter than before (Deny).
        for cmd in [
            "rtk bypass caffeinate g''it push",
            "rtk bypass caffeinate gi\\t push",
            "rtk bypass caffeinate \"g\"it push",
            "rtk bypass exec -a x g''it push",
            "rtk bypass command -p g''it push",
            "rtk bypass caffeinate sh -c 'g\"\"it push'",
            "echo 'git push' | rtk bypass caffeinate bash",
            "rtk bypass caffeinate bash <<< 'g''it push'",
            "rtk bypass caffeinate eval 'g\"\"it push'",
            "rtk bypass env -i caffeinate g''it push",
            "rtk bypass caffeinate env g''it push",
            "rtk bypass caffeinate find . -maxdepth 0 -exec g''it push \\;",
            "rtk bypass npm exec -c 'g\"\"it push'",
            "rtk bypass watch -n1 'g\"\"it push'",
            "rtk bypass caffeinate git -C . push",
            "rtk bypass caffeinate /usr/bin/git push",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Deny, "{cmd}");
        }
        // RG1: anthology_shell::analyze reads what runs; stricter than before (Ask).
        for cmd in [
            "rtk bypass ssh h ls -la",
            "rtk bypass watch -n1 'git status'",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Ask, "{cmd}");
        }
    }

    #[test]
    fn arguments_from_stdin_or_the_filesystem_complete_a_head() {
        let r = push_only();
        for cmd in [
            "echo push | rtk bypass xargs git",
            "printf push | rtk bypass xargs -n1 git",
            "rtk bypass xargs -a args.txt git",
            "rtk bypass caffeinate xargs git <<< push",
            "touch push; rtk bypass find . -name push -execdir git {} \\;",
            "rtk bypass parallel git ::: push",
            "echo push | xargs git",
            "find . -execdir git {} \\;",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Ask, "{cmd}");
        }
        for cmd in [
            "xargs rm",
            "find . -name x -exec grep foo {} \\;",
            "find . -type f -exec git add {} \\;",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Pass, "{cmd}");
        }
        assert_eq!(gate("rtk bypass parallel git push ::: a", &r), Gate::Deny);
        // RG1: anthology_shell::analyze reads what runs; stricter than before (Ask).
        for cmd in [
            "rtk bypass xargs git status",
            "rtk bypass parallel echo ::: a b",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Ask, "{cmd}");
        }
    }

    #[test]
    fn every_round_three_backstop_is_inert_without_rules() {
        let none = rules(&[], &[], &[]);
        for cmd in [
            "rtk git -C . push",
            "rtk /usr/bin/git push",
            "rtk bypass caffeinate g''it push",
            "echo push | rtk bypass xargs git",
            "rtk bypass ssh h 'x=push; git $x'",
            "rtk run -c 'time time git push'",
        ] {
            assert_eq!(gate(cmd, &none), Gate::Pass, "{cmd}");
        }
        assert!(!none.embeds_head("ssh h git -C . push"));
    }

    // --- review round 4 ---

    #[test]
    fn a_bare_dot_or_a_word_after_a_data_program_is_not_a_command() {
        let r = push_only();
        for cmd in [
            "rtk npx prettier --check .",
            "rtk npx prettier --write .",
            "rtk npx eslint .",
            "rtk npx tsc -p .",
            "rtk ruff check .",
            "rtk ruff format .",
            "rtk mypy .",
            "rtk pytest .",
            "rtk go vet .",
            "rtk golangci-lint run .",
            "rtk pip install -e .",
            "rtk docker build -t app .",
            "rtk bypass rsync -a . /tmp/x",
            "rtk docker exec app python manage.py migrate",
            "rtk cargo build $FLAGS",
            "rtk run -c 'ruff check . && mypy .'",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Pass, "{cmd}");
        }
        // After a launcher, or with inline code or stdin, they are still commands.
        for cmd in [
            "rtk bypass caffeinate python3 script.py",
            "rtk bypass caffeinate bash",
            "rtk bypass nice bash script.sh",
            "rtk bypass ssh h python3 -c 'x'",
            "rtk docker exec app python -c 'x'",
            "rtk docker exec -i app bash",
            "rtk bypass xargs python3",
            "rtk pnpm exec python3 script.py",
            "rtk bypass caffeinate source x.sh",
            "rtk bypass caffeinate $c",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Ask, "{cmd}");
        }
        // RG1: anthology_shell::analyze reads what runs; stricter than before (Ask).
        for cmd in ["rtk uv run python script.py"] {
            assert_eq!(gate(cmd, &r), Gate::Ask, "{cmd}");
        }
    }

    #[test]
    fn a_script_a_launcher_runs_is_read_in_full() {
        in_sandbox(|dir, r| {
            for cmd in ["rtk bypass caffeinate sh -c 'f=secret; cat $f.txt'"] {
                // What only a launcher's argument list reaches is a question.
                assert_eq!(gate_in(dir, cmd, r), Gate::Ask, "{cmd}");
            }
            // Controls the gate already read in full.
            for cmd in [
                "rtk bypass sh -c 'cat secret.txt'",
                "rtk bypass nice sh -c 'cat secret.txt'",
                "rtk bypass caffeinate cat secret.txt",
            ] {
                assert_eq!(gate_in(dir, cmd, r), Gate::Deny, "{cmd}");
            }
            // A script that reads nothing denied passes.
            assert_eq!(
                gate_in(dir, "rtk bypass caffeinate sh -c 'cat ok.txt'", r),
                Gate::Pass
            );
            // RG1: anthology_shell::analyze reads what runs; stricter than before (Deny).
            for cmd in [
                "rtk bypass caffeinate sh -c 'cat secret.txt'",
                "rtk bypass caffeinate sh -c 'base64 -i secret.txt'",
                "rtk bypass caffeinate sh -c 'head secret.txt'",
                "rtk bypass caffeinate sh -c 'cat < secret.txt'",
                "rtk bypass caffeinate sh -c \"cat secret.txt; echo\"",
                "rtk bypass caffeinate sh -c 'cp secret.txt /tmp/x'",
                "rtk find . -maxdepth 0 -exec caffeinate sh -c 'cat secret.txt' \\;",
            ] {
                assert_eq!(gate_in(dir, cmd, r), Gate::Deny, "{cmd}");
            }
        });
        let r = push_only();
        for cmd in [
            "rtk bypass caffeinate sh -c 'echo Z2l0IHB1c2g= | base64 -d | sh'",
            "rtk bypass caffeinate sh -c 'python3 -c \"import os; os.system(chr(103)+\\\"it push\\\")\"'",
            "rtk bypass caffeinate sh -c 'cat /dev/stdin | sh' <<< 'git push'",
            "rtk bypass caffeinate bash -c 'source /dev/stdin <<< \"x\"'",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Ask, "{cmd}");
        }
        // After a data program the script stays a guess: rules only.
        assert_eq!(gate("rtk grep sh -c foo src/", &r), Gate::Pass);
    }

    #[test]
    fn a_double_dash_before_a_nested_command_is_dropped_as_rtk_drops_it() {
        let r = bash_rules();
        for cmd in [
            "rtk git -- push",
            "rtk git -C . -- push",
            "rtk git -- push --force origin main",
            "git -- push",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Deny, "{cmd}");
        }
        for cmd in ["rtk kubectl -- apply -f x.yaml", "rtk aws -- s3 ls"] {
            assert_eq!(gate(cmd, &r), Gate::Ask, "{cmd}");
        }
        let gh = rules(&["gh pr merge:*"], &[], &[]);
        assert_eq!(gate("rtk gh -- pr merge 1", &gh), Gate::Deny);
        // A `--` that ends options for the program still does.
        assert_eq!(gate("rtk git log -- push", &push_only()), Gate::Pass);
        assert_eq!(
            gate("rtk git checkout -- push.txt", &push_only()),
            Gate::Pass
        );
        let texts = inner("rtk git -- push").expect("rtk").texts;
        assert!(texts.contains(&"git push".to_string()), "{texts:?}");
    }

    #[test]
    fn git_told_to_run_something_else_asks_when_a_rule_is_about_git() {
        let r = push_only();
        for cmd in [
            "rtk git -c alias.p=push p",
            "PUSHVAR=push rtk git --config-env=alias.p=PUSHVAR p",
            "GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=alias.p GIT_CONFIG_VALUE_0=push rtk git p",
            "rtk git -c 'alias.p=!echo Z2l0IHB1c2g= | base64 -d | sh' p",
            "rtk git -c diff.external='sh -c \"eval \\$X\"' diff",
            "rtk git -c core.pager='x' log",
            "rtk git -c core.sshCommand=x fetch",
            "rtk git -c credential.helper='!x' fetch",
            "rtk git -c protocol.ext.allow=always clone x",
            "rtk git -c filter.x.textconv=y diff",
            "rtk git -c $k=v p",
            "rtk run -c 'GIT_SSH_COMMAND=x git fetch'",
            "rtk run -c 'GIT_PAGER=x git log'",
            "rtk run -c 'GIT_EXTERNAL_DIFF=x git diff'",
            "rtk run -c 'env GIT_EDITOR=x git commit'",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Ask, "{cmd}");
        }
        // Top level the host judges the original.
        assert_eq!(gate("git -c alias.p=push p", &r), Gate::Defer);
        assert_eq!(gate("GIT_PAGER=x git log", &r), Gate::Defer);
        // Ordinary config is left alone, and so is a rule set that is not about git.
        for cmd in [
            "rtk git -c color.ui=never diff",
            "git -c color.ui=never diff",
            "rtk git -C . -c user.name=x commit -m y",
            "GIT_DIR=.git git status",
            "rtk run -c 'GIT_DIR=.git git status'",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Pass, "{cmd}");
        }
        let curl_only = rules(&["curl:*"], &[], &[]);
        assert_eq!(gate("rtk git -c alias.p=push p", &curl_only), Gate::Pass);
    }

    #[test]
    fn only_options_may_stand_between_a_rules_words() {
        let r = push_only();
        for cmd in [
            "git stash push -m wip",
            "git stash push",
            "git log --grep push",
            "git branch push",
            "rtk git stash push -m wip",
            "rtk git log --grep push",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Pass, "{cmd}");
        }
        for cmd in [
            "git -C . push",
            "git --bare push",
            "git -c a=b -C x push",
            "rtk git --bare push",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Deny, "{cmd}");
        }
        let kube = rules(&[], &["kubectl apply:*"], &[]);
        assert_eq!(gate("rtk kubectl -n prod apply -f x", &kube), Gate::Ask);
        assert_eq!(gate("rtk kubectl get pods apply", &kube), Gate::Pass);
        assert!(kube.embeds_head("ssh h kubectl --context c apply"));
        assert!(!kube.embeds_head("ssh h kubectl get apply"));
    }

    #[test]
    fn every_round_four_backstop_is_inert_without_rules() {
        let none = rules(&[], &[], &[]);
        for cmd in [
            "rtk git -- push",
            "rtk git -c alias.p=push p",
            "GIT_CONFIG_COUNT=1 rtk git p",
            "rtk bypass caffeinate sh -c 'cat secret.txt'",
            "rtk pnpm exec cat secret.txt",
        ] {
            assert_eq!(gate(cmd, &none), Gate::Pass, "{cmd}");
        }
    }

    // --- round 7: the plain shape ---

    #[test]
    fn the_plain_shape_passes_listed_rtk_commands_only() {
        let r = rules(&["git push:*"], &["kubectl apply:*"], &[]);
        let at = |cmd: &str| plain(cmd, &r, Some("/work/project"));
        for cmd in [
            "rtk git status",
            "rtk git status -sb",
            "rtk git log --oneline -5",
            "rtk git log --format='%h %s' -n 3",
            "rtk git log --grep push",
            "rtk git diff HEAD~1",
            "rtk git show HEAD^",
            "rtk git diff --cached -- src/main.rs",
            "rtk git add -A",
            "rtk git add .",
            "rtk git commit -m 'fix: push handler'",
            "rtk git commit -m 'Revert \"push handler\"'",
            "rtk git checkout feature/x",
            "rtk git branch -a",
            "rtk ls",
            "rtk ls -la src",
            "rtk ls ~/projects",
            "rtk tree -L 2 .",
            "rtk read README.md",
            "rtk read -n --max-lines 40 src/main.rs",
            "rtk wc -l src/main.rs",
            "rtk grep -rn push src/",
            "rtk grep -rn 'fn main(' src",
            "rtk rg -i -A3 todo src",
            "rtk find . -name '*.rs'",
            "rtk find src -type f -name '*.rs' -maxdepth 2",
            "rtk cargo test",
            "rtk cargo test --release -p rtk",
            "rtk cargo clippy --all-targets",
            "rtk cargo fmt --all --check",
            "rtk cargo build --release",
            // Round 8: every listed sub, and the needless asks of round 7.
            "rtk git blame -L 10,20 src/main.rs",
            "rtk git switch -c feature/x",
            "rtk git rev-parse --show-toplevel",
            "rtk git rev-parse --short HEAD",
            "rtk git ls-files -m",
            "rtk diff a.txt b.txt",
            "rtk cargo check --all-targets",
            "rtk cargo build -r",
            "rtk cargo test -r",
            "rtk git status -uno",
            "rtk git status --untracked-files=no",
            "rtk git diff -U5 HEAD",
            "rtk git diff --unified=1",
            "rtk git show --pretty=oneline HEAD",
            "rtk git log --pretty HEAD~3",
            "rtk git commit -m 'feat!: breaking'",
            "rtk git commit -m 'costs $5'",
            "rtk git commit -m 'a `b` c\\d'",
            "rtk git commit -am \"fix: x\"",
            "rtk find . '!' -name x",
            "rtk git log HEAD~5..HEAD",
            "rtk git diff --stat HEAD~1..HEAD",
            "rtk git log main^..HEAD",
            "rtk ls ~",
            "rtk ls ~/x",
        ] {
            let got = crate::core::user_env::with_vars(&[("HOME", Some("/home/u"))], || at(cmd));
            assert_eq!(got, Gate::Pass, "{cmd}");
        }
        // HOME unset, empty or relative: a `~` names nothing the hook can check.
        for home in [None, Some(""), Some("../home"), Some("home")] {
            for cmd in ["rtk ls ~", "rtk ls ~/x", "rtk read ~/notes.txt"] {
                let got = crate::core::user_env::with_vars(&[("HOME", home)], || at(cmd));
                assert_eq!(got, Gate::Ask, "{cmd} with HOME={home:?}");
            }
        }
        for cmd in [
            // Not one simple command, or something the shell expands or redirects.
            "rtk git status; ls",
            "rtk git status && rtk ls",
            "rtk ls | head",
            "rtk ls &",
            "rtk ls\nrtk bypass git push",
            "rtk ls > out",
            "< secret.txt rtk read",
            "rtk read < secret.txt",
            "rtk read 3<<<x",
            "rtk read <<EOF",
            "(rtk ls)",
            "{ rtk ls; }",
            "rtk ls $HOME",
            "rtk ls ${x}",
            "rtk ls $(pwd)",
            "rtk ls `pwd`",
            "rtk ls \"$x\"",
            "rtk ls src/*.rs",
            "rtk ls src/?.rs",
            "rtk ls src/[ab].rs",
            "rtk ls {a,b}",
            "rtk ls ~root",
            "rtk ls ~''/x",
            "rtk ls a~b",
            "rtk ls '~/x' ~x",
            "rtk ls =ls",
            "rtk ls # x",
            "rtk ls !x",
            "rtk ls a\\ b",
            "X=1 rtk ls",
            "nice rtk ls",
            "RTK ls",
            "rtk 'ls",
            // Not on the list, or an option it does not know.
            "rtk bypass ls",
            "rtk run -c ls",
            "rtk proxy ls",
            "rtk err ls",
            "rtk kubectl get pods",
            "rtk docker ps",
            "rtk dotnet build",
            "rtk go test ./...",
            "rtk npm run build",
            "rtk npx prettier --check .",
            "rtk prettier --check .",
            "rtk pytest",
            "rtk git -C . status",
            "rtk git -c alias.x=y status",
            "rtk git push",
            "rtk git stash push",
            "rtk git log --output=x",
            "rtk git diff --ext-diff",
            "rtk git show HEAD:secret.txt",
            "rtk git log -- a:b",
            "rtk git commit -F msg.txt",
            "rtk grep -f patterns.txt src",
            "rtk grep --pre=x y .",
            "rtk rg -z x .",
            "rtk find . -exec rm {} +",
            "rtk find . -delete",
            "rtk find . -fprint out",
            "rtk find . -newer x",
            "rtk cargo test --config x",
            "rtk cargo +nightly test",
            "rtk cargo test --target ./t.json",
            "rtk cargo test -- --logfile x",
            // Round 8, I-1: an optional git value is glued only, and no git word
            // but a commit message holds a `:`.
            "rtk git show --pretty HEAD:secret.txt",
            "rtk git log --pretty HEAD:secret.txt",
            "rtk git log --format HEAD:secret.txt",
            "rtk git diff -U HEAD:secret.txt --unified HEAD:ok.txt",
            "rtk git status --untracked-files HEAD:secret.txt",
            "rtk git status -u HEAD:x",
            "rtk git show --pretty=format:%h HEAD",
            "rtk git log --author=a:b",
            "rtk git log --grep x:y",
            "rtk git branch --contains HEAD:x",
            "rtk git diff -UHEAD:x",
            // I-2: after cargo's `--` nothing passes; no word opens with `@`.
            "rtk cargo clippy -- -D warnings",
            "rtk cargo clippy -- -D unknown-lints -D @secret.txt",
            "rtk cargo test -- --nocapture",
            "rtk cargo fmt -- --check",
            "rtk cargo test @x",
            "rtk cargo build --features=@x",
            "rtk cargo build -p@x",
            "rtk read @x",
            "rtk grep -rn @x .",
            // M-2: git add -f.
            "rtk git add -f config",
            "rtk git add --force config",
            "rtk git add -Af .",
            // M-3: a separate value never opens with `-`.
            "rtk cargo build --bin --config=build.rustc-wrapper=/tmp/x",
            "rtk cargo test --test --config=x",
            "rtk cargo build -p --config=x",
            "rtk cargo build --features --config=x",
            "rtk git log -n --output=x",
            "rtk tree -L -o x",
            "rtk find . -name -x",
            // Quotes still matter where the shell expands.
            "rtk git commit -m \"costs $5\"",
            "rtk git commit -m \"wow!\"",
            "rtk git log HEAD~x",
            "rtk cargo run",
            "rtk ls --colo",
            // A rule's text anywhere.
            "rtk ls 'git push'",
            "rtk grep -rn 'kubectl  apply' src",
            // Descriptors and standard input.
            "rtk read -",
            "rtk read /dev/stdin",
            "rtk read //dev/fd/0",
            "rtk read /./proc/self/fd/0",
            "rtk read /proc/self/cwd/x",
            "rtk read /sys/x",
            "rtk read ../../../../../../../../dev/stdin",
            "rtk diff - x",
            "rtk read --max-lines=/dev/stdin x",
            "rtk grep -A/dev/stdin x .",
        ] {
            assert_ne!(at(cmd), Gate::Pass, "{cmd}");
        }
    }

    /// APFS and NTFS fold case: `/DEV/stdin` is `/dev/stdin` there (not on Linux).
    #[cfg(any(target_os = "macos", windows))]
    #[test]
    fn a_descriptor_path_in_another_case_asks_where_case_folds() {
        let r = rules(&["git push:*"], &[], &[]);
        assert_eq!(
            plain("rtk read /DEV/stdin", &r, Some("/work/project")),
            Gate::Ask
        );
    }

    /// A directory too large to scan for `x~1`'s possible names asks (`None`).
    #[test]
    fn a_stem_scan_past_its_cap_gives_up() {
        let tmp = test_isolation::tempdir();
        for name in ["a1", "a2", "b"] {
            std::fs::write(tmp.path().join(name), "x").expect("write");
        }
        let found = stem_matches("a", Some(tmp.path()), false, 10).expect("under the cap");
        assert_eq!(found.len(), 3, "{found:?}");
        assert_eq!(
            stem_matches("a", Some(tmp.path()), false, 3).map(|v| v.len()),
            Some(3)
        );
        assert_eq!(stem_matches("a", Some(tmp.path()), false, 2), None);
    }

    #[test]
    fn plain_paths_meet_the_read_rules() {
        in_sandbox(|dir, r| {
            let at = |cmd: &str| plain(cmd, r, Some(&dir.to_string_lossy()));
            for cmd in [
                "rtk read secret.txt",
                "rtk read ./sub/../secret.txt",
                "rtk read link",
                "rtk read config/.env",
                "rtk git diff -- secret.txt",
                "rtk git log secret^..HEAD",
                "rtk git log -p secret^1",
                "rtk git log secret~1",
                "rtk wc -l secret.txt",
                "rtk grep -rn x secret.txt",
                "rtk grep -e x -- secret.txt",
            ] {
                assert_eq!(at(cmd), Gate::Deny, "{cmd}");
            }
            // R8-1: under zsh's EXTENDED_GLOB these name `config/.env` or a file
            // under the home; only a strict revision word may hold `^`, `~` or `#`.
            for cmd in [
                "rtk git log -p c^1./.env",
                "rtk git log -p c^1../.env",
                "rtk git show HEAD -- c^1./.env",
                "rtk git diff HEAD~1 c^1./.env",
                "rtk git log -p ~/x/secret^1",
                "rtk git log -p ~/project/config/.e^1",
                "rtk git log -p ~/notes~1",
                "rtk git log secret.txt~1",
                "rtk git log secret.txt~1..HEAD",
                "rtk git log HEAD~1..origin/main",
                "rtk git log a#b",
                "rtk git log HEAD~1x",
                "rtk git log 'HEAD'~1",
                "rtk grep -e s^q .",
                "rtk ls ~/x^1",
                // git reads a quoted wildcard as a pathspec glob.
                "rtk git log -p 'secre[t].txt'",
                "rtk git diff 'secre?.txt'",
                "rtk git add '*secret*'",
                "rtk git checkout -- 'secre[t].txt'",
                "rtk git log --format='%h' -- 'secre*'",
            ] {
                assert_eq!(at(cmd), Gate::Ask, "{cmd}");
            }
            for cmd in [
                "rtk read ok.txt",
                "rtk grep -rn secret.txt .",
                "rtk find . -name secret.txt",
                // Single-quoted text is literal; a commit message is no path.
                "rtk read 'secre*'",
                "rtk ls '$(id)'",
                "rtk git commit -m secret.txt",
                "rtk git commit -m 'costs $5'",
                "rtk git commit -m fix#12",
                "rtk git log HEAD~5..HEAD",
                "rtk git log main^..HEAD",
                "rtk git diff HEAD~2...HEAD",
            ] {
                assert_eq!(at(cmd), Gate::Pass, "{cmd}");
            }
        });
        let ask = rules_with_read_ask(&[], &[], &[], &["./secret.txt"]);
        let tmp = test_isolation::tempdir();
        let dir = tmp.path().to_string_lossy().into_owned();
        assert_eq!(plain("rtk read secret.txt", &ask, Some(&dir)), Gate::Ask);
        assert_eq!(plain("rtk read ok.txt", &ask, Some(&dir)), Gate::Pass);
    }

    #[test]
    fn command_words_leave_redirects_and_their_operands_out() {
        let cases: &[(&str, &[&str])] = &[
            ("git >/dev/null push", &["git", "push"]),
            ("git 2>&1 push", &["git", "push"]),
            ("git > out push x", &["git", "push", "x"]),
            ("ed -s <<< '!git push'", &["ed", "-s"]),
            ("ed -s <<'EOF'", &["ed", "-s"]),
            ("cat < in.txt -n", &["cat", "-n"]),
            ("a>b c", &["a", "c"]),
            ("grep 'x > y' f", &["grep", "'x > y'", "f"]),
        ];
        for (segment, words) in cases {
            assert_eq!(command_words(segment), *words, "{segment}");
        }
    }

    #[test]
    fn a_dashed_git_builtin_reads_as_the_subcommand() {
        let r = rules(&["git push:*"], &[], &[]);
        for cmd in [
            "rtk bypass /usr/lib/git-core/git-push",
            "rtk bypass git-push origin main",
            "rtk run git-push",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Deny, "{cmd}");
        }
        assert_eq!(gate("rtk git >/dev/null push", &r), Gate::Deny);
    }
}
