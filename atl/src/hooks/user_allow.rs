//! Anthology fork: whether the user's own allow rules cover a line, as Claude Code
//! matches them. ATL never widens permissions: the hook may leave a line to the
//! `Bash(atl *)` allow only when Claude Code would have allowed what the user typed
//! anyway, and an allow rule of the user's is one way for that to be true (the other
//! is the read-only proof, `read_only_proof`).
//!
//! The matching here is deliberately not `permissions::command_matches_pattern`,
//! which reads a rule with no wildcard as "this command or anything that starts with
//! it" (`Bash(git push)` would cover `git push --force`) and serves the deny and ask
//! rules, where matching more is the safe side. Claude Code (2.1.326, read from its
//! bundle) reads an allow rule as one of:
//!
//! - **prefix**, `Bash(prefix:*)`: the command is `prefix`, or starts with `prefix `;
//! - **wildcard**, any other rule with an unescaped `*`: `*` matches any run of
//!   characters, anchored at both ends, and a rule that ends in ` *` with that as its
//!   only `*` also matches the bare command (`npm run *` covers `npm run`);
//! - **exact**: the command is the rule, nothing else.
//!
//! A prefix or wildcard rule never covers an exec wrapper (`watch`, `setsid`,
//! `ionice`, `flock`) or a `find` that runs or writes (`-exec`, `-execdir`, `-ok`,
//! `-okdir`, `-delete`, `-fprint`, `-fprint0`, `-fprintf`, `-fls`, `-files0-from`),
//! as in Claude Code, where `Bash(find *)` does not allow `find . -delete`. Which
//! files may supply allow rules is `permissions::claude_hook_allow_files`.
//!
//! A line must be covered a simple command at a time, and a line that cannot be cut
//! into simple commands (a substitution, a redirect to a file) is covered by none.
//! The install's own blanket `Bash(atl *)` allow is not the user's allow for an `atl`
//! command: it covers every one, which is what this check exists to look behind, so
//! for a command whose word is `atl` only a rule that does not cover every `atl ...`
//! counts. Everything else is stricter than Claude Code, never looser: a rule with a
//! backslash matches nothing, and a segment keeps its words as typed (no peeling of
//! `env`, `timeout`, assignments).

use crate::discover::lexer::{contains_unattestable_construct, split_for_permissions};

/// What no allow rule may cover for an `atl` command to count it: a made-up
/// subcommand, which only a blanket rule (`atl *`, `*`) matches.
const BLANKET_PROBE: &str = "atl zzz-no-such-subcommand";

/// Whether every simple command of `line` is covered by one of `allow` (the
/// patterns inside `Bash(...)`).
pub(crate) fn user_allows(line: &str, allow: &[String]) -> bool {
    if allow.is_empty() || contains_unattestable_construct(line) {
        return false;
    }
    let mut any = false;
    for segment in split_for_permissions(line) {
        let segment = segment.trim();
        if segment.is_empty() {
            continue;
        }
        any = true;
        if !segment_allowed(segment, allow) {
            return false;
        }
    }
    any
}

/// Whether `segment` is covered by a rule: for an `atl` command, by one that is not
/// blanket.
fn segment_allowed(segment: &str, allow: &[String]) -> bool {
    let atl = segment
        .split([' ', '\t'])
        .next()
        .is_some_and(|word| matches!(basename(word), "atl" | "rtk" /* legacy */));
    allow
        .iter()
        .filter(|rule| !atl || !matches(BLANKET_PROBE, rule))
        .any(|rule| matches(segment, rule))
}

fn basename(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

/// One rule against one command, as Claude Code matches an allow rule.
pub(crate) fn matches(command: &str, rule: &str) -> bool {
    let rule = rule.trim();
    if rule.is_empty() || rule.contains('\\') {
        return false;
    }
    // Claude Code never lets a prefix or wildcard rule cover these (only an exact
    // rule, where the user wrote the whole line).
    if rule.contains('*') && runs_what_follows(command) {
        return false;
    }
    if let Some(prefix) = rule.strip_suffix(":*")
        && !prefix.contains('*')
    {
        let prefix = prefix.trim_end();
        return command == prefix
            || command
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with(' '));
    }
    if !rule.contains('*') {
        return command == rule;
    }
    // A trailing ` *`, the only wildcard, makes the arguments optional.
    if let Some(prefix) = rule.strip_suffix(" *")
        && !prefix.contains('*')
        && command == prefix
    {
        return true;
    }
    glob(command.as_bytes(), rule.as_bytes())
}

/// The exec wrappers Claude Code's prefix and wildcard rules never cover.
const EXEC_WRAPPERS: [&str; 4] = ["watch", "setsid", "ionice", "flock"];

/// The `find` options that run a command or write a file.
const FIND_RUNNING_OPTIONS: [&str; 10] = [
    "-exec",
    "-execdir",
    "-ok",
    "-okdir",
    "-delete",
    "-fprint",
    "-fprint0",
    "-fprintf",
    "-fls",
    "-files0-from",
];

/// Whether `command` starts with an exec wrapper, or is a `find` that runs or
/// writes. Quotes are ignored, so `'-delete'` counts, and a path to the program
/// is the program.
fn runs_what_follows(command: &str) -> bool {
    let mut words = command.split([' ', '\t']).filter(|word| !word.is_empty());
    let Some(first) = words.next() else {
        return false;
    };
    let program = basename(first.trim_matches(['\'', '"']));
    if EXEC_WRAPPERS.contains(&program) {
        return true;
    }
    program == "find"
        && words.any(|word| FIND_RUNNING_OPTIONS.contains(&word.trim_matches(['\'', '"'])))
}

/// `*` matches any run of bytes (including none); the rest is literal; both ends are
/// anchored.
fn glob(text: &[u8], pattern: &[u8]) -> bool {
    let (mut t, mut p) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while t < text.len() {
        if p < pattern.len() && pattern[p] == b'*' {
            star = Some(p);
            mark = t;
            p += 1;
        } else if p < pattern.len() && pattern[p] == text[t] {
            t += 1;
            p += 1;
        } else if let Some(s) = star {
            mark += 1;
            t = mark;
            p = s + 1;
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|&b| b == b'*')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn an_allow_rule_matches_as_claude_code_matches_it() {
        let cases: &[(&str, &str, bool)] = &[
            // exact: only the command
            ("git push", "git push", true),
            ("git push --force", "git push", false),
            ("git pushx", "git push", false),
            // prefix
            ("cargo build", "cargo build:*", true),
            ("cargo build --release", "cargo build:*", true),
            ("cargo builder", "cargo build:*", false),
            ("cargo", "cargo build:*", false),
            // wildcard, trailing ` *`: the bare command too
            ("npm run", "npm run *", true),
            ("npm run build", "npm run *", true),
            ("npm runx", "npm run *", false),
            ("npm install", "npm run *", false),
            // no space: a plain prefix glob
            ("lsof -i", "ls*", true),
            // wildcard elsewhere
            ("git -C x status", "git * status", true),
            ("git status", "git * status", false),
            ("docker run alpine", "docker * alpine", true),
            ("docker run alpine x", "docker * alpine", false),
            // everything
            ("anything at all", "*", true),
            // never a rule with an escape
            ("echo *", "echo \\*", false),
            ("", "", false),
        ];
        for (command, rule, want) in cases {
            assert_eq!(matches(command, rule), *want, "{command:?} vs {rule:?}");
        }
    }

    /// Claude Code's own exceptions: a `*` or `:*` rule never covers an exec wrapper
    /// or a `find` that runs or writes; an exact rule is the exact command.
    #[test]
    fn a_star_rule_never_covers_an_exec_wrapper_or_a_find_that_runs_or_writes() {
        let cases: &[(&str, &str, bool)] = &[
            ("find . -delete", "find *", false),
            ("find . -delete", "find:*", false),
            ("find . -delete", "*", false),
            ("find . -delete", "find . *", false),
            ("find . -exec rm {} ;", "find *", false),
            ("find . -execdir rm {} ;", "find *", false),
            ("find . -ok rm {} ;", "find *", false),
            ("find . -okdir rm {} ;", "find *", false),
            ("find . -fprint o", "find *", false),
            ("find . -fprint0 o", "find *", false),
            ("find . -fprintf o x", "find *", false),
            ("find . -fls o", "find *", false),
            ("find . -files0-from l", "find *", false),
            ("find . '-delete'", "find *", false),
            ("/usr/bin/find . -delete", "/usr/bin/find *", false),
            ("find", "find *", true),
            ("find . -name x", "find *", true),
            ("find . -name -deleted", "find *", true),
            ("watch touch x", "watch *", false),
            ("watch touch x", "watch:*", false),
            ("watch touch x", "*", false),
            ("setsid touch x", "setsid *", false),
            ("ionice touch x", "ionice:*", false),
            ("flock f touch x", "flock *", false),
            ("/usr/bin/flock f touch x", "*", false),
            // An exact rule is the whole line, whatever it holds.
            ("find . -delete", "find . -delete", true),
            ("watch touch x", "watch touch x", true),
            // Not an exception: other launchers and other finds.
            ("timeout 5 ls", "timeout *", true),
            ("findutils --version", "findutils *", true),
        ];
        for (command, rule, want) in cases {
            assert_eq!(matches(command, rule), *want, "{command:?} vs {rule:?}");
        }
        // Through `atl`, the line is judged as what it runs.
        let allow = rules(&["atl *", "find *", "watch *"]);
        for line in [
            "find . -delete",
            "watch touch /tmp/pwn",
            "find . -name x && find . -delete",
        ] {
            assert!(!user_allows(line, &allow), "{line}");
        }
        assert!(user_allows("find . -name x", &allow));
    }

    #[test]
    fn a_line_is_covered_a_simple_command_at_a_time() {
        let allow = rules(&["git status", "cargo test:*"]);
        for line in [
            "git status",
            "cargo test -p atl",
            "git status && cargo test",
            "git status; cargo test x | cargo test y",
        ] {
            assert!(user_allows(line, &allow), "{line}");
        }
        for line in [
            "git status -s",
            "git status && rm -rf x",
            "rm -rf x; git status",
            "git status > out.txt",
            "git status $(rm x)",
            "",
        ] {
            assert!(!user_allows(line, &allow), "{line}");
        }
        assert!(!user_allows("git status", &[]));
    }

    #[test]
    fn the_blanket_atl_allow_is_not_the_users_allow_for_an_atl_command() {
        let blanket = rules(&["atl *"]);
        for line in [
            "atl init -g",
            "atl rm -rf x",
            "/usr/bin/atl bypass x",
            "atl",
        ] {
            assert!(!user_allows(line, &blanket), "{line}");
        }
        // Any rule that covers every atl command is blanket.
        for allow in [&["*"][..], &["atl:*"], &["atl *"], &["atl"]] {
            assert!(!user_allows("atl init -g", &rules(allow)), "{allow:?}");
        }
        // A rule of the user's own for one atl command counts.
        let own = rules(&["atl *", "atl init:*", "atl config --create"]);
        assert!(user_allows("atl init -g", &own));
        assert!(user_allows("atl config --create", &own));
        assert!(!user_allows("atl config --reset", &own));
        // A command that is not atl is covered by any rule, the blanket one too.
        assert!(user_allows("ls -la", &rules(&["*"])));
    }
}
