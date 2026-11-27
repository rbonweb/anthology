//! Hook installation and lifecycle management for AI coding agents.

pub mod constants;
// anthology fork: the Claude hook's deny/ask/Read() enforcement past atl prefixes (C1).
pub mod anthology_gate;
// Shares `hook_cmd`'s constraint: it runs inside the hook, where stray output
// corrupts the JSON protocol. `from_agent` is the one exception and carries its
// own allow -- it is reached only from the `atl hook check` CLI.
#[deny(clippy::print_stdout, clippy::print_stderr)]
pub mod decision;
pub mod hook_audit_cmd;
pub mod hook_check;
#[deny(clippy::print_stdout, clippy::print_stderr)]
pub mod hook_cmd;
pub mod init;
pub mod integrity;
pub mod permissions;
pub mod permissions_opencode;
// anthology fork: drops the gate's needless ask for a line shown read-only, and
// rewrites nothing in auto mode.
pub mod read_only_proof;
#[deny(clippy::print_stdout, clippy::print_stderr)]
#[cfg(feature = "engine")]
pub mod posthook;
pub mod rewrite_cmd;
#[deny(clippy::print_stdout, clippy::print_stderr)]
#[cfg(feature = "engine")]
pub mod session_start;
pub mod trust;
pub mod verify_cmd;

fn is_atl_binary(binary: &str) -> bool {
    let binary_name = binary.rsplit(['/', '\\']).next().unwrap_or(binary);
    matches!(binary_name, "atl" | "atl.exe")
}

fn is_atl_hook_command(command: &str, agent: &str) -> bool {
    let mut tokens = crate::discover::lexer::tokenize_with_newlines(command);
    // Ignore only literal LF operator tokens at the command's boundaries.
    // The lexer also reports the CR of a CRLF pair as an LF operator; checking
    // the source byte leaves that token for the guard below, and the word check
    // after shell_split would reject the CR anyway.
    while tokens.first().is_some_and(|token| {
        matches!(token.kind, crate::discover::lexer::TokenKind::Operator)
            && token.value == "\n"
            && command.as_bytes().get(token.offset) == Some(&b'\n')
    }) {
        tokens.remove(0);
    }
    while tokens.last().is_some_and(|token| {
        matches!(token.kind, crate::discover::lexer::TokenKind::Operator)
            && token.value == "\n"
            && command.as_bytes().get(token.offset) == Some(&b'\n')
    }) {
        tokens.pop();
    }
    if tokens.iter().any(|token| match token.kind {
        crate::discover::lexer::TokenKind::Operator
        | crate::discover::lexer::TokenKind::Pipe(_)
        | crate::discover::lexer::TokenKind::Redirect => true,
        crate::discover::lexer::TokenKind::Shellism => token.value == "&",
        _ => false,
    }) {
        return false;
    }

    let parts = crate::discover::lexer::shell_split(command);
    let [parsed_binary, hook, target] = parts.as_slice() else {
        return false;
    };

    is_atl_binary(parsed_binary) && hook == "hook" && target == agent
}

pub fn is_claude_hook_command(command: &str) -> bool {
    is_atl_hook_command(command, "claude")
}

pub fn is_legacy_rewrite_hook(command: &str) -> bool {
    let parts = crate::discover::lexer::shell_split(command);
    let [word] = parts.as_slice() else {
        return false;
    };

    let filename = word.rsplit(['/', '\\']).next().unwrap_or(word);
    filename == constants::REWRITE_HOOK_FILE
}

#[cfg(feature = "engine")]
static HOOK_AGENT: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

#[cfg(feature = "engine")]
pub(crate) fn set_hook_agent(agent: &str) {
    let _ = HOOK_AGENT.set(Some(agent.to_string()));
}

#[cfg(feature = "engine")]
pub(crate) fn hook_agent() -> Option<String> {
    HOOK_AGENT.get().cloned().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_hook_command_matches_bare_and_absolute_atl() {
        assert!(is_claude_hook_command("atl hook claude"));
        assert!(is_claude_hook_command("/opt/homebrew/bin/atl hook claude"));
        assert!(is_claude_hook_command(
            "\"/opt/homebrew/bin/atl\" hook claude"
        ));
        assert!(is_claude_hook_command(
            "/Users/jane/My\\ Apps/atl hook claude"
        ));
    }

    #[test]
    fn claude_hook_command_rejects_other_commands() {
        assert!(!is_claude_hook_command("not-atl hook claude"));
        assert!(!is_claude_hook_command("/opt/homebrew/bin/atl hook cursor"));
        assert!(!is_claude_hook_command("echo atl hook claude"));
    }

    #[test]
    fn legacy_rewrite_hook_matches_direct_invocations() {
        assert!(is_legacy_rewrite_hook("atl-rewrite.sh"));
        assert!(is_legacy_rewrite_hook("~/.claude/hooks/atl-rewrite.sh"));
        assert!(is_legacy_rewrite_hook("/path/to/atl-rewrite.sh"));
        assert!(is_legacy_rewrite_hook("./hooks/atl-rewrite.sh"));
        assert!(is_legacy_rewrite_hook("\"/path/to/atl-rewrite.sh\""));
        assert!(is_legacy_rewrite_hook(
            "\"/Users/John Doe/.claude/hooks/atl-rewrite.sh\""
        ));
        assert!(is_legacy_rewrite_hook(
            "'C:\\Users\\test\\.claude\\hooks\\atl-rewrite.sh'"
        ));
        assert!(is_legacy_rewrite_hook(
            "C:/Users/test/.claude/hooks/atl-rewrite.sh"
        ));
    }

    #[test]
    fn legacy_rewrite_hook_rejects_non_invocations() {
        assert!(!is_legacy_rewrite_hook("echo atl-rewrite.sh"));
        assert!(!is_legacy_rewrite_hook("/custom/atl-rewrite.sh.backup"));
        assert!(!is_legacy_rewrite_hook(
            "python3 /custom/audit.py --label atl-rewrite.sh"
        ));
        assert!(!is_legacy_rewrite_hook(
            "audit.sh --mirror ~/.claude/hooks/atl-rewrite.sh"
        ));
        assert!(!is_legacy_rewrite_hook(
            "~/.claude/hooks/atl-rewrite.sh && logger -t audit done"
        ));
        assert!(!is_legacy_rewrite_hook(
            "~/.claude/hooks/atl-rewrite.sh | tee -a /tmp/audit.log"
        ));
        assert!(!is_legacy_rewrite_hook(
            "~/.claude/hooks/atl-rewrite.sh --flag"
        ));
        assert!(!is_legacy_rewrite_hook("/custom/my-atl-rewrite.sh"));
        assert!(!is_legacy_rewrite_hook(""));
    }
}
