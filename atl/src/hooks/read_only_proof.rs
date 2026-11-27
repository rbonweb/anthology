//! Anthology fork: when the Claude hook may drop its ask for a read-only line, and
//! when it must not rewrite at all.
//!
//! [`anthology_gate`](super::anthology_gate) asks, with `permissionDecision: "ask"`,
//! whenever a line that may run atl is not in the plain shape, and it counts any
//! line that merely contains `atl` (or the legacy `rtk`) as one (`grep -n x atl/src/main.rs`).
//! A hook ask beats the user's `Bash(atl *)` allow in every mode, so a harmless
//! read prompts. And in auto mode Claude Code's classifier judges what the model
//! wrote; a hook that changes the input afterwards makes it refuse ("a hook changed
//! this call's input after auto mode reviewed it"). This module fixes both, and
//! only ever relaxes what the gate has already settled:
//!
//! 1. **Deny first.** It never sees a deny: [`HookDecision::Deny`] is returned by
//!    the gate before and passes through untouched.
//! 2. **A needless ask is dropped.** An [`AskOwner::Hook`] ask is relaxed only
//!    when all of these hold:
//!    - ATL's mirror of the host's rule matching finds no ask rule for the line
//!      (`ClaudeHookPermissions::verdict` is not `Ask`);
//!    - the gate, run on the line as typed, finds nothing to ask about: no ask
//!      rule, no `Read()` rule, no unreadable part ([`Gate::Pass`]). What is left
//!      to the ask is the "mentions atl, so it must be plain" step, which is about
//!      what the line may run, and the next point answers that;
//!    - `anthology perm classify`, the proof the permissions mod uses, shows the
//!      line read-only ([`proven_read_only`]), so the line runs no atl at all, and
//!      every path it reads lies under a project root, where Claude Code reads
//!      without asking.
//!
//!    The answer is then an allow with the rewrite when the rewrite is in the
//!    plain shape and the gate passes it as well; otherwise no rewrite and no
//!    decision (`Defer`): the host judges the original, as it would without atl.
//!    Anything the proof does not cover keeps today's ask.
//! 3. **Auto mode.** With `permission_mode` `auto` in the payload nothing is
//!    rewritten: an allow or a host-judged rewrite becomes `Defer`, and a needless
//!    ask (point 2) becomes `Defer` too. An ask the proof does not cover stays.
//!
//! The proof is a subprocess, so it is spent only on a line the gate would ask
//! about. ATL finds it through `ATL_PERM_CLASSIFY`, set by `bin/atl-hook` to the
//! plugin's `bin/anthology`; with the variable unset (an `atl init` install, no
//! plugin) nothing is ever proven and the hook behaves as it did. The request is
//! the classifier's own protocol, with one difference: the classifier does not
//! trust the request's `cwd` to be the shell's, so the line is sent as
//! `cd <payload cwd> && <line>`, which is the working directory the gate already
//! resolves relative paths and rules against.

use super::anthology_gate::{Gate, evaluate, plain};
use super::decision::{AskOwner, HookDecision};
use super::permissions::{ClaudeHookPermissions, PermissionVerdict};
use crate::core::user_env;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

// The variables read below are named with literals, which the environment scan of
// `core::test_isolation` needs: `ATL_PERM_CLASSIFY` is the plugin's `bin/anthology`
// shim, set by `bin/atl-hook`; `ATL_HOOK_USER_PATH` is the caller's PATH before the
// shim made it safe (the classifier checks the PATH the user's shell runs the line
// with, so it gets that one).

/// What `fish_script::wrap` starts a wrapped script with.
const FISH_WRAP: &str = "atl run --shell fish -c ";
/// The first line of a proof.
const TOKEN: &str = "anthology-perm:read-only:v1";
/// The longest line sent for a proof (as the permissions mod).
const MAX_COMMAND: usize = 4096;
/// The most paths a proof may list (as the permissions mod).
const MAX_PATHS: usize = 64;
const MAX_STDOUT: u64 = 64 * 1024;
const TIMEOUT: Duration = Duration::from_secs(3);

/// The permission mode of the call, as far as the hook cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// `permission_mode` is `auto`: the host's classifier reviews the call.
    Auto,
    /// Any other mode, or none.
    Other,
}

impl Mode {
    /// The top-level `permission_mode` of the hook input. Claude Code writes it
    /// before `tool_input`, and the model cannot add a top-level key; a spelling
    /// other than exactly `auto` is not auto mode.
    pub(crate) fn of(payload: &Value) -> Self {
        match payload.get("permission_mode").and_then(Value::as_str) {
            Some("auto") => Self::Auto,
            _ => Self::Other,
        }
    }
}

/// What [`refine`] asks about a call, each answered only when needed.
pub(crate) struct Checks<'a> {
    /// The host's rule matching, as ATL mirrors it, asks about the line.
    pub(crate) rule_asks: bool,
    /// The gate, run on the line as typed, has nothing to ask about.
    pub(crate) original_clear: &'a dyn Fn() -> bool,
    /// The gate passes the rewrite, and it is in the plain shape.
    pub(crate) rewrite_clear: &'a dyn Fn(&str) -> bool,
    /// `anthology perm classify` shows the line read-only.
    pub(crate) proof: &'a dyn Fn() -> bool,
}

impl Checks<'_> {
    /// An ask the gate gave only because the line may run atl, for a line shown
    /// not to.
    fn needless(&self) -> bool {
        !self.rule_asks && (self.original_clear)() && (self.proof)()
    }
}

/// Relax `decision`, the gate's final answer, as the module doc says. A deny, a
/// defer and every ask but a needless one come back as they were.
pub(crate) fn refine(
    cmd: &str,
    decision: HookDecision,
    mode: Mode,
    checks: &Checks<'_>,
) -> HookDecision {
    match decision {
        HookDecision::AllowRewrite(_) | HookDecision::AskRewrite(_, AskOwner::Host)
            if mode == Mode::Auto =>
        {
            HookDecision::Defer
        }
        // A fish wrap is asked about on purpose (the host would judge one `atl run`
        // command); its script is not one the proof reads, and it is left alone here.
        HookDecision::AskRewrite(rewritten, AskOwner::Hook)
            if !rewritten.starts_with(FISH_WRAP) && checks.needless() =>
        {
            if mode == Mode::Auto || rewritten == cmd || !(checks.rewrite_clear)(&rewritten) {
                HookDecision::Defer
            } else {
                HookDecision::AllowRewrite(rewritten)
            }
        }
        other => other,
    }
}

/// [`refine`] for the Claude hook, with the gate's own answers.
pub(crate) fn refine_claude(
    cmd: &str,
    decision: HookDecision,
    mode: Mode,
    permissions: &ClaudeHookPermissions,
    cwd: Option<&str>,
) -> HookDecision {
    // Everything that could change is a rewrite or a hook ask; spend nothing else.
    if !matches!(
        decision,
        HookDecision::AllowRewrite(_) | HookDecision::AskRewrite(..)
    ) {
        return decision;
    }
    let rules = &permissions.gate;
    refine(
        cmd,
        decision,
        mode,
        &Checks {
            rule_asks: permissions.verdict == PermissionVerdict::Ask,
            original_clear: &|| evaluate(cmd, rules, permissions.verdict, cwd) == Gate::Pass,
            rewrite_clear: &|rewritten| {
                evaluate(rewritten, rules, PermissionVerdict::Default, cwd) == Gate::Pass
                    && plain(rewritten, rules, cwd) == Gate::Pass
            },
            proof: &|| proven_read_only(cmd, cwd, rules.project_roots(), rules.read_rule_texts()),
        },
    )
}

/// Whether `anthology perm classify` shows `cmd` read-only, run in `cwd`, and
/// every path it reads lies under one of `roots`. False on every failure: no
/// classifier, a working directory it cannot send, a refusal, a timeout.
pub(crate) fn proven_read_only(
    cmd: &str,
    cwd: Option<&str>,
    roots: &[PathBuf],
    read_rules: Vec<String>,
) -> bool {
    let Some(classifier) = classifier() else {
        return false;
    };
    // The classifier takes a working directory only from a leading absolute,
    // literal `cd`.
    let Some(cwd) = cwd
        .filter(|cwd| cwd.starts_with('/'))
        .and_then(anthology_shell::literal_cd_target)
    else {
        return false;
    };
    if cmd.len() > MAX_COMMAND || cmd.contains(['\n', '\r']) {
        return false;
    }
    let roots: Vec<PathBuf> = roots
        .iter()
        .filter_map(|root| std::fs::canonicalize(root).ok())
        .filter(|root| root.is_dir())
        .collect();
    let root_texts: Vec<&str> = roots.iter().filter_map(|root| root.to_str()).collect();
    if root_texts.is_empty() || root_texts.len() != roots.len() {
        return false;
    }
    let request = json!({
        "command": format!("cd {cwd} && {cmd}"),
        "cwd": cwd,
        "roots": root_texts,
        "read_rules": read_rules,
    })
    .to_string();
    let Some(stdout) = run_classifier(&classifier, &request) else {
        return false;
    };
    classified_paths(&stdout).is_some_and(|paths| {
        paths
            .iter()
            .all(|path| roots.iter().any(|root| Path::new(path).starts_with(root)))
    })
}

/// The classifier the shim named: an absolute path to a file.
fn classifier() -> Option<PathBuf> {
    let path = PathBuf::from(user_env::var("ATL_PERM_CLASSIFY")?);
    (path.is_absolute() && path.is_file()).then_some(path)
}

/// The paths a proof lists, one absolute path per line after the token, or `None`
/// for anything else: no token, no path, a relative or blank line, an
/// unterminated last line, more than [`MAX_PATHS`] paths (the permissions mod's
/// reading of the same output).
fn classified_paths(stdout: &str) -> Option<Vec<&str>> {
    let body = stdout.strip_suffix('\n')?;
    let mut lines = body.split('\n');
    if lines.next()? != TOKEN {
        return None;
    }
    let paths: Vec<&str> = lines.collect();
    let sound = !paths.is_empty()
        && paths.len() <= MAX_PATHS
        && paths
            .iter()
            .all(|path| path.starts_with('/') && !path.contains(['\r', '\0']));
    sound.then_some(paths)
}

/// Run `<classifier> perm classify` with `request` on stdin: its stdout when it
/// exits 0 within [`TIMEOUT`], `None` otherwise. The child is killed on a timeout.
fn run_classifier(classifier: &Path, request: &str) -> Option<String> {
    let mut command = Command::new(classifier);
    command
        .args(["perm", "classify"])
        // A hook never downloads or installs.
        .env("ANTHOLOGY_NO_INSTALL", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(path) = user_env::var("ATL_HOOK_USER_PATH") {
        command.env("PATH", path);
    }
    let mut child = command.spawn().ok()?;
    // The request is far below the pipe's buffer, so this never blocks; a child
    // that exits without reading it only makes the write fail.
    let mut stdin = child.stdin.take()?;
    let _ = stdin.write_all(request.as_bytes());
    drop(stdin);
    let stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut text = Vec::new();
        let _ = stdout.take(MAX_STDOUT + 1).read_to_end(&mut text);
        text
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < TIMEOUT => {
                std::thread::sleep(Duration::from_millis(2));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let text = reader.join().ok()?;
    if !status.success() || text.len() as u64 > MAX_STDOUT {
        return None;
    }
    String::from_utf8(text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    const CMD: &str = "grep -n x atl/src/main.rs";
    const REWRITE: &str = "atl grep -n x atl/src/main.rs";

    /// The answers of a [`Checks`], and how often the proof was asked.
    struct Facts {
        rule_asks: bool,
        original_clear: bool,
        rewrite_clear: bool,
        proven: bool,
        proofs: Cell<u32>,
    }

    impl Facts {
        fn needless() -> Self {
            Self {
                rule_asks: false,
                original_clear: true,
                rewrite_clear: true,
                proven: true,
                proofs: Cell::new(0),
            }
        }

        fn refine(&self, decision: HookDecision, mode: Mode) -> HookDecision {
            refine(
                CMD,
                decision,
                mode,
                &Checks {
                    rule_asks: self.rule_asks,
                    original_clear: &|| self.original_clear,
                    rewrite_clear: &|_| self.rewrite_clear,
                    proof: &|| {
                        self.proofs.set(self.proofs.get() + 1);
                        self.proven
                    },
                },
            )
        }
    }

    fn hook_ask(rewritten: &str) -> HookDecision {
        HookDecision::AskRewrite(rewritten.to_string(), AskOwner::Hook)
    }

    #[test]
    fn a_needless_ask_becomes_an_allow_with_the_rewrite() {
        let facts = Facts::needless();
        assert_eq!(
            facts.refine(hook_ask(REWRITE), Mode::Other),
            HookDecision::AllowRewrite(REWRITE.to_string())
        );
    }

    #[test]
    fn a_needless_ask_without_a_clean_rewrite_defers() {
        let mut facts = Facts::needless();
        facts.rewrite_clear = false;
        assert_eq!(
            facts.refine(hook_ask(REWRITE), Mode::Other),
            HookDecision::Defer
        );
        // No rewrite was made at all: the ask was about the original.
        assert_eq!(
            Facts::needless().refine(hook_ask(CMD), Mode::Other),
            HookDecision::Defer
        );
    }

    #[test]
    fn an_ask_stays_unless_every_condition_holds() {
        for (name, facts) in [
            (
                "an ask rule matches",
                Facts {
                    rule_asks: true,
                    ..Facts::needless()
                },
            ),
            (
                "the gate has something to ask about",
                Facts {
                    original_clear: false,
                    ..Facts::needless()
                },
            ),
            (
                "not shown read-only",
                Facts {
                    proven: false,
                    ..Facts::needless()
                },
            ),
        ] {
            for mode in [Mode::Other, Mode::Auto] {
                assert_eq!(
                    facts.refine(hook_ask(REWRITE), mode),
                    hook_ask(REWRITE),
                    "{name} / {mode:?}"
                );
            }
        }
    }

    #[test]
    fn the_proof_is_the_last_thing_asked_for() {
        // An ask rule or a clear gate answer settles it without a subprocess.
        for facts in [
            Facts {
                rule_asks: true,
                ..Facts::needless()
            },
            Facts {
                original_clear: false,
                ..Facts::needless()
            },
        ] {
            facts.refine(hook_ask(REWRITE), Mode::Other);
            assert_eq!(facts.proofs.get(), 0);
        }
        // Nothing but a hook ask is ever proven.
        let facts = Facts::needless();
        for decision in [
            HookDecision::Deny,
            HookDecision::Defer,
            HookDecision::AllowRewrite(REWRITE.to_string()),
            HookDecision::AskRewrite(REWRITE.to_string(), AskOwner::Host),
        ] {
            facts.refine(decision, Mode::Other);
        }
        assert_eq!(facts.proofs.get(), 0);
    }

    /// The wrap the fish path builds (`fish_script::wrap`) keeps its explicit ask.
    #[test]
    fn a_fish_wrap_keeps_its_ask() {
        let wrapped = crate::discover::fish_script::wrap("set x 1; cat x");
        assert!(wrapped.starts_with(FISH_WRAP), "{wrapped}");
        for mode in [Mode::Other, Mode::Auto] {
            assert_eq!(
                Facts::needless().refine(hook_ask(&wrapped), mode),
                hook_ask(&wrapped),
                "{mode:?}"
            );
        }
    }

    #[test]
    fn a_deny_and_a_defer_are_never_touched() {
        for facts in [
            Facts::needless(),
            Facts {
                proven: false,
                ..Facts::needless()
            },
        ] {
            for mode in [Mode::Other, Mode::Auto] {
                assert_eq!(
                    facts.refine(HookDecision::Deny, mode),
                    HookDecision::Deny,
                    "{mode:?}"
                );
                assert_eq!(
                    facts.refine(HookDecision::Defer, mode),
                    HookDecision::Defer,
                    "{mode:?}"
                );
            }
        }
    }

    #[test]
    fn auto_mode_rewrites_nothing_the_host_would_judge() {
        let facts = Facts {
            proven: false,
            ..Facts::needless()
        };
        let host_judged = || {
            [
                HookDecision::AllowRewrite(REWRITE.to_string()),
                HookDecision::AskRewrite(REWRITE.to_string(), AskOwner::Host),
            ]
        };
        for decision in host_judged() {
            assert_eq!(facts.refine(decision, Mode::Auto), HookDecision::Defer);
        }
        // Outside auto mode they are as they were.
        for (decision, same) in host_judged().into_iter().zip(host_judged()) {
            assert_eq!(facts.refine(decision, Mode::Other), same);
        }
    }

    /// An ask in auto mode that the proof does not cover is still an ask, and one
    /// it covers is no rewrite and no ask.
    #[test]
    fn auto_mode_drops_a_needless_ask_and_keeps_the_other_asks() {
        assert_eq!(
            Facts::needless().refine(hook_ask(REWRITE), Mode::Auto),
            HookDecision::Defer
        );
        let unproven = Facts {
            proven: false,
            ..Facts::needless()
        };
        assert_eq!(
            unproven.refine(hook_ask(REWRITE), Mode::Auto),
            hook_ask(REWRITE)
        );
    }

    #[test]
    fn the_mode_is_the_top_level_permission_mode_auto_and_nothing_else() {
        let mode = |payload: Value| Mode::of(&payload);
        assert_eq!(mode(json!({"permission_mode": "auto"})), Mode::Auto);
        for other in [
            "default",
            "acceptEdits",
            "plan",
            "dontAsk",
            "bypassPermissions",
            "Auto",
            " auto",
            "autopilot",
            "",
        ] {
            assert_eq!(
                mode(json!({"permission_mode": other})),
                Mode::Other,
                "{other:?}"
            );
        }
        assert_eq!(mode(json!({})), Mode::Other);
        assert_eq!(mode(json!({"permission_mode": null})), Mode::Other);
        assert_eq!(mode(json!({"permission_mode": 1})), Mode::Other);
        // Only the top level counts: the tool input is the model's.
        assert_eq!(
            mode(json!({"tool_input": {"permission_mode": "auto"}})),
            Mode::Other
        );
    }

    #[test]
    fn a_proof_is_the_token_and_absolute_paths_only() {
        let ok = "anthology-perm:read-only:v1\n/w/a\n/w/b\n";
        assert_eq!(classified_paths(ok), Some(vec!["/w/a", "/w/b"]));
        let many = format!(
            "anthology-perm:read-only:v1\n{}",
            "/w/a\n".repeat(MAX_PATHS + 1)
        );
        for bad in [
            "",
            "\n",
            "anthology-perm:read-only:v1\n",
            "anthology-perm:read-only:v1",
            "anthology-perm:read-only:v1\n/w/a",
            "anthology-perm:read-only:v1\n\n",
            "anthology-perm:read-only:v1\nrelative\n",
            "anthology-perm:read-only:v1\n/w/a\n\n",
            "anthology-perm:read-only:v2\n/w/a\n",
            "x anthology-perm:read-only:v1\n/w/a\n",
            "anthology-perm:read-only:v1\n/w/a\r\n",
            many.as_str(),
        ] {
            assert_eq!(classified_paths(bad), None, "{bad:?}");
        }
    }

    /// The ask this module drops exists because `Bash(atl *)` would allow a line
    /// that runs atl. That the proof never covers one is the shared prover's
    /// property (the classifier is built on it); this pins it here, where it is
    /// relied on.
    #[test]
    fn a_line_that_runs_atl_is_never_proven_read_only() {
        for line in [
            "atl ls",
            "atl read x",
            "atl bypass cat x",
            "atl run -c 'ls'",
            "cd /w && atl git status",
            "ls | atl read x",
            "/usr/local/bin/atl ls",
            "rtk ls", // legacy name
            "ls; atl bypass rm x",
        ] {
            assert!(
                matches!(
                    anthology_shell::classify_readonly(line, "/home/t"),
                    anthology_shell::ReadOnly::NotProven(_)
                ),
                "{line}"
            );
        }
        // A word that only contains atl is no command (what the module is for).
        assert!(matches!(
            anthology_shell::classify_readonly("cd /w && grep -n x atl/src/main.rs", "/home/t"),
            anthology_shell::ReadOnly::Proven { .. }
        ));
    }

    /// Nothing that can be checked without a subprocess gets one.
    #[test]
    fn no_classifier_no_proof() {
        let root = [PathBuf::from("/")];
        let prove = |classifier: Option<&str>, cmd: &str, cwd: Option<&str>| {
            user_env::with_vars(&[("ATL_PERM_CLASSIFY", classifier)], || {
                proven_read_only(cmd, cwd, &root, Vec::new())
            })
        };
        // Unset, empty, relative, missing.
        for classifier in [
            None,
            Some(""),
            Some("anthology"),
            Some("/nonexistent/anthology"),
        ] {
            assert!(!prove(classifier, "ls /", Some("/")), "{classifier:?}");
        }
        // Even a classifier that exists is not run for a line or a directory it
        // cannot be sent.
        let exists = Some("/bin/sh");
        assert!(!prove(exists, "ls /", None), "no working directory");
        assert!(!prove(exists, "ls /", Some("relative")), "a relative one");
        assert!(!prove(exists, "ls /", Some("/a b")), "a space in it");
        assert!(!prove(exists, "ls /", Some("/a/../b")), "a `..` in it");
        assert!(!prove(exists, "ls /\nls /", Some("/")), "a newline");
        assert!(
            !prove(exists, &"ls /;".repeat(MAX_COMMAND), Some("/")),
            "too long"
        );
    }
}
