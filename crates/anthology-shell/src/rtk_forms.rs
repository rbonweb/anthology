//! rtk runner and passthrough forms (SH2): guard_rules.py `_rtk_command`, `_RTK_PASSTHROUGH`, `_RTK_RUNNERS`.

use crate::lexer::pystr;
use crate::prefixes::{drop_front, skip_flags};

/// rtk subcommands that run their arguments as a command (`proxy` aliases `bypass`).
pub const RTK_PASSTHROUGH: &[&str] = &["bypass", "proxy"];

/// rtk subcommands that run a command or a script (rtk/src/lib.rs `Commands`).
pub const RTK_RUNNERS: &[&str] = &["run", "err", "test", "summary"];

/// rtk's global options (rtk/src/lib.rs `Cli`): `-v`/`-vv`/... and `--verbose` before the subcommand, and
/// `--skip-env` anywhere. Rust-only (`Opaque::UnknownWrapperOption`).
fn known_global(token: &str, before_sub: bool) -> bool {
    token == "--skip-env"
        || (before_sub
            && (token == "--verbose"
                || (token.len() > 1
                    && token.starts_with('-')
                    && token[1..].bytes().all(|b| b == b'v'))))
}

/// Whether an option in `toks[from..to]` is not one rtk is known to take without a value.
fn unknown_option(toks: &[String], from: usize, to: usize, before_sub: bool) -> bool {
    toks.get(from..to.min(toks.len())).is_some_and(|words| {
        words
            .iter()
            .any(|t| t != "--" && !known_global(t, before_sub))
    })
}

/// `_rtk_command`: `rtk ...` → the command it runs. `bypass`/`proxy`/`run`/`err`/`test`/`summary` run a
/// command; any other subcommand wraps a fixed tool and the words after the global flags stand.
/// `rtk run -c|--command <script> [--shell X]` and `rtk err|test|summary --shell X <script>` become
/// `X -c <script>` (X defaults to `sh`); a lone argument holding whitespace is also run as a script.
/// Rust-only: `unknown` is set for an option rtk is not known to take without a value.
pub(crate) fn rtk_command(mut toks: Vec<String>, unknown: &mut bool) -> Vec<String> {
    let mut unused = false;
    let mut i = skip_flags(&toks, 1, &[], &[], &mut unused);
    *unknown |= unknown_option(&toks, 1, i, true);
    let sub = toks.get(i).map_or("", String::as_str);
    if RTK_PASSTHROUGH.contains(&sub) {
        let end = skip_flags(&toks, i + 1, &[], &[], &mut unused);
        *unknown |= unknown_option(&toks, i + 1, end, false);
        drop_front(&mut toks, end);
        return toks;
    }
    if !RTK_RUNNERS.contains(&sub) {
        drop_front(&mut toks, i);
        return toks;
    }
    let run = sub == "run";
    let (mut shell, mut script): (Option<String>, Option<String>) = (None, None);
    let n = toks.len();
    i += 1;
    while i < n {
        let token = toks[i].as_str();
        let value = || toks.get(i + 1).cloned();
        let second = token.chars().nth(1);
        if token == "--" {
            i += 1;
            break;
        }
        if token == "--shell" {
            shell = value();
            i += 2;
        } else if let Some(v) = token.strip_prefix("--shell=") {
            shell = Some(v.to_string());
            i += 1;
        } else if run && token == "--command" {
            script = value();
            i += 2;
        } else if let Some(v) = token.strip_prefix("--command=").filter(|_| run) {
            script = Some(v.to_string());
            i += 1;
        } else if run
            && token.starts_with('-')
            && second.is_some_and(|c| c != '-')
            && token.contains('c')
        {
            // `-c X`, `-vc X`, `-cX`
            let attached = token.find('c').map_or("", |at| &token[at + 1..]);
            if attached.is_empty() {
                script = value();
                i += 2;
            } else {
                script = Some(attached.to_string());
                i += 1;
            }
        } else if token.starts_with('-') && token.chars().count() > 1 {
            *unknown |= !known_global(token, false);
            i += 1; // `--skip-env`, `-v`, ...
        } else {
            break;
        }
    }
    drop_front(&mut toks, i);
    let rest = toks;
    let shell = shell.filter(|s| !s.is_empty());
    if run && let Some(script) = script {
        return vec![
            shell.unwrap_or_else(|| "sh".to_string()),
            "-c".to_string(),
            script,
        ];
    }
    if !run
        && let Some(shell) = shell
        && !rest.is_empty()
    {
        return vec![shell, "-c".to_string(), rest.join(" ")];
    }
    if rest.len() == 1 && pystr::split_ws(&rest[0]).len() > 1 {
        return vec!["sh".to_string(), "-c".to_string(), rest[0].clone()]; // `rtk err "npx playwright install"`
    }
    rest
}
