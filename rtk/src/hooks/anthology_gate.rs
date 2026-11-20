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
//! - Opaque inside an rtk expansion (the hook asks) when any Bash deny or ask rule
//!   exists: `eval`, `source`, `.`; a command word with `$` or a glob; a shell with
//!   no `-c` (stdin or a script file); an interpreter given inline code (python,
//!   node, bun, deno eval, perl, ruby, php, lua, osascript); busybox, toybox, sudo,
//!   doas; `rg --pre`. Still unread: a script file the agent wrote and then runs
//!   (`python x.py`, `./x.sh`), `python -m`, and any other program that runs a
//!   string (`awk system()`, `make`, a git alias or hook, `ssh host cmd`).
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
use std::path::{Path, PathBuf};

/// How many rtk or wrapper expansions deep the gate reads before it gives up and
/// asks.
const MAX_DEPTH: usize = 4;
/// Units of work (about sixteen bytes of text each) one command may cost, so a
/// crafted command cannot make the hook slow. Past it the answer is ask, or defer
/// when nothing in the command could be an rtk form (see [`evaluate`]).
const WORK_BUDGET: usize = 1_000_000;
/// The least a segment costs: 4000 segments use the whole budget.
const SEGMENT_COST: usize = 250;
/// The shells whose `-c` script the gate reads.
const SHELLS: &[&str] = &["sh", "bash", "zsh", "dash", "ksh", "mksh", "fish"];
/// The shells whose grammar the lexer models: anything else is opaque.
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
pub(crate) fn apply(gate: Gate, cmd: &str, decision: HookDecision) -> HookDecision {
    if gate != Gate::Ask {
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
        Self {
            deny,
            ask,
            read_deny,
            read_ask,
            prefixes: registry::normalize_transparent_prefixes(transparent_prefixes),
            project_roots,
        }
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
    };
    if scanner.scan_text(cmd, 0, ctx, &mut cwds).is_err() {
        return Gate::Deny;
    }
    if scanner.exhausted {
        // Part of the command was never read. Text that may hold an rtk form (or
        // any Bash rule at all) asks; otherwise the host, which sees no rtk text
        // and gets no rewrite, judges the original.
        if scanner.bash_rules || cmd.contains("rtk") {
            scanner.ask = true;
        } else {
            scanner.defer = true;
        }
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
}

/// One more text to scan, and how it was reached.
#[derive(PartialEq)]
struct Expansion {
    text: String,
    via_rtk: bool,
    payload: bool,
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
    /// The command word's basename, unquoted.
    name: String,
    /// The form with every word unquoted and joined by single spaces: what a rule
    /// that names `git push` has to match when the text says `git 'push'`.
    unquoted: String,
}

impl<'a> Parsed<'a> {
    fn new(form: &'a str) -> Self {
        let (words, spans) = lexer::words_and_spans(form);
        let resolved: Vec<String> = words.iter().map(|word| resolve_word_text(word)).collect();
        let name = resolved
            .first()
            .map(|word| basename(word).to_string())
            .unwrap_or_default();
        Self {
            form,
            words,
            spans,
            name,
            unquoted: resolved.join(" "),
        }
    }
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
        let cdpath_set = mentions_word(cmd, "CDPATH")
            || crate::core::user_env::var("CDPATH").is_some_and(|value| !value.is_empty());
        let env = EnvFlags {
            home: mentions_word(cmd, "HOME") || cdpath_set,
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
        if !self.bash_rules {
            return;
        }
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
        if ctx.in_rtk && substitution {
            self.ask = true;
        }
        if self.reading() {
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
        // past each assignment or wrapper in turn.
        let residue = permissions::strip_grammar_residue(segment);
        let peeled = registry::matching_readings(residue);
        if !peeled.complete {
            self.ask = true;
        }
        let mut forms: Vec<&str> = vec![segment, residue];
        let positions = registry::shell_wrapper_positions(residue, &self.rules.prefixes);
        for form in peeled.readings.iter().copied().chain(positions) {
            if !form.is_empty() && !forms.contains(&form) {
                forms.push(form);
            }
        }
        if !self.spend(forms.len() * (1 + segment.len() / 16)) {
            return Ok(());
        }
        let parsed: Vec<Parsed> = forms.iter().map(|form| Parsed::new(form)).collect();

        // Rules are matched against the text as written and as the shell reads it
        // once the quotes are gone: `git 'push'` is `git push` to a rule.
        for reading in parsed
            .iter()
            .flat_map(|p| [p.form, p.unquoted.as_str()])
            .filter(|reading| !reading.is_empty())
        {
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

        let mut expansions: Vec<Expansion> = Vec::new();
        for p in &parsed {
            if p.words.is_empty() {
                continue;
            }
            let name = p.name.as_str();
            let args = &p.words[1..];
            if self.reading() {
                if READ_COMMANDS.contains(&name) {
                    self.check_read(name, args, ctx, cwds, false)?;
                } else if ctx.in_rtk && !is_assignment(name) {
                    // Anything rtk runs can read a file: sed, awk, cp, jq...
                    self.check_read(name, args, ctx, cwds, true)?;
                }
            }
            if (ctx.in_rtk || ctx.payload) && is_opaque_command(name, &p.words) {
                self.opaque(ctx);
            }
            if SHELLS.contains(&name)
                && let Some(script) = shell_wrapper_script(&p.words)
            {
                push_unique(
                    &mut expansions,
                    Expansion {
                        text: script,
                        via_rtk: false,
                        payload: false,
                    },
                );
            }
            if let Some(inner) = rtk_inner(p.form, &p.words, &p.spans) {
                self.ask |= inner.opaque || substitution;
                for text in inner.texts {
                    push_unique(
                        &mut expansions,
                        Expansion {
                            text,
                            via_rtk: true,
                            payload: false,
                        },
                    );
                }
            }
            let payloads = exec_payloads(name, p.form, &p.words, &p.spans);
            if payloads.unattested {
                self.opaque(Ctx {
                    payload: true,
                    ..ctx
                });
            }
            for text in payloads.texts {
                push_unique(
                    &mut expansions,
                    Expansion {
                        text,
                        via_rtk: false,
                        payload: true,
                    },
                );
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
            };
            // A `cd` inside the expansion runs in a process of its own.
            let mut nested = cwds.clone();
            self.scan_text(&expansion.text, depth + 1, nested_ctx, &mut nested)?;
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
            // No prompt for a `$VAR`, but a named path that an ask rule covers asks.
            self.ask |= matched_ask;
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
fn is_opaque_command(name: &str, words: &[&str]) -> bool {
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
        return true;
    }
    match name {
        "eval" | "source" | "." | "busybox" | "toybox" | "sudo" | "doas" => true,
        // A shell with no `-c` reads stdin or a script file.
        shell if SHELLS.contains(&shell) => shell_wrapper_script(words).is_none(),
        // `rg --pre CMD` runs CMD on every file.
        "rg" | "grep" => words[1..].iter().any(|word| {
            let word = resolve_word_text(word);
            word == "--pre" || word.starts_with("--pre=")
        }),
        other => runs_inline_code(other, words),
    }
}

/// An interpreter given code on its command line.
fn runs_inline_code(name: &str, words: &[&str]) -> bool {
    let is_python = name
        .strip_prefix("python")
        .is_some_and(|version| version.chars().all(|c| c.is_ascii_digit() || c == '.'));
    // (single-dash flag letters that introduce code, long flags that do)
    let (letters, long): (&str, &[&str]) = if is_python {
        ("c", &[])
    } else {
        match name {
            "node" | "nodejs" | "bun" => ("ep", &["--eval", "--print"]),
            "perl" => ("eE", &[]),
            "ruby" | "lua" | "luajit" | "osascript" => ("e", &[]),
            "php" => ("r", &[]),
            "deno" => ("", &[]),
            _ => return false,
        }
    };
    for raw in &words[1..] {
        let word = resolve_word_text(raw);
        if name == "deno" && word == "eval" {
            return true;
        }
        if long.contains(&word.as_str()) {
            return true;
        }
        match word.strip_prefix('-') {
            Some(flags) if !word.starts_with("--") => {
                if flags.chars().any(|c| letters.contains(c)) {
                    return true;
                }
            }
            Some(_) => {}
            // The first operand is the script: its own flags are not ours.
            None => break,
        }
    }
    false
}

/// The commands an exec action runs.
#[derive(Debug, Default, PartialEq, Eq)]
struct Payloads {
    texts: Vec<String>,
    /// The action's command could not be read.
    unattested: bool,
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
        "xargs" => {
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
            WordVerdict::Uncertain => Self::Uncertain,
            WordVerdict::Denied => Self::Denied,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WordVerdict {
    Allowed,
    Uncertain,
    Denied,
}

/// A word the gate cannot turn into one path: it expands at run time.
fn is_uncertain_word(word: &str) -> bool {
    word.contains(['$', '`', '*', '?', '[', '{'])
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
        for candidate in read_candidates(name, args) {
            match self.check_word(&candidate.text, cwds, generic) {
                WordVerdict::Denied => return ReadOutcome::Denied,
                WordVerdict::Uncertain if !candidate.pattern => uncertain = true,
                WordVerdict::Uncertain | WordVerdict::Allowed => {}
            }
        }
        if uncertain {
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
            unknown = true;
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

/// One word a READ command may read.
struct Candidate {
    text: String,
    /// A grep/rg search pattern: still checked as a path, but a glob-looking one
    /// does not make the command uncertain.
    pattern: bool,
}

/// Every non-option word of a READ command (and the value of `--opt=value`) is a
/// candidate path. `find`'s paths end where its expression starts; `grep` and `rg`
/// take their pattern as the first operand unless `-e`/`-f` supply it.
fn read_candidates(name: &str, args: &[&str]) -> Vec<Candidate> {
    let resolved: Vec<String> = args.iter().map(|arg| resolve_word_text(arg)).collect();
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
        i += 1;
        if options_done {
            out.push(Candidate {
                text: arg.clone(),
                pattern: false,
            });
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
        out.push(Candidate {
            text: arg.clone(),
            pattern: is_pattern,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
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
            apply(Gate::Ask, "git status", allow),
            HookDecision::AskRewrite("rtk git status".into(), AskOwner::Hook)
        );
        let host = HookDecision::AskRewrite("rtk git status".into(), AskOwner::Host);
        assert_eq!(
            apply(Gate::Ask, "git status", host),
            HookDecision::AskRewrite("rtk git status".into(), AskOwner::Hook)
        );
        assert_eq!(
            apply(Gate::Ask, "htop", HookDecision::Defer),
            HookDecision::AskRewrite("htop".into(), AskOwner::Hook)
        );
        assert_eq!(
            apply(Gate::Pass, "htop", HookDecision::Defer),
            HookDecision::Defer
        );
        assert_eq!(
            apply(Gate::Ask, "x", HookDecision::Deny),
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
            "find . -exec $CMD {} \\;",
            "xargs --frobnicate git status",
            "env --frobnicate git status",
            "find . -exec python3 -c 'print(1)' \\;",
        ] {
            assert_eq!(gate(cmd, &r), Gate::Defer, "{cmd}");
        }
        // Behind rtk the same asks.
        assert_eq!(gate("rtk find . -exec sh {} \\;", &r), Gate::Ask);
        // With no Bash rule there is nothing to protect.
        let no_bash = rules(&[], &[], &["./secret.txt"]);
        assert_eq!(gate("find . -exec sh {} \\;", &no_bash), Gate::Pass);
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
        ] {
            assert_eq!(gate(cmd, &r), Gate::Ask, "{cmd}");
        }
        // The same constructs outside rtk keep today's behaviour, and the
        // ordinary ones inside it are not opaque.
        for cmd in [
            "eval 'git status'",
            "python3 -c 'x'",
            "bash script.sh",
            "rtk bypass python3 script.py",
            "rtk run -c '[ -f x ] && echo y'",
            "rtk run -c 'FOO=$BAR true'",
            "rtk run -c 'echo hi'",
        ] {
            assert_ne!(gate(cmd, &r), Gate::Ask, "{cmd}");
        }
        // No Bash rule: nothing to be opaque about.
        let none = rules(&[], &[], &["./secret.txt"]);
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
            // An unresolvable operand adds no prompt for a program that is no read;
            // a program outside rtk is the host's own business.
            for cmd in [
                "rtk bypass sed -n p ok.txt",
                "rtk bypass echo $HOME",
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
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
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
                started.elapsed() < std::time::Duration::from_secs(10),
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
        // Nothing that could be rtk, and only Read rules: the host judges the original.
        assert_eq!(gate(&many, &read_only), Gate::Defer);
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
}
