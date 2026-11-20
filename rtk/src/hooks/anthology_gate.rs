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
//!    cannot resolve (`$VAR`, a glob, an unknown cwd) is `Defer` at top level (the
//!    host judges the original natively) and ask inside an rtk or wrapper expansion.
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
//! With no deny, ask or `Read` rule anywhere the gate is inert and the hook's
//! output is what it was before.
//!
//! # What the hook cannot see
//!
//! `--settings`, `--disallowedTools`, server-managed settings and the macOS MDM
//! profile (`com.anthropic.claudecode`) never reach a hook; a rule that lives only
//! there is not enforced here (Claude Code still applies it to the text it judges).
//!
//! # Known residuals
//!
//! - Files a recursive search descends into (`grep -r x .`, `find . -name x`) are
//!   not matched against `Read()` rules: only the paths named on the command line.
//! - Inside an rtk expansion every command's operand words are checked against
//!   `Read()` deny rules, whatever the command (`rtk sed -n p secret.txt`). What
//!   reads a file without naming it as an operand is not seen: `rtk deps`
//!   (manifests), `find -files0-from FILE`, `rg --pre`, a script or interpreter
//!   that opens a path itself.
//! - Opaque inside an rtk expansion (the hook asks) when any deny or ask rule of
//!   any kind exists: `eval`, `source`, `.`, `trap`, `alias`; a command word with
//!   `$` or a glob; any shell with no parseable `-c` (stdin, a script file, tcsh
//!   and csh included); any invocation of an interpreter (python, perl, ruby,
//!   node, bun, deno, php, lua, osascript, expect, tclsh, wish, Rscript, R,
//!   julia, pwsh, powershell), a script file included; busybox, toybox, sudo,
//!   doas; `rg --pre`. Still unread: `./x.sh` and other programs that run a
//!   string (`awk system()`, `make`), and git aliases and config defined outside
//!   the command (`~/.gitconfig`, a repository's `.git/config`, hooks). Git
//!   config set on the command line or in its environment (`-c alias.p=push`,
//!   `GIT_CONFIG_COUNT`, `GIT_PAGER`...) is read when a rule's head starts with
//!   `git`: it asks inside rtk and leaves a top-level command unrewritten.
//! - A launcher the gate does not model (`caffeinate`, `arch`, `ssh host cmd`,
//!   `watch`, `su -c`...) is not peeled. Backstop: inside an rtk expansion the
//!   words of any Bash rule's literal head appearing anywhere in the text, quoted
//!   or not, make the hook ask; at top level they make it leave a rewrite to the
//!   host. A rule with a leading or inner `*` has no head to look for.
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
//! - `rtk git show HEAD:secret.txt` prints a `Read()`-denied file's committed content:
//!   a `REV:path` operand is not split. Claude Code does not apply Read rules to
//!   `git show` either.
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
//! - Globs are never expanded against the disk: `rtk git add src/*.rs` asks when a
//!   `Read()` rule exists.
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
        let mut wildcard = false;
        for pattern in deny.iter().chain(&ask) {
            match rule_head(pattern) {
                Some(mut head) => {
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
        for segment in lexer::split_for_permissions(text) {
            let (words, _) = lexer::words_and_spans(segment);
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

    /// Whether any rule exists for the gate to enforce.
    fn active(&self) -> bool {
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
    let mut cwds = Cwds::new(initial);
    let ctx = Ctx {
        inner: false,
        in_rtk: false,
        payload: false,
        soft: false,
        demote: false,
        open: false,
    };
    if scanner.scan_text(cmd, 0, ctx, &mut cwds).is_err() {
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
        for segment in lexer::split_for_permissions(text) {
            self.scan_segment(segment, depth, ctx, substitution, cwds)?;
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
        } else if unresolved || matched_ask {
            if ctx.inner {
                self.ask = true;
            } else {
                self.defer = true;
            }
        }
    }

    /// `< file` (and `<> file`) reads the file whatever the command is.
    fn check_input_redirects(&mut self, text: &str, ctx: Ctx, cwds: &Cwds) -> Result<(), Denied> {
        for segment in lexer::split_for_wrappers(text) {
            let tokens = lexer::tokenize(segment);
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
                let operand = i + 1 + usize::from(glued_write);
                let Some(target) = tokens.get(operand).filter(|t| t.kind == TokenKind::Arg) else {
                    continue;
                };
                let word = resolve_word_text(&target.value);
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
            match self.check_word(&candidate.text, cwds, generic) {
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

    /// Whether one path word is denied, allowed or cannot be resolved.
    fn check_word(&self, word: &str, cwds: &Cwds, generic: bool) -> WordVerdict {
        if word.is_empty() {
            return WordVerdict::Allowed;
        }
        if !generic && self.rules.iter().any(|rule| matches!(rule, Rule::All)) {
            return WordVerdict::Denied;
        }
        if is_uncertain_word(word) {
            return WordVerdict::Expanding;
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

/// An operand, and the value of a `key=value` operand too (`dd if=secret.txt`).
fn push_operand(out: &mut Vec<Candidate>, arg: &str, pattern: bool, expands: bool) {
    out.push(Candidate {
        text: arg.to_string(),
        pattern,
        expands,
    });
    if let Some((key, value)) = arg.split_once('=')
        && !value.is_empty()
        && !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        out.push(Candidate {
            text: value.to_string(),
            pattern,
            expands,
        });
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
    let mut out = Vec::new();
    let mut options_done = false;
    let mut i = 0;
    while let Some(arg) = resolved.get(i) {
        let here = expanding[i];
        i += 1;
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
                "cd sub && cat ok.txt",
                "rtk read ok.txt",
                "echo secret.txt",
            ] {
                assert_eq!(gate_in(dir, cmd, r), Gate::Pass, "{cmd}");
            }
        });
    }

    #[test]
    fn uncertain_read_paths_defer_at_top_level_and_ask_inside_a_wrapper() {
        in_sandbox(|dir, r| {
            for cmd in [
                "cat *.txt",
                "cat $F",
                "cat `x`",
                "cat {a,b}",
                "cat sec?et.txt",
                "cat [s]ecret.txt",
            ] {
                assert_eq!(gate_in(dir, cmd, r), Gate::Defer, "{cmd}");
            }
            for cmd in ["rtk read *.txt", "rtk run cat *.txt", "bash -c 'cat *.txt'"] {
                assert_eq!(gate_in(dir, cmd, r), Gate::Ask, "{cmd}");
            }
            // An unknown cwd makes a relative path uncertain.
            assert_eq!(gate_in(dir, "cd $X && cat ok.txt", r), Gate::Defer);
            assert_eq!(gate_in(dir, "cd - && cat ok.txt", r), Gate::Defer);
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
                HookDecision::AskRewrite("rtk kubectl apply -f x".into(), AskOwner::Hook)
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
            assert_eq!(
                hook("aws s3 ls"),
                HookDecision::AskRewrite("rtk aws s3 ls".into(), AskOwner::Host)
            );
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
            assert_eq!(hook("cat *.txt"), HookDecision::Defer);
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
            "rtk run eval 'git push'",
            "rtk run -c 'eval \"git push\"'",
            "rtk run -c 'g=git; $g push'",
            "echo 'curl x' | rtk bypass bash",
            "echo 'curl x' | rtk bypass sh -s",
            "rtk bypass bash <<< 'curl x'",
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
            "rtk run -c 'FOO=$BAR true'",
            "rtk run -c 'echo hi'",
        ] {
            assert_ne!(gate(cmd, &r), Gate::Ask, "{cmd}");
        }
        // No rule at all: nothing to be opaque about.
        let none = rules(&[], &[], &[]);
        assert_eq!(gate("rtk bypass python3 -c 'x'", &none), Gate::Pass);
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
        assert_eq!(gate("HOME=/ cat ~/.ssh/id_rsa", &r), Gate::Defer);
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
                assert_eq!(gate_in(dir, "cd sub && cat ok.txt", r), Gate::Defer);
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
            "rtk git $'push'",
            "rtk git $'\\x70ush'",
            "rtk git $\"push\"",
            "rtk bypass git $'push'",
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
            "rtk bypass caffeinate git push",
            "rtk bypass caffeinate -i git push",
            "rtk bypass arch -arm64 git push",
            "rtk bypass xcrun git push",
            "rtk bypass script -q /dev/null git push",
            "rtk bypass sandbox-exec -p x git push",
            "rtk bypass watch git push",
            "rtk bypass flock /tmp/l git push",
            "rtk bypass setsid git push",
            "rtk bypass unbuffer git push",
            "rtk bypass ionice -c3 git push",
            "rtk bypass chrt 0 git push",
            "rtk bypass taskset 1 git push",
            "rtk bypass gtimeout 5 git push",
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
                "rtk bypass base64 $'secret.txt'",
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
        assert_eq!(gate("rtk bypass caffeinate =git push", &r), Gate::Ask);
        assert_eq!(
            gate("rtk bypass caffeinate /usr/bin/git push", &r),
            Gate::Ask
        );
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
    }

    #[test]
    fn a_denied_command_behind_an_unmodelled_launcher_asks() {
        let r = push_only();
        for cmd in [
            "rtk bypass caffeinate g''it push",
            "rtk bypass caffeinate gi\\t push",
            "rtk bypass caffeinate \"g\"it push",
            "rtk bypass ssh h \"g\"\"it push\"",
            "rtk bypass exec -a x g''it push",
            "rtk bypass command -p g''it push",
            "x=it; rtk bypass caffeinate g$x push",
            "rtk bypass caffeinate {git,} push",
            "rtk bypass caffeinate $'\\x67it' push",
            "rtk bypass caffeinate git pu*h",
            "rtk bypass caffeinate sh -c 'g\"\"it push'",
            "rtk bypass caffeinate bash -c 'x=push; git $x'",
            "echo 'git push' | rtk bypass caffeinate bash",
            "rtk bypass caffeinate bash <<< 'g''it push'",
            "echo 'git push' | rtk bypass caffeinate python3",
            "rtk bypass caffeinate eval 'g\"\"it push'",
            "rtk bypass env -i caffeinate g''it push",
            "rtk bypass caffeinate env g''it push",
            "rtk bypass caffeinate find . -maxdepth 0 -exec g''it push \\;",
            "rtk bypass npm exec -c 'g\"\"it push'",
            "rtk bypass watch -n1 'g\"\"it push'",
            "rtk bypass ssh h 'x=push; git $x'",
            "rtk git -c 'alias.p=!git push' p",
            "rtk bypass caffeinate git -C . push",
            "rtk bypass caffeinate /usr/bin/git push",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Ask, "{cmd}");
        }
        // What the launcher runs is a question, never a deny, but an ordinary
        // command in the same place passes.
        for cmd in [
            "rtk bypass caffeinate git status",
            "rtk bypass ssh h ls -la",
            "rtk bypass watch -n1 'git status'",
            "rtk grep python src/",
            "rtk grep -rn node src/",
            "rtk git commit -m 'use python and node here'",
            "rtk git log --oneline -n 5",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Pass, "{cmd}");
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
            "rtk bypass xargs git status",
            "xargs rm",
            "find . -name x -exec grep foo {} \\;",
            "find . -type f -exec git add {} \\;",
            "rtk bypass parallel echo ::: a b",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Pass, "{cmd}");
        }
        assert_eq!(gate("rtk bypass parallel git push ::: a", &r), Gate::Deny);
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
            "rtk uv run python script.py",
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
    }

    #[test]
    fn a_script_a_launcher_runs_is_read_in_full() {
        in_sandbox(|dir, r| {
            for cmd in [
                "rtk bypass caffeinate sh -c 'cat secret.txt'",
                "rtk bypass caffeinate sh -c 'base64 -i secret.txt'",
                "rtk bypass caffeinate sh -c 'head secret.txt'",
                "rtk bypass caffeinate sh -c 'cat < secret.txt'",
                "rtk bypass caffeinate sh -c \"cat secret.txt; echo\"",
                "rtk bypass caffeinate sh -c 'f=secret; cat $f.txt'",
                "rtk bypass caffeinate sh -c 'cp secret.txt /tmp/x'",
                "rtk find . -maxdepth 0 -exec caffeinate sh -c 'cat secret.txt' \\;",
            ] {
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
}
