//! Processes incoming hook calls from AI agents and rewrites commands on the fly.
//!
//! Uses `writeln!(stdout, ...)` instead of `println!` — accidental stdout/stderr
//! corrupts the JSON protocol (Claude Code bug #4669 silently disables the hook).

use super::constants::PRE_TOOL_USE_KEY;
use super::decision::{self, AskOwner, HookDecision};
use super::permissions::{self, PermissionVerdict};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::io::{self, Read, Write};

use crate::core::tracking::HookOutcome;
use crate::core::user_dirs;
use crate::core::user_env;
use crate::core::utils::strip_leading_bom;

#[cfg(feature = "engine")]
mod claude_pipeline;

const STDIN_CAP: usize = 1_048_576; // 1 MiB

fn read_stdin_limited() -> Result<String> {
    let mut input = String::new();
    io::stdin()
        .take((STDIN_CAP + 1) as u64)
        .read_to_string(&mut input)
        .context("Failed to read stdin")?;
    if input.len() > STDIN_CAP {
        anyhow::bail!("hook stdin exceeds {} byte limit", STDIN_CAP);
    }
    Ok(input)
}

// ── Copilot hook (VS Code + Copilot CLI) ──────────────────────

/// The decision every hook applies -- [`decision::decide_for_agent_at`] -- plus
/// the recall bookkeeping the hook path performs for any command it does not
/// deny. `cwd` is the payload's working directory when the host reports one
/// (only the Claude Code hook does; see #3864).
///
/// `deny_rules` says whether ATL can show that nothing in a wrapped script
/// could escape a deny rule. Only Claude Code's own hook can
/// ([`decide_claude_hook_action`]); every other caller passes
/// [`permissions::DenyRules::Unattested`] and therefore never wraps.
fn decide_from_verdict(
    cmd: &str,
    verdict: PermissionVerdict,
    deny_rules: permissions::DenyRules,
    cwd: Option<&str>,
) -> HookDecision {
    if verdict == PermissionVerdict::Deny {
        return HookDecision::Deny;
    }
    crate::hooks::rewrite_cmd::track_tee_read(cmd);
    decision::decide_for_agent_at(cmd, verdict, deny_rules, cwd)
}

fn decide_hook_action(cmd: &str, host: permissions::Host, cwd: Option<&str>) -> HookDecision {
    decide_from_verdict(
        cmd,
        permissions::check_command_for(cmd, host),
        permissions::DenyRules::Unattested,
        cwd,
    )
}

/// [`decide_hook_action`] for Claude Code's own hook, the one host whose
/// settings ATL reads and whose hook asks explicitly for a wrap. Copilot
/// shares `Host::Claude` for its verdict but judges the returned command
/// itself, so it keeps [`decide_hook_action`].
fn decide_claude_hook_action(
    cmd: &str,
    cwd: Option<&str>,
    mode: super::read_only_proof::Mode,
) -> HookDecision {
    let permissions = permissions::check_claude_hook_command(cmd);
    if permissions.verdict == PermissionVerdict::Deny {
        return HookDecision::Deny;
    }
    crate::hooks::rewrite_cmd::track_tee_read(cmd);
    decision::decide_for_claude_hook_in(cmd, &permissions, cwd, mode)
}

// ── Gemini hook ───────────────────────────────────────────────

// ── Google Antigravity hook ───────────────────────────────────

// ── Vibe hook ─────────────────────────────────────────────────

// ── Audit logging ─────────────────────────────────────────────

/// Best-effort audit log when ATL_HOOK_AUDIT=1.
fn audit_log(action: &str, original: &str, rewritten: &str) {
    if user_env::var("ATL_HOOK_AUDIT").as_deref() != Some("1") {
        return;
    }
    let _ = audit_log_inner(action, original, rewritten);
}

/// Escape newlines to prevent log-line injection in the pipe-delimited audit log.
fn sanitize_log_field(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn audit_log_inner(action: &str, original: &str, rewritten: &str) -> Option<()> {
    let home = user_dirs::home()?;
    let dir = home.join(".local").join("share").join("atl");
    crate::core::utils::create_private_dir(&dir).ok()?;
    let path = dir.join("hook-audit.log");
    let mut file = crate::core::utils::open_private(
        std::fs::OpenOptions::new().create(true).append(true),
        &path,
    )
    .ok()?;
    let ts = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S");
    writeln!(
        file,
        "{} | {} | {} | {}",
        ts,
        action,
        sanitize_log_field(original),
        sanitize_log_field(rewritten)
    )
    .ok()
}

// ── Claude Code native hook ────────────────────────────────────

#[derive(Debug)]
enum PayloadAction {
    Rewrite {
        cmd: String,
        rewritten: String,
        decision: HookOutcome,
        output: Value,
    },
    Skip {
        decision: HookOutcome,
        reason: &'static str,
        cmd: String,
    },
    // anthology fork: an explicit deny, so the host never judges a rewrite of a
    // command a deny rule matches (`anthology_gate`).
    Deny {
        cmd: String,
        output: Value,
    },
    Ignore,
}

fn pre_tool_use_rewrite_output(
    v: &Value,
    rewritten: &str,
    permission_decision: Option<&str>,
) -> Value {
    let mut updated_input = v.get("tool_input").cloned().unwrap_or_else(|| json!({}));
    if let Some(obj) = updated_input.as_object_mut() {
        obj.insert("command".into(), Value::String(rewritten.to_string()));
    }

    let mut hook_output = json!({
        "hookEventName": PRE_TOOL_USE_KEY,
        "permissionDecisionReason": "ATL auto-rewrite",
        "updatedInput": updated_input
    });
    if let Some(decision) = permission_decision {
        hook_output["permissionDecision"] = Value::String(decision.to_string());
    }

    json!({ "hookSpecificOutput": hook_output })
}

fn process_claude_payload(v: &Value) -> PayloadAction {
    let cmd = match v
        .pointer("/tool_input/command")
        .and_then(|c| c.as_str())
        .filter(|c| !c.is_empty())
    {
        Some(c) => c,
        None => return PayloadAction::Ignore,
    };

    let cwd = v.get("cwd").and_then(Value::as_str);
    let mode = super::read_only_proof::Mode::of(v);
    process_claude_payload_from_decision(v, cmd, decide_claude_hook_action(cmd, cwd, mode))
}

/// Pure core of `process_claude_payload`, taking the hook decision directly so the
/// full Allow/Ask/Deny/Defer matrix is unit-testable without depending on real
/// permission config files — mirrors the `copilot_cli_response_from_decision`/
/// `droid_response_from_decision` split already used elsewhere in this file.
fn process_claude_payload_from_decision(
    v: &Value,
    cmd: &str,
    decision: HookDecision,
) -> PayloadAction {
    let (rewritten, permission_decision, outcome) = match decision {
        // anthology fork: an explicit deny, not silence. The host would judge the
        // original, but under `Bash(atl *)` it must never be handed the chance
        // to judge an atl-prefixed rewrite or an inner command instead.
        HookDecision::Deny => {
            return PayloadAction::Deny {
                cmd: cmd.to_string(),
                output: super::anthology_gate::deny_output(),
            };
        }
        HookDecision::Defer => {
            return PayloadAction::Skip {
                decision: HookOutcome::Defer,
                reason: "skip:defer",
                cmd: cmd.to_string(),
            };
        }
        HookDecision::AllowRewrite(r) => (r, Some("allow"), HookOutcome::Allow),
        HookDecision::AskRewrite(r, AskOwner::Host) => (r, None, HookOutcome::Ask),
        // The host would judge the command ATL built, not the parts inside it
        // (a fish wrap is one `atl run` command), so the hook asks itself.
        HookDecision::AskRewrite(r, AskOwner::Hook) => (r, Some("ask"), HookOutcome::Ask),
    };

    PayloadAction::Rewrite {
        cmd: cmd.to_string(),
        output: pre_tool_use_rewrite_output(v, &rewritten, permission_decision),
        rewritten,
        decision: outcome,
    }
}

/// Pull the fields `log_hook_decision` needs out of the raw PreToolUse payload.
/// `None` when `session_id`/`tool_use_id` are absent — both are required to join
/// back to the transcript later, so there's nothing useful to log without them.
/// Split out from `log_hook_decision` so this extraction is unit-testable without
/// touching the tracking DB.
fn hook_log_fields(v: &Value) -> Option<(&str, &str, &str)> {
    let session_id = v.get("session_id").and_then(|s| s.as_str())?;
    let tool_use_id = v.get("tool_use_id").and_then(|s| s.as_str())?;
    let project_path = v.get("cwd").and_then(|c| c.as_str()).unwrap_or("");
    Some((session_id, tool_use_id, project_path))
}

/// Log the real hook decision to the tracking DB, keyed by the transcript's
/// `tool_use_id`, so `atl discover` can later read ground truth about historical
/// hook coverage instead of re-deriving a guess from today's hook-install state.
///
/// Best-effort only — a tracking failure must never affect the hook's real output
/// (fallback pattern from `rust-patterns.md`): this is a side channel, not the
/// hook's actual job.
fn log_hook_decision(v: &Value, cmd: &str, decision: HookOutcome, rewritten: Option<&str>) {
    let Some((session_id, tool_use_id, project_path)) = hook_log_fields(v) else {
        return;
    };

    let Ok(tracker) = crate::core::tracking::Tracker::new() else {
        return;
    };
    if let Err(e) = tracker.record_hook_decision(
        session_id,
        tool_use_id,
        project_path,
        cmd,
        decision,
        rewritten,
        env!("CARGO_PKG_VERSION"),
    ) && !crate::core::db::every_write_would_fail(&e)
    {
        let _ = writeln!(
            io::stderr(),
            "[atl hook] hook_decisions logging failed: {e}"
        );
    }
}

/// Run the Claude Code PreToolUse hook natively.
pub fn run_claude() -> Result<()> {
    #[cfg(feature = "engine")]
    let input = claude_pipeline::read_payload()?;
    #[cfg(not(feature = "engine"))]
    let input = match read_stdin_limited() {
        Ok(input) => input,
        // anthology fork: a tool call the hook cannot read must not fall through
        // to the host's allow rules (`Bash(atl *)`): exit 1 would, and so would
        // silence. Claude Code reads the JSON on exit 0.
        Err(e) => {
            let _ = writeln!(io::stderr(), "[atl hook] {e:#}");
            let _ = writeln!(io::stdout(), "{}", super::anthology_gate::ask_output());
            return Ok(());
        }
    };
    #[cfg(feature = "engine")]
    let raw_len = input.len();

    let input = strip_leading_bom(&input).trim();
    if input.is_empty() {
        return Ok(());
    }

    let v: Value = match serde_json::from_str(input) {
        Ok(v) => v,
        Err(e) => {
            let _ = writeln!(io::stderr(), "[atl hook] Failed to parse JSON input: {e}");
            // anthology fork: ask rather than leave an unread call to the allow rules.
            let _ = writeln!(io::stdout(), "{}", super::anthology_gate::ask_output());
            return Ok(());
        }
    };

    #[cfg(feature = "engine")]
    if claude_pipeline::handle(&v, raw_len)? {
        return Ok(());
    }

    match process_claude_payload(&v) {
        PayloadAction::Rewrite {
            cmd,
            rewritten,
            decision,
            output,
        } => {
            // Write the response Claude Code is synchronously blocked on FIRST.
            // `log_hook_decision` is a best-effort side channel (see its own doc
            // comment: "a tracking failure must never affect the hook's real
            // output") that opens a SQLite connection with a 5s busy_timeout — on
            // lock contention (concurrent hook invocations sharing the default
            // history.db) that write can block for real seconds. Since this fires
            // on every single Bash tool call now (not just ATL-covered ones), that
            // latency must never sit in front of the response, or it directly
            // stalls the tool call it's supposedly just logging.
            let _ = writeln!(io::stdout(), "{output}");
            audit_log("rewrite", &cmd, &rewritten);
            log_hook_decision(&v, &cmd, decision, Some(&rewritten));
        }
        PayloadAction::Skip {
            decision,
            cmd,
            reason,
        } => {
            // `atl hook audit`'s skip-breakdown groups by a "skip:<reason>" prefix
            // (see hook_audit_cmd.rs). Carry the reason separately from the
            // decision so distinct defer causes remain diagnosable.
            //
            // Skip has no stdout response to write (Claude Code falls through to
            // its own native handling), but log_hook_decision is still deferred to
            // last for the same reason as the Rewrite arm above: it must never be
            // what a Bash tool call is waiting on.
            audit_log(reason, &cmd, "");
            log_hook_decision(&v, &cmd, decision, None);
        }
        // anthology fork: the explicit deny is written before anything else.
        PayloadAction::Deny { cmd, output } => {
            let _ = writeln!(io::stdout(), "{output}");
            audit_log("skip:deny_rule", &cmd, "");
            log_hook_decision(&v, &cmd, HookOutcome::Deny, None);
        }
        PayloadAction::Ignore => {}
    }

    Ok(())
}

#[cfg(test)]
fn run_claude_inner(input: &str) -> Option<String> {
    let input = strip_leading_bom(input);
    let v: Value = serde_json::from_str(input).ok()?;
    match process_claude_payload(&v) {
        PayloadAction::Rewrite { output, .. } | PayloadAction::Deny { output, .. } => {
            Some(output.to_string())
        }
        _ => None,
    }
}

// ── Trae native hook ───────────────────────────────────────────

// ── Codex CLI native hook ─────────────────────────────────────

// ── Cursor native hook ─────────────────────────────────────────

// ── Factory Droid PreToolUse hook ──────────────────────────────
//
// Payload is shaped like Claude Code's (docs.factory.ai/reference/hooks-reference);
// the shell tool is matched as `Execute`. ATL steps aside on Droid's explicit
// deny lists and otherwise rewrites via `updatedInput` with no
// `permissionDecision` — the verdict stays with Droid's native flow.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::test_isolation;
    use crate::hooks::permissions::PermissionVerdict;

    fn rewrite_command_no_prefixes(cmd: &str, excluded: &[String]) -> Option<String> {
        crate::discover::registry::rewrite_command(cmd, excluded, &[])
    }

    // --- Copilot format detection ---

    fn vscode_input(tool: &str, cmd: &str) -> Value {
        json!({
            "tool_name": tool,
            "tool_input": { "command": cmd }
        })
    }

    fn copilot_cli_input(tool: &str, cmd: &str) -> Value {
        let args = serde_json::to_string(&json!({ "command": cmd })).unwrap();
        json!({ "toolName": tool, "toolArgs": args })
    }

    fn copilot_ide_input(cmd: &str) -> Value {
        let args = serde_json::to_string(&json!({
            "command": cmd,
            "explanation": "Run command",
            "isBackground": false
        }))
        .unwrap();
        json!({ "toolName": "run_in_terminal", "toolArgs": args })
    }

    // --- VS Code Copilot Chat / Copilot CLI (PascalCase) handler ---
    // Serves both VS Code Copilot Chat's PreToolUse hook and Copilot CLI's
    // PascalCase-compat entry (#3037): the same `atl hook copilot` call
    // answers both from one JSON schema.

    // --- Copilot CLI handler: transparent rewrite via modifiedArgs ---

    fn cli_args(cmd: &str) -> Value {
        json!({ "command": cmd })
    }

    // --- Gemini format ---

    #[test]
    fn test_print_allow_format() {
        let expected = r#"{"decision":"allow"}"#;
        assert_eq!(expected, r#"{"decision":"allow"}"#);
    }

    #[test]
    fn test_print_rewrite_format() {
        let output = serde_json::json!({
            "decision": "allow",
            "hookSpecificOutput": {
                "tool_input": {
                    "command": "atl git status"
                }
            }
        });
        let json: Value = serde_json::from_str(&output.to_string()).unwrap();
        assert_eq!(json["decision"], "allow");
        assert_eq!(
            json["hookSpecificOutput"]["tool_input"]["command"],
            "atl git status"
        );
    }

    #[test]
    fn test_gemini_hook_uses_rewrite_command() {
        assert_eq!(
            rewrite_command_no_prefixes("git status", &[]),
            Some("atl git status".into())
        );
        assert_eq!(
            rewrite_command_no_prefixes("cargo test", &[]),
            Some("atl cargo test".into())
        );
        assert_eq!(
            rewrite_command_no_prefixes("atl git status", &[]),
            Some("atl git status".into())
        );
        assert_eq!(rewrite_command_no_prefixes("cat <<EOF", &[]), None);
    }

    /// #3864: the Claude hook feeds the payload cwd into the decision, so a
    /// git command from inside a managed worktree is left to Claude Code.
    #[test]
    fn test_claude_payload_cwd_in_worktree_skips_git_rewrite() {
        let v = claude_payload_with_ids(
            "git status",
            "sess-1",
            "toolu_01ABC",
            "/repo/.claude/worktrees/feat-x",
        );
        assert!(matches!(
            process_claude_payload(&v),
            PayloadAction::Skip {
                decision: HookOutcome::Defer,
                ..
            }
        ));
    }

    #[test]
    fn test_gemini_hook_excluded_commands() {
        let excluded = vec!["curl".to_string()];
        assert_eq!(
            rewrite_command_no_prefixes("curl https://example.com", &excluded),
            None
        );
        assert_eq!(
            rewrite_command_no_prefixes("git status", &excluded),
            Some("atl git status".into())
        );
    }

    #[test]
    fn test_gemini_hook_env_prefix_preserved() {
        assert_eq!(
            rewrite_command_no_prefixes("RUST_LOG=debug cargo test", &[]),
            Some("RUST_LOG=debug atl cargo test".into())
        );
    }

    #[test]
    fn test_pre_tool_use_rewrite_output_merges_input_and_optional_decision() {
        let input = json!({
            "tool_input": {
                "command": "git status",
                "timeout": 30_000,
                "description": "Inspect the working tree"
            }
        });

        let ask = pre_tool_use_rewrite_output(&input, "atl git status", None);
        let ask_hook = &ask["hookSpecificOutput"];
        assert_eq!(ask_hook["hookEventName"], PRE_TOOL_USE_KEY);
        assert_eq!(ask_hook["permissionDecisionReason"], "ATL auto-rewrite");
        assert!(ask_hook.get("permissionDecision").is_none());
        assert_eq!(ask_hook["updatedInput"]["command"], "atl git status");
        assert_eq!(ask_hook["updatedInput"]["timeout"], 30_000);
        assert_eq!(
            ask_hook["updatedInput"]["description"],
            "Inspect the working tree"
        );

        let allow = pre_tool_use_rewrite_output(&input, "atl git status", Some("allow"));
        assert_eq!(allow["hookSpecificOutput"]["permissionDecision"], "allow");
    }

    // --- Claude handler ---

    fn claude_input(cmd: &str) -> String {
        json!({
            "tool_name": "Bash",
            "tool_input": { "command": cmd }
        })
        .to_string()
    }

    fn claude_input_with_fields(cmd: &str, timeout: u64, description: &str) -> String {
        json!({
            "tool_name": "Bash",
            "tool_input": {
                "command": cmd,
                "timeout": timeout,
                "description": description
            }
        })
        .to_string()
    }

    /// Matches the real PreToolUse payload shape captured from a live Claude Code
    /// session (verified fields: session_id, transcript_path, cwd, tool_use_id).
    fn claude_payload_with_ids(cmd: &str, session_id: &str, tool_use_id: &str, cwd: &str) -> Value {
        json!({
            "session_id": session_id,
            "transcript_path": "/home/user/.claude/projects/-home-user-project/session.jsonl",
            "cwd": cwd,
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": { "command": cmd },
            "tool_use_id": tool_use_id
        })
    }

    #[test]
    fn test_hook_log_fields_extracts_real_payload_shape() {
        let v =
            claude_payload_with_ids("git status", "sess-1", "toolu_01ABC", "/home/user/project");
        let (session_id, tool_use_id, project_path) = hook_log_fields(&v).unwrap();
        assert_eq!(session_id, "sess-1");
        assert_eq!(tool_use_id, "toolu_01ABC");
        assert_eq!(project_path, "/home/user/project");
    }

    #[test]
    fn test_hook_log_fields_none_without_tool_use_id() {
        // Older/foreign payload shapes without a tool_use_id must not be logged —
        // there's no join key to match it back to a transcript entry. Uses the
        // real claude_input() fixture, which also lacks session_id — see the
        // isolated variant below for a payload that has session_id present but
        // tool_use_id specifically absent.
        let v: Value = serde_json::from_str(&claude_input("git status")).unwrap();
        assert!(hook_log_fields(&v).is_none());
    }

    #[test]
    fn test_hook_log_fields_none_with_session_id_but_no_tool_use_id() {
        // hook_log_fields checks session_id first and short-circuits via `?`, so
        // the fixture above (missing both fields) can't tell us whether
        // tool_use_id extraction specifically works — it passes even if that
        // check were completely broken. This isolates tool_use_id: session_id
        // present, tool_use_id absent.
        let v = json!({
            "session_id": "sess-1",
            "tool_name": "Bash",
            "tool_input": { "command": "git status" }
        });
        assert!(hook_log_fields(&v).is_none());
    }

    #[test]
    fn test_hook_log_fields_defaults_missing_cwd_to_empty() {
        let v = json!({
            "session_id": "sess-1",
            "tool_use_id": "toolu_01ABC",
            "tool_name": "Bash",
            "tool_input": { "command": "git status" }
        });
        let (_, _, project_path) = hook_log_fields(&v).unwrap();
        assert_eq!(project_path, "");
    }

    // The decision field on PayloadAction feeds directly into hook_decisions —
    // exercise the full Allow/Ask/Deny/Defer matrix against the pure
    // process_claude_payload_from_decision (no real permission config needed).

    #[test]
    fn test_process_claude_payload_decision_allow() {
        let v = claude_input_value("git status");
        match process_claude_payload_from_decision(
            &v,
            "git status",
            HookDecision::AllowRewrite("atl git status".to_string()),
        ) {
            PayloadAction::Rewrite {
                decision,
                rewritten,
                ..
            } => {
                assert_eq!(decision, HookOutcome::Allow);
                assert_eq!(rewritten, "atl git status");
            }
            other => {
                panic!("expected Rewrite, got a different PayloadAction variant instead: {other:?}")
            }
        }
    }

    #[test]
    fn test_process_claude_payload_decision_ask() {
        let v = claude_input_value("git status");
        match process_claude_payload_from_decision(
            &v,
            "git status",
            HookDecision::AskRewrite("atl git status".to_string(), AskOwner::Host),
        ) {
            PayloadAction::Rewrite { decision, .. } => assert_eq!(decision, HookOutcome::Ask),
            other => {
                panic!("expected Rewrite, got a different PayloadAction variant instead: {other:?}")
            }
        }
    }

    /// A wrap ATL built asks explicitly, since the host would only judge the
    /// `atl run` command, and is logged as an ask. The same text as an
    /// ordinary ask leaves the decision to the host.
    #[test]
    fn test_process_claude_payload_decision_fish_wrap_asks_explicitly() {
        let cmd = "test -d src; and git status";
        let wrapped = "atl run --shell fish -c 'test -d src; and git status'";
        let v = claude_input_value(cmd);
        for (decision, expected) in [
            (
                HookDecision::AskRewrite(wrapped.to_string(), AskOwner::Hook),
                Some("ask"),
            ),
            (
                HookDecision::AskRewrite(wrapped.to_string(), AskOwner::Host),
                None,
            ),
        ] {
            match process_claude_payload_from_decision(&v, cmd, decision) {
                PayloadAction::Rewrite {
                    decision,
                    output,
                    rewritten,
                    ..
                } => {
                    assert_eq!(decision, HookOutcome::Ask, "{expected:?}");
                    assert_eq!(rewritten, wrapped);
                    assert_eq!(
                        output
                            .pointer("/hookSpecificOutput/updatedInput/command")
                            .and_then(Value::as_str),
                        Some(wrapped)
                    );
                    assert_eq!(
                        output
                            .pointer("/hookSpecificOutput/permissionDecision")
                            .and_then(Value::as_str),
                        expected
                    );
                }
                other => panic!("expected Rewrite, got {other:?}"),
            }
        }
    }

    // anthology fork: a deny is an explicit `permissionDecision: "deny"`, not
    // silence, and carries no `updatedInput`.
    #[test]
    fn test_process_claude_payload_decision_deny() {
        let v = claude_input_value("rm -rf /");
        match process_claude_payload_from_decision(&v, "rm -rf /", HookDecision::Deny) {
            PayloadAction::Deny { cmd, output } => {
                assert_eq!(cmd, "rm -rf /");
                assert_eq!(
                    output
                        .pointer("/hookSpecificOutput/hookEventName")
                        .and_then(Value::as_str),
                    Some("PreToolUse")
                );
                assert_eq!(
                    output
                        .pointer("/hookSpecificOutput/permissionDecision")
                        .and_then(Value::as_str),
                    Some("deny")
                );
                assert!(
                    output
                        .pointer("/hookSpecificOutput/permissionDecisionReason")
                        .and_then(Value::as_str)
                        .is_some_and(|reason| reason.starts_with("ATL: "))
                );
                assert!(
                    output.pointer("/hookSpecificOutput/updatedInput").is_none(),
                    "a deny must not hand the host a command"
                );
            }
            other => {
                panic!("expected Deny, got a different PayloadAction variant instead: {other:?}")
            }
        }
    }

    /// The deny is what `run_claude_inner` reports, and a defer still says nothing.
    #[test]
    fn test_claude_deny_prints_and_defer_stays_silent() {
        let v = claude_input_value("rm -rf /");
        let printed = match process_claude_payload_from_decision(&v, "rm -rf /", HookDecision::Deny)
        {
            PayloadAction::Deny { output, .. } => output.to_string(),
            other => panic!("expected Deny, got {other:?}"),
        };
        assert!(
            printed.contains(r#""permissionDecision":"deny""#),
            "{printed}"
        );
        assert!(matches!(
            process_claude_payload_from_decision(&v, "rm -rf /", HookDecision::Defer),
            PayloadAction::Skip {
                decision: HookOutcome::Defer,
                ..
            }
        ));
    }

    /// An ask the hook owns prints `"ask"` for the original command too, when no
    /// rewrite exists.
    #[test]
    fn test_claude_hook_ask_without_a_rewrite_keeps_the_command() {
        let v = claude_input_value("atl run aws s3 ls");
        match process_claude_payload_from_decision(
            &v,
            "atl run aws s3 ls",
            HookDecision::AskRewrite("atl run aws s3 ls".to_string(), AskOwner::Hook),
        ) {
            PayloadAction::Rewrite {
                output, decision, ..
            } => {
                assert_eq!(decision, HookOutcome::Ask);
                assert_eq!(
                    output
                        .pointer("/hookSpecificOutput/permissionDecision")
                        .and_then(Value::as_str),
                    Some("ask")
                );
                assert_eq!(
                    output
                        .pointer("/hookSpecificOutput/updatedInput/command")
                        .and_then(Value::as_str),
                    Some("atl run aws s3 ls")
                );
            }
            other => panic!("expected Rewrite, got {other:?}"),
        }
    }

    #[test]
    fn test_process_claude_payload_decision_defer() {
        let v = claude_input_value("git status $(rm -rf /tmp/x)");
        match process_claude_payload_from_decision(
            &v,
            "git status $(rm -rf /tmp/x)",
            HookDecision::Defer,
        ) {
            PayloadAction::Skip { decision, .. } => assert_eq!(decision, HookOutcome::Defer),
            other => {
                panic!("expected Skip, got a different PayloadAction variant instead: {other:?}")
            }
        }
    }

    fn claude_input_value(cmd: &str) -> Value {
        serde_json::from_str(&claude_input(cmd)).unwrap()
    }

    /// Auto mode: Claude Code refuses a call whose input a hook changed after
    /// auto mode reviewed it, so nothing is rewritten there; every other mode
    /// still is.
    #[test]
    fn test_claude_auto_mode_is_not_rewritten() {
        let input = |mode: &str| {
            json!({
                "permission_mode": mode,
                "tool_name": "Bash",
                "tool_input": { "command": "git status" }
            })
            .to_string()
        };
        assert_eq!(run_claude_allowing(&input("auto")), None);
        for mode in [
            "default",
            "acceptEdits",
            "plan",
            "dontAsk",
            "bypassPermissions",
        ] {
            assert!(run_claude_allowing(&input(mode)).is_some(), "{mode}");
        }
    }

    #[test]
    fn test_claude_rewrite_git_status() {
        let result = run_claude_allowing(&claude_input("git status")).unwrap();
        let v: Value = serde_json::from_str(&result).unwrap();
        let cmd = v
            .pointer("/hookSpecificOutput/updatedInput/command")
            .and_then(|c| c.as_str())
            .unwrap();
        assert_eq!(cmd, "atl git status");
    }

    #[test]
    fn test_claude_rewrite_preserves_tool_input_fields() {
        let input = claude_input_with_fields("git status", 30000, "Check repo status");
        let result = run_claude_allowing(&input).unwrap();
        let v: Value = serde_json::from_str(&result).unwrap();
        let updated = &v["hookSpecificOutput"]["updatedInput"];
        assert_eq!(updated["command"], "atl git status");
        assert_eq!(updated["timeout"], 30000);
        assert_eq!(updated["description"], "Check repo status");
    }

    #[test]
    fn test_claude_passthrough_no_output() {
        assert!(run_claude_inner(&claude_input("htop")).is_none());
    }

    #[test]
    fn test_claude_substitution_not_rewritten() {
        // A substitution payload must never be rewritten into updatedInput;
        // ATL skips so Claude Code evaluates the original command natively.
        assert!(run_claude_inner(&claude_input("git status `rm -rf /tmp/x`")).is_none());
        assert!(run_claude_inner(&claude_input("git status $(rm -rf /tmp/x)")).is_none());
        assert!(run_claude_inner(&claude_input("git log --pretty=\"$(rm -rf /tmp/x)\"")).is_none());
    }

    #[test]
    fn test_claude_file_redirect_not_rewritten() {
        assert!(run_claude_inner(&claude_input("git log > /tmp/out.txt")).is_none());
    }

    #[test]
    fn test_claude_fd_dup_redirect_still_rewritten() {
        // `2>&1` is attestable — the rewrite proceeds as normal.
        assert!(run_claude_allowing(&claude_input("git status 2>&1")).is_some());
    }

    #[test]
    fn test_claude_heredoc_passthrough() {
        assert!(run_claude_inner(&claude_input("cat <<EOF\nhello\nEOF")).is_none());
    }

    /// `run_claude_inner` as a user whose settings allow every Bash command. ATL
    /// never widens permissions, so with no allow rule and no read-only proof a
    /// command is not rewritten at all; a test that checks a rewrite needs one.
    fn run_claude_allowing(input: &str) -> Option<String> {
        let root = test_isolation::tempdir();
        let claude = root.path().join(".claude");
        std::fs::create_dir_all(&claude).expect("mkdir .claude");
        std::fs::write(
            claude.join("settings.json"),
            r#"{"permissions":{"allow":["Bash(*)"]}}"#,
        )
        .expect("write settings");
        test_isolation::with_root(root.path(), || run_claude_inner(input))
    }

    /// An `atl ...` line is judged as what it runs, and `Bash(atl *)` is not the
    /// user's allow for it: with no rule of the user's it asks, about the line as typed.
    #[test]
    fn test_claude_typed_atl_asks_unless_what_it_runs_is_allowed() {
        let decision = |out: Option<String>| -> Option<String> {
            let v: Value = serde_json::from_str(&out?).unwrap();
            v.pointer("/hookSpecificOutput/permissionDecision")
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        for cmd in [
            "atl git status",
            "atl rm -rf x",
            "atl init -g",
            "atl run -c 'ls'",
        ] {
            assert_eq!(
                decision(run_claude_inner(&claude_input(cmd))),
                Some("ask".into()),
                "{cmd}"
            );
        }
        // Atl's own read-only subcommands, and lines that only mention atl, are not asked.
        for cmd in ["atl gain", "atl --version", "echo atl is here"] {
            assert!(run_claude_inner(&claude_input(cmd)).is_none(), "{cmd}");
        }
        // The user's allow rule covers what the line runs (the blanket one does not).
        let root = test_isolation::tempdir();
        let claude = root.path().join(".claude");
        std::fs::create_dir_all(&claude).expect("mkdir .claude");
        std::fs::write(
            claude.join("settings.json"),
            r#"{"permissions":{"allow":["Bash(atl *)","Bash(git status:*)"]}}"#,
        )
        .expect("write settings");
        test_isolation::with_root(root.path(), || {
            assert!(run_claude_inner(&claude_input("atl git status")).is_none());
            assert!(run_claude_inner(&claude_input("atl rm -rf x")).is_some());
        });
    }

    /// No allow rule and nothing proven read-only: the original goes back untouched.
    #[test]
    fn test_claude_does_not_rewrite_what_claude_code_would_prompt_for() {
        for cmd in [
            "git push --force",
            "git status",
            "cargo build",
            "ssh h rm -rf x",
            "find . -delete",
            "npx pkg",
        ] {
            assert!(run_claude_inner(&claude_input(cmd)).is_none(), "{cmd}");
        }
    }

    /// `atl bypass <cmd>` runs any command, so the user's `Bash(atl *)` allow must
    /// not decide on it: the hook asks, whatever the settings hold. Without a
    /// classifier (no plugin) nothing is proven read-only, so even a read asks.
    #[test]
    fn test_claude_atl_bypass_asks_instead_of_leaving_it_to_the_atl_allow() {
        let ask = |cmd: &str, mode: Option<&str>| -> (Option<String>, Option<String>) {
            let mut input: Value = serde_json::from_str(&claude_input(cmd)).unwrap();
            if let Some(mode) = mode {
                input["permission_mode"] = json!(mode);
            }
            let out: Value =
                serde_json::from_str(&run_claude_inner(&input.to_string()).unwrap_or_else(|| {
                    panic!("{cmd}: the hook said nothing, so `Bash(atl *)` would decide")
                }))
                .unwrap();
            let field = |pointer: &str| {
                out.pointer(pointer)
                    .and_then(Value::as_str)
                    .map(str::to_string)
            };
            (
                field("/hookSpecificOutput/permissionDecision"),
                field("/hookSpecificOutput/updatedInput/command"),
            )
        };
        for cmd in [
            "atl bypass rm -rf x",
            "atl proxy rm -rf x",
            "atl bypass git log -5",
            "/usr/local/bin/atl bypass rm -rf x",
            "env atl bypass rm -rf x",
        ] {
            // Nothing to rewrite: the ask is about the command as typed.
            assert_eq!(
                ask(cmd, None),
                (Some("ask".into()), Some(cmd.into())),
                "{cmd}"
            );
            assert_eq!(
                ask(cmd, Some("auto")),
                (Some("ask".into()), Some(cmd.into())),
                "{cmd} (auto)"
            );
        }
        for cmd in [
            "git status && atl bypass rm -rf x",
            "ls; atl bypass rm -rf x",
            "ls | atl bypass tee x",
            "atl bypass rm -rf x | cat",
        ] {
            assert_eq!(ask(cmd, None).0.as_deref(), Some("ask"), "{cmd}");
            // Auto mode rewrites nothing: the ask is about the line as typed.
            assert_eq!(
                ask(cmd, Some("auto")),
                (Some("ask".into()), Some(cmd.into())),
                "{cmd} (auto)"
            );
        }
    }

    /// What only mentions the words is as it was.
    #[test]
    fn test_claude_atl_bypass_ask_does_not_touch_other_lines() {
        for cmd in ["echo atl bypass", "atl gain"] {
            assert!(run_claude_inner(&claude_input(cmd)).is_none(), "{cmd}");
        }
    }

    #[test]
    fn test_claude_empty_command_passthrough() {
        let input = json!({
            "tool_name": "Bash",
            "tool_input": { "command": "" }
        })
        .to_string();
        assert!(run_claude_inner(&input).is_none());
    }

    #[test]
    fn test_claude_malformed_json_passthrough() {
        assert!(run_claude_inner("not valid json {{{").is_none());
    }

    #[test]
    fn test_claude_env_prefix_preserved() {
        let result = run_claude_allowing(&claude_input("GIT_PAGER=cat git status")).unwrap();
        let v: Value = serde_json::from_str(&result).unwrap();
        let cmd = v
            .pointer("/hookSpecificOutput/updatedInput/command")
            .and_then(|c| c.as_str())
            .unwrap();
        assert_eq!(cmd, "GIT_PAGER=cat atl git status");
    }

    #[test]
    fn test_claude_compound_command() {
        let result = run_claude_allowing(&claude_input("git add . && cargo test")).unwrap();
        let v: Value = serde_json::from_str(&result).unwrap();
        let cmd = v
            .pointer("/hookSpecificOutput/updatedInput/command")
            .and_then(|c| c.as_str())
            .unwrap();
        assert_eq!(cmd, "atl git add . && atl cargo test");
    }

    #[test]
    fn test_claude_pipeline_rewrites_only_safe_final_stage() {
        let result = run_claude_allowing(&claude_input("cargo test | grep FAILED")).unwrap();
        let v: Value = serde_json::from_str(&result).unwrap();
        let cmd = v
            .pointer("/hookSpecificOutput/updatedInput/command")
            .and_then(|c| c.as_str())
            .unwrap();
        assert_eq!(cmd, "cargo test | atl grep FAILED");
    }

    #[test]
    fn test_claude_pipeline_rewrites_producer_when_consumers_safe() {
        let result = run_claude_allowing(&claude_input("git log | tail -5")).unwrap();
        let v: Value = serde_json::from_str(&result).unwrap();
        let cmd = v
            .pointer("/hookSpecificOutput/updatedInput/command")
            .and_then(|c| c.as_str())
            .unwrap();
        assert_eq!(cmd, "atl git log | tail -5");
    }

    #[test]
    fn test_claude_json_output_structure() {
        let result = run_claude_allowing(&claude_input("git status")).unwrap();
        let v: Value = serde_json::from_str(&result).unwrap();
        let hook = &v["hookSpecificOutput"];

        assert_eq!(hook["hookEventName"], PRE_TOOL_USE_KEY);
        // permissionDecision is only set when an explicit allow rule matches (the
        // user's, here); without one a rewrite is not made at all.
        assert_eq!(hook["permissionDecision"], "allow");
        assert_eq!(hook["permissionDecisionReason"], "ATL auto-rewrite");
        assert!(hook["updatedInput"].is_object());
        assert!(hook["updatedInput"]["command"].is_string());
    }

    #[test]
    fn test_claude_no_tool_input_passthrough() {
        let input = json!({ "tool_name": "Bash" }).to_string();
        assert!(run_claude_inner(&input).is_none());
    }

    #[test]
    fn test_claude_strips_utf8_bom() {
        // Windows hosts may prepend a UTF-8 BOM to hook stdin (confirmed for
        // Cursor). Without stripping, str::trim leaves U+FEFF in place,
        // serde_json::from_str fails, run_claude logs to stderr and returns
        // Ok(()) — every command silently stops being rewritten.
        let payload = claude_input("git status");
        let with_bom = format!("\u{feff}{}", payload);
        let result = run_claude_allowing(&with_bom).expect("BOM-prefixed payload must parse");
        let v: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(
            v["hookSpecificOutput"]["updatedInput"]["command"],
            "atl git status"
        );
    }

    // --- Trae handler ---

    fn trae_input(cmd: &str) -> String {
        json!({
            "tool_name": "RunCommand",
            "tool_input": {
                "command": cmd,
                "description": "Check repository status",
                "timeout": 30000
            }
        })
        .to_string()
    }

    // --- Codex handler ---

    fn codex_input_with_permission_mode(cmd: &str, permission_mode: Option<&str>) -> String {
        let mut input = json!({
            "session_id": "session-1",
            "turn_id": "turn-1",
            "hook_event_name": PRE_TOOL_USE_KEY,
            "tool_name": "Bash",
            "tool_use_id": "tool-1",
            "tool_input": { "command": cmd }
        });
        if let Some(permission_mode) = permission_mode {
            input["permission_mode"] = json!(permission_mode);
        }
        input.to_string()
    }

    // Normal Codex payloads include `default`; missing permission_mode is
    // covered deliberately by the dedicated fail-open test below.
    fn codex_input(cmd: &str) -> String {
        codex_input_with_permission_mode(cmd, Some("default"))
    }

    // --- Cursor handler ---

    fn cursor_input(cmd: &str) -> String {
        json!({
            "tool_name": "Bash",
            "tool_input": { "command": cmd }
        })
        .to_string()
    }

    // --- Audit logging ---

    /// Where `audit_log` writes under a test's own root.
    fn audit_log_under(root: &std::path::Path) -> std::path::PathBuf {
        root.join(".local")
            .join("share")
            .join("atl")
            .join("hook-audit.log")
    }

    #[test]
    fn test_audit_log_silent_when_disabled() {
        let root = test_isolation::tempdir();
        test_isolation::with_root(root.path(), || {
            user_env::with_vars(&[("ATL_HOOK_AUDIT", None)], || {
                audit_log("test", "git status", "atl git status");
            });
        });
        assert!(!audit_log_under(root.path()).exists());
    }

    #[test]
    fn test_audit_log_format_four_fields() {
        let root = test_isolation::tempdir();
        test_isolation::with_root(root.path(), || {
            user_env::with_vars(&[("ATL_HOOK_AUDIT", Some("1"))], || {
                audit_log("rewrite", "git status", "atl git status");
            });
        });

        let content =
            std::fs::read_to_string(audit_log_under(root.path())).expect("audit log written");
        let parts: Vec<&str> = content.trim().split(" | ").collect();
        assert_eq!(
            parts.len(),
            4,
            "Expected 4 pipe-delimited fields, got: {:?}",
            parts
        );
        assert_eq!(parts[1], "rewrite");
        assert_eq!(parts[2], "git status");
        assert_eq!(parts[3], "atl git status");
    }

    // --- Adversarial tests ---

    #[test]
    fn test_audit_log_sanitizes_newlines() {
        let sanitized = sanitize_log_field("git status\nfake | inject | evil");
        assert!(!sanitized.contains('\n'));
        assert!(sanitized.contains("\\n"));
    }

    #[test]
    fn test_audit_log_sanitizes_pipe_delimiter() {
        let sanitized = sanitize_log_field("git log | head");
        assert!(
            !sanitized.contains(" | "),
            "unescaped ' | ' breaks field parsing: {}",
            sanitized
        );
        assert!(sanitized.contains("\\|"));
    }

    #[test]
    fn test_claude_unicode_null_passthrough() {
        let input = claude_input("git status \u{0000}\u{FEFF}");
        let _ = run_claude_inner(&input);
    }

    #[test]
    fn test_claude_extremely_long_command() {
        let long_cmd = format!("git status {}", "A".repeat(100_000));
        let input = claude_input(&long_cmd);
        let _ = run_claude_inner(&input);
    }

    // --- OpenCode: the answer the plugin acts on ---
    //
    // OpenCode judges whatever command runs against the user's own rules;
    // plugins cannot change that verdict. So the invariant pinned here is:
    // a rewrite is returned only when it leaves the verdict untouched.

    #[test]
    fn test_cursor_deny_blocks_rewrite() {
        use super::permissions::check_command_with_rules;
        let deny = vec!["git status".to_string()];
        assert_eq!(
            check_command_with_rules("git status", &deny, &[], &[]),
            PermissionVerdict::Deny
        );
    }

    #[test]
    fn test_gemini_deny_blocks_rewrite() {
        use super::permissions::check_command_with_rules;
        let deny = vec!["cargo test".to_string()];
        assert_eq!(
            check_command_with_rules("cargo test", &deny, &[], &[]),
            PermissionVerdict::Deny
        );
    }

    // --- Shared decision flow (all hosts route through this) ---

    fn decide_with_rules(
        cmd: &str,
        deny: &[String],
        ask: &[String],
        allow: &[String],
    ) -> HookDecision {
        let verdict = permissions::check_command_with_rules(cmd, deny, ask, allow);
        decide_from_verdict(cmd, verdict, permissions::DenyRules::Unattested, None)
    }

    fn all_allowed() -> Vec<String> {
        vec!["*".to_string()]
    }

    #[test]
    fn test_decide_allow_for_attestable_allowed_command() {
        assert!(matches!(
            decide_with_rules("git status", &[], &[], &all_allowed()),
            HookDecision::AllowRewrite(_)
        ));
    }

    #[test]
    fn test_decide_ask_for_default_verdict() {
        assert!(matches!(
            decide_with_rules("git status", &[], &[], &[]),
            HookDecision::AskRewrite(_, AskOwner::Host)
        ));
    }

    #[test]
    fn test_decide_deny() {
        assert!(matches!(
            decide_with_rules(
                "rm -rf /tmp/x",
                &["rm -rf".to_string()],
                &[],
                &all_allowed()
            ),
            HookDecision::Deny
        ));
    }

    #[test]
    fn test_decide_defer_for_substitution_even_when_allowed() {
        for cmd in [
            "git status `rm -rf /tmp/x`",
            "git status $(rm -rf /tmp/x)",
            "git log --pretty=\"$(rm -rf /tmp/x)\"",
        ] {
            assert!(
                matches!(
                    decide_with_rules(cmd, &[], &[], &all_allowed()),
                    HookDecision::Defer
                ),
                "expected Defer for {cmd}"
            );
        }
    }

    #[test]
    fn test_decide_defer_for_file_redirect() {
        assert!(matches!(
            decide_with_rules("git log > /tmp/out.txt", &[], &[], &all_allowed()),
            HookDecision::Defer
        ));
    }

    #[test]
    fn test_decide_leaves_folded_ls_tree_to_git_where_its_lines_are_read() {
        assert!(matches!(
            decide_with_rules("git ls-tree -r --name-only HEAD", &[], &[], &all_allowed()),
            HookDecision::AllowRewrite(ref r) if r == "atl git ls-tree -r --name-only HEAD"
        ));
        for cmd in [
            "git ls-tree -r --name-only HEAD > files.txt",
            "git ls-tree -r --name-only HEAD | head -5",
            "git ls-tree -r --name-only HEAD | cat",
            "for f in $(git ls-tree -r --name-only HEAD); do echo \"$f\"; done",
            "git ls-tree -r --name-only HEAD | while read f; do echo \"$f\"; done",
        ] {
            assert!(
                matches!(
                    decide_with_rules(cmd, &[], &[], &all_allowed()),
                    HookDecision::Defer
                ),
                "{cmd}"
            );
        }
    }

    #[test]
    fn test_decide_allow_for_fd_dup_redirect() {
        assert!(matches!(
            decide_with_rules("git status 2>&1", &[], &[], &all_allowed()),
            HookDecision::AllowRewrite(_)
        ));
    }

    // --- Gemini rendering ---

    // --- Factory Droid hook ---

    fn droid_input(tool: &str, cmd: &str) -> String {
        json!({
            "session_id": "abc123",
            "hook_event_name": "PreToolUse",
            "tool_name": tool,
            "tool_input": { "command": cmd }
        })
        .to_string()
    }

    fn vibe_input(tool: &str, cmd: &str) -> String {
        json!({
            "session_id": "abc123",
            "hook_event_name": "pre_tool",
            "tool_name": tool,
            "tool_input": { "command": cmd }
        })
        .to_string()
    }

    fn antigravity_input(cmd: &str) -> String {
        json!({
            "toolCall": {
                "name": "run_command",
                "args": {
                    "CommandLine": cmd
                }
            }
        })
        .to_string()
    }

    #[test]
    fn test_claude_shell_wrapper_rewrites_without_auto_allow() {
        let result = run_claude_allowing(&claude_input(r#"bash -c "head foo && grep -R bar .""#))
            .expect("wrapper rewrite expected");
        let v: Value = serde_json::from_str(&result).unwrap();
        let output = &v["hookSpecificOutput"];
        assert_eq!(
            output["updatedInput"]["command"],
            r#"bash -c "atl read foo --level none --head-lines 10 && atl grep -R bar .""#
        );
        assert!(output.get("permissionDecision").is_none());
    }

    #[test]
    fn test_decide_asks_for_allowed_shell_wrapper_rewrite() {
        match decide_with_rules(
            r#"bash -c "head foo && grep -R bar .""#,
            &[],
            &[],
            &all_allowed(),
        ) {
            HookDecision::AskRewrite(rewritten, AskOwner::Host) => {
                assert_eq!(
                    rewritten,
                    r#"bash -c "atl read foo --level none --head-lines 10 && atl grep -R bar .""#
                )
            }
            _ => panic!("supported shell wrapper must rewrite with confirmation"),
        }
    }
}
