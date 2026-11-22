//! G4: agent-browser (guard_rules.py `_g4`, `_names_agent_browser`).

use anthology_pycompat::str as pystr;
use anthology_shell::Entry;

use super::{JS_PACKAGE_MANAGERS, first_arg};

/// `_G4_EXEMPT_PROGRAMS`: programs that only name, print or move the word.
const G4_EXEMPT_PROGRAMS: &[&str] = &[
    "grep", "egrep", "fgrep", "rg", "ag", "ack", "git", "echo", "printf", "which", "type", "ls",
    "cat", "head", "tail", "less", "wc", "file", "stat", "rm",
    // also only name paths and cannot run them (`find -exec` is analyzed as its own command)
    "cd", "pushd", "mkdir", "touch", "cp", "mv", "find", "du", "diff", "tree", "realpath",
    "basename", "dirname", "readlink", "chmod", "chown", "ln",
];
const UNINSTALL_VERBS: &[&str] = &["uninstall", "remove", "rm", "un", "r"];
const INSPECT_VERBS: &[&str] = &["ls", "list", "view", "info", "why", "outdated"];
const BREW_VERBS: &[&str] = &["uninstall", "remove", "rm", "info", "list", "search"];

/// `_names_agent_browser(token)`: the last path part, before any `@` or `:`, is
/// `agent-browser`; a URL never is.
fn names_agent_browser(token: &str) -> bool {
    if token.contains("://") {
        return false;
    }
    let lowered = pystr::lower(pystr::strip_ws(token));
    let last = lowered.rsplit('/').next().unwrap_or("");
    last.split(['@', ':']).next() == Some("agent-browser")
}

/// `_g4(entry)`.
pub fn g4(entry: &Entry) -> bool {
    let prog = entry.prog.as_str();
    if G4_EXEMPT_PROGRAMS.contains(&prog) {
        return false;
    }
    let verb = first_arg(&entry.args);
    if JS_PACKAGE_MANAGERS.contains(&prog)
        && verb.is_some_and(|v| UNINSTALL_VERBS.contains(&v) || INSPECT_VERBS.contains(&v))
    {
        return false;
    }
    if prog == "brew" && verb.is_some_and(|v| BREW_VERBS.contains(&v)) {
        return false;
    }
    entry.raw.iter().any(|t| names_agent_browser(t))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_part_before_a_version_or_colon_counts() {
        for t in [
            "agent-browser",
            "/usr/local/bin/agent-browser",
            "agent-browser@1.2",
            " Agent-Browser:x",
        ] {
            assert!(names_agent_browser(t), "{t}");
        }
        for t in [
            "https://x/agent-browser",
            "agent-browsers",
            "my-agent-browser",
            "agent-browser/x",
        ] {
            assert!(!names_agent_browser(t), "{t}");
        }
    }
}
