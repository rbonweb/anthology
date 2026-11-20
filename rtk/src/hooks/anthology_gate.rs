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
//! - Commands rtk does not rewrite into reads (`sed`, `awk`, `less`, a script) are
//!   outside the `Read()` check; nothing launders them through `rtk`.
//! - A wrapper the peel does not model (`env -i rtk ...`, `sudo rtk ...`) is not
//!   expanded: the hook sees the command word `env` or `sudo`.
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
/// Upper bound on segments scanned for one command, so a crafted command cannot
/// make the hook slow. Past it the answer is ask.
const SEGMENT_BUDGET: usize = 4000;
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

/// Append the `Read(...)` entries of a `permissions.deny` array to `target`: the
/// path pattern for `Read(pattern)`, and `**` for a bare `Read`, which denies
/// every read.
pub(crate) fn append_read_deny(rules_value: Option<&Value>, target: &mut Vec<String>) {
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
    /// The user's `transparent_prefixes`, normalized.
    prefixes: Vec<String>,
    project_roots: Vec<PathBuf>,
}

impl GateRules {
    pub(crate) fn new(
        deny: Vec<String>,
        ask: Vec<String>,
        read_deny: Vec<String>,
        transparent_prefixes: &[String],
        project_roots: Vec<PathBuf>,
    ) -> Self {
        Self {
            deny,
            ask,
            read_deny,
            prefixes: registry::normalize_transparent_prefixes(transparent_prefixes),
            project_roots,
        }
    }

    /// Whether any rule exists for the gate to enforce.
    fn active(&self) -> bool {
        !(self.deny.is_empty() && self.ask.is_empty() && self.read_deny.is_empty())
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
    let mut scanner = Scanner::new(rules, initial.as_deref());
    let mut cwds = Cwds::new(initial);
    let ctx = Ctx {
        inner: false,
        in_rtk: false,
    };
    if scanner.scan_text(cmd, 0, ctx, &mut cwds).is_err() {
        Gate::Deny
    } else if scanner.ask {
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
}

struct Scanner<'a> {
    rules: &'a GateRules,
    reads: Option<ReadMatcher>,
    ask: bool,
    defer: bool,
    budget: usize,
}

impl<'a> Scanner<'a> {
    fn new(rules: &'a GateRules, cwd: Option<&Path>) -> Self {
        let reads = (!rules.read_deny.is_empty()).then(|| {
            ReadMatcher::new(
                &rules.read_deny,
                user_dirs::home(),
                &rules.project_roots,
                cwd,
                cfg!(any(target_os = "macos", windows)),
            )
        });
        Self {
            rules,
            reads,
            ask: false,
            defer: false,
            budget: SEGMENT_BUDGET,
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
        if self.reads.is_some() {
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
        if self.budget == 0 {
            self.ask = true;
            return Ok(());
        }
        self.budget -= 1;

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
        for form in &forms {
            if self
                .rules
                .deny
                .iter()
                .any(|pattern| permissions::command_matches_pattern(form, pattern))
            {
                return Err(Denied);
            }
            if self
                .rules
                .ask
                .iter()
                .any(|pattern| permissions::command_matches_pattern(form, pattern))
            {
                self.ask = true;
            }
        }

        let mut expansions: Vec<(String, bool)> = Vec::new();
        for form in &forms {
            let (words, spans) = lexer::words_and_spans(form);
            let Some(first) = words.first() else {
                continue;
            };
            let name = basename(&resolve_word_text(first)).to_string();
            if READ_COMMANDS.contains(&name.as_str()) {
                self.check_read(&name, &words[1..], ctx, cwds)?;
            }
            if SHELLS.contains(&name.as_str())
                && let Some(script) = shell_wrapper_script(&words)
            {
                push_unique(&mut expansions, (script, false));
            }
            if let Some(inner) = rtk_inner(form, &words, &spans) {
                self.ask |= inner.opaque || substitution;
                for text in inner.texts {
                    push_unique(&mut expansions, (text, true));
                }
            }
        }

        if self.reads.is_some() {
            apply_cd(residue, cwds);
        }

        for (text, via_rtk) in expansions {
            if depth >= MAX_DEPTH {
                self.ask = true;
                continue;
            }
            let nested_ctx = Ctx {
                inner: true,
                in_rtk: ctx.in_rtk || via_rtk,
            };
            // A `cd` inside the expansion runs in a process of its own.
            let mut nested = cwds.clone();
            self.scan_text(&text, depth + 1, nested_ctx, &mut nested)?;
        }
        Ok(())
    }

    fn check_read(
        &mut self,
        name: &str,
        args: &[&str],
        ctx: Ctx,
        cwds: &Cwds,
    ) -> Result<(), Denied> {
        let outcome = match &self.reads {
            Some(reads) => reads.check_command(name, args, cwds),
            None => return Ok(()),
        };
        self.apply_read(outcome, ctx)
    }

    fn apply_read(&mut self, outcome: ReadOutcome, ctx: Ctx) -> Result<(), Denied> {
        match outcome {
            ReadOutcome::Denied => Err(Denied),
            ReadOutcome::Uncertain => {
                if ctx.inner {
                    self.ask = true;
                } else {
                    self.defer = true;
                }
                Ok(())
            }
            ReadOutcome::Clear => Ok(()),
        }
    }

    /// `< file` reads the file whatever the command is.
    fn check_input_redirects(&mut self, text: &str, ctx: Ctx, cwds: &Cwds) -> Result<(), Denied> {
        for segment in lexer::split_for_wrappers(text) {
            let tokens = lexer::tokenize(segment);
            for (i, token) in tokens.iter().enumerate() {
                let is_input = token.kind == TokenKind::Redirect
                    && token.value.trim_start_matches(|c: char| c.is_ascii_digit()) == "<";
                if !is_input {
                    continue;
                }
                let Some(target) = tokens.get(i + 1).filter(|t| t.kind == TokenKind::Arg) else {
                    continue;
                };
                let word = resolve_word_text(&target.value);
                if word == "/dev/null" {
                    continue;
                }
                let outcome = match &self.reads {
                    Some(reads) => reads.check_word(&word, cwds),
                    None => return Ok(()),
                };
                self.apply_read(ReadOutcome::from(outcome), ctx)?;
            }
        }
        Ok(())
    }
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
        "bypass" | "proxy" => push_rest(&mut inner.texts, rest),
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
        }
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
        None => push_rest(&mut inner.texts, rest),
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
fn apply_cd(segment: &str, cwds: &mut Cwds) {
    let (words, _) = lexer::words_and_spans(segment);
    let Some(first) = words.first() else {
        return;
    };
    let target = match basename(&resolve_word_text(first)) {
        "cd" => cd_target(&words[1..], true),
        "pushd" => cd_target(&words[1..], false),
        "popd" => CdTarget::Unknown,
        _ => return,
    };
    cwds.apply(&target);
}

fn cd_target(args: &[&str], empty_is_home: bool) -> CdTarget {
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
        return match (empty_is_home, user_dirs::home()) {
            (true, Some(home)) => CdTarget::Absolute(home),
            _ => CdTarget::Unknown,
        };
    };
    if is_uncertain_word(&operand) {
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
    Glob(String),
    /// `**`: any number of components, including none.
    Any,
}

impl Comp {
    fn parse(text: &str) -> Self {
        if text == "**" {
            Self::Any
        } else if text.contains(['*', '?', '[', '\\']) {
            Self::Glob(text.to_string())
        } else {
            Self::Literal(text.to_string())
        }
    }

    fn matches(&self, name: &str) -> bool {
        match self {
            Self::Literal(literal) => literal == name,
            Self::Glob(glob) => {
                let glob: Vec<char> = glob.chars().collect();
                let name: Vec<char> = name.chars().collect();
                glob_segment(&glob, &name)
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
        }
    }

    /// The outcome for one READ command: `args` are its words after the command.
    fn check_command(&self, name: &str, args: &[&str], cwds: &Cwds) -> ReadOutcome {
        if self.rules.iter().any(|rule| matches!(rule, Rule::All)) {
            return ReadOutcome::Denied;
        }
        let mut uncertain = false;
        for candidate in read_candidates(name, args) {
            match self.check_word(&candidate.text, cwds) {
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
    fn check_word(&self, word: &str, cwds: &Cwds) -> WordVerdict {
        if word.is_empty() {
            return WordVerdict::Allowed;
        }
        if self.rules.iter().any(|rule| matches!(rule, Rule::All)) {
            return WordVerdict::Denied;
        }
        if is_uncertain_word(word) {
            return WordVerdict::Uncertain;
        }
        let mut candidates: Vec<PathBuf> = Vec::new();
        let mut unknown = false;
        if word == "~" || word.starts_with("~/") {
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
            if self.denied(&normalize(path)) {
                return WordVerdict::Denied;
            }
            if let Ok(real) = std::fs::canonicalize(path)
                && self.denied(&real)
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
    fn denied(&self, path: &Path) -> bool {
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
            Rule::All => true,
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
/// in full matches whatever lies below.
fn match_prefix(pattern: &[Comp], path: &[String]) -> bool {
    match pattern.split_first() {
        None => true,
        Some((Comp::Any, rest)) => {
            match_prefix(rest, path) || (!path.is_empty() && match_prefix(pattern, &path[1..]))
        }
        Some((comp, rest)) => {
            path.first().is_some_and(|name| comp.matches(name)) && match_prefix(rest, &path[1..])
        }
    }
}

/// gitignore-style glob for one path component: `*`, `?`, `[...]`, `\x`.
fn glob_segment(pattern: &[char], text: &[char]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some(('*', rest)) => {
            let rest = {
                let skip = rest.iter().take_while(|c| **c == '*').count();
                &rest[skip..]
            };
            (0..=text.len()).any(|k| glob_segment(rest, &text[k..]))
        }
        Some(('?', rest)) => !text.is_empty() && glob_segment(rest, &text[1..]),
        Some(('[', _)) => match parse_class(pattern) {
            Some((class, used)) => {
                text.first().is_some_and(|c| class.matches(*c))
                    && glob_segment(&pattern[used..], &text[1..])
            }
            None => text.first() == Some(&'[') && glob_segment(&pattern[1..], &text[1..]),
        },
        Some(('\\', rest)) if !rest.is_empty() => {
            text.first() == Some(&rest[0]) && glob_segment(&rest[1..], &text[1..])
        }
        Some((c, rest)) => text.first() == Some(c) && glob_segment(rest, &text[1..]),
    }
}

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
        GateRules::new(strings(deny), strings(ask), strings(read), &[], Vec::new())
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
        m.denied(&normalize(Path::new(path)))
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
        assert_eq!(m.check_command("cat", &["x"], &cwds), ReadOutcome::Denied);
        assert_eq!(m.check_command("ls", &[], &cwds), ReadOutcome::Denied);
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
        append_read_deny(Some(&value), &mut target);
        assert_eq!(target, strings(&["./secret.txt", "**", "//etc/shadow"]));
        append_read_deny(None, &mut target);
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
}
