//! Word-level refusals on top of the classifier's table, read from the same parse the classifier
//! cross-checks (`anthology_shell::analyze`): every stage's program and arguments, quotes removed.
//!
//! - `grep`, `egrep` and `fgrep` with `GREP_OPTIONS` set (BSD grep honours it), and `rg` with
//!   `RIPGREP_CONFIG_PATH` set (it can add any option): no decision.
//! - git, any word holding a signature atom (`%G…`, `%(signature…)`, `--sort=signature`): it runs
//!   gpg or ssh-keygen.
//! - git forms that print history the index scan cannot see (a file committed, then removed):
//!   `status -v`, `diff --cached` / `--staged`, a `diff` operand that could be a revision (only
//!   `./…` and `/…` cannot), every patch form of `log` (`-p -u -W -m`, `--patch`, `--word-diff`,
//!   `--color-words`, `--binary`, `--stat` and the other diff-output options, `-L`, and the
//!   content searches `-S` and `-G`), and `show`, `cat-file` and `grep` outright. `blame` stays,
//!   since it prints the tracked file as it is, but not with an operand that could be a revision.

use anthology_core::Env;
use anthology_shell::analyze;

use super::shown;

/// The `log` options that print a patch, a diff of history, or its file list.
const LOG_PATCH_LONG: &[&str] = &[
    "--patch",
    "--word-diff",
    "--color-words",
    "--binary",
    "--stat",
    "--numstat",
    "--shortstat",
    "--dirstat",
    "--summary",
    "--unified",
    "--submodule",
    "--function-context",
    "--cc",
    "--diff-merges",
    "--remerge-diff",
    "--full-diff",
    "--name-only",
    "--name-status",
    "--raw",
    "--pickaxe-regex",
    "--pickaxe-all",
    "--find-object",
];

/// `log` short options that print a patch (`-p`, `-u`), shape one (`-W`), show merges' diffs
/// (`-m`, `-c`), trace lines through history (`-L`), or search the content of history (`-S`, `-G`).
const LOG_PATCH_SHORT: &[char] = &['p', 'u', 'W', 'm', 'c', 'L', 'S', 'G'];

pub fn check(command: &str, home: &str, env: &Env) -> Result<(), String> {
    for entry in analyze(command, home).entries {
        // `prog` is the program word's name (`/usr/bin/git` is `git`), past every wrapper.
        let args = &entry.args;
        match entry.prog.as_str() {
            "grep" | "egrep" | "fgrep" if env.get_os("GREP_OPTIONS").is_some() => {
                return Err("GREP_OPTIONS is set, and grep reads its options from it".into());
            }
            "rg" if env.get_os("RIPGREP_CONFIG_PATH").is_some() => {
                return Err("RIPGREP_CONFIG_PATH is set, and rg reads its options from it".into());
            }
            "git" => git(args)?,
            _ => {}
        }
    }
    Ok(())
}

fn history(what: &str) -> String {
    format!(
        "git history: {} prints content the index does not hold",
        shown(what)
    )
}

/// A short option cluster (`-sv`) that holds `c`.
fn short_has(word: &str, chars: &[char]) -> bool {
    word.starts_with('-') && !word.starts_with("--") && word[1..].contains(chars)
}

fn git(args: &[String]) -> Result<(), String> {
    if args.iter().any(|arg| {
        let lower = arg.to_ascii_lowercase();
        arg.contains("%G") || lower.contains("signature")
    }) {
        return Err("a git signature atom: it runs gpg".into());
    }
    let mut at = 0;
    while matches!(
        args.get(at).map(String::as_str),
        Some("--no-pager" | "--no-optional-locks")
    ) {
        at += 1;
    }
    let Some(sub) = args.get(at) else {
        return Ok(());
    };
    if sub.starts_with('-') {
        // The table refuses every other global option; this layer does too, on its own.
        return Err(format!("git global option {}", shown(sub)));
    }
    let rest = &args[at + 1..];
    // An operand that could be a revision: only `./…` and `/…` cannot.
    let revision = |sub: &str| match rest
        .iter()
        .find(|w| !w.starts_with('-') && !w.starts_with("./") && !w.starts_with('/'))
    {
        Some(word) => Err(history(&format!("git {sub} with a revision ({word})"))),
        None => Ok(()),
    };
    match sub.as_str() {
        "show" | "cat-file" | "grep" => Err(history(&format!("git {sub}"))),
        "status" => match rest
            .iter()
            .find(|w| *w == "--verbose" || short_has(w, &['v']))
        {
            Some(word) => Err(history(&format!("git status {word}"))),
            None => Ok(()),
        },
        "diff" => {
            if let Some(word) = rest.iter().find(|w| *w == "--cached" || *w == "--staged") {
                return Err(history(&format!("git diff {word}")));
            }
            revision("diff")
        }
        // `blame` prints the tracked file as it is; at a revision it prints history.
        "blame" => revision("blame"),
        "log" => match rest.iter().find(|w| {
            short_has(w, LOG_PATCH_SHORT)
                || LOG_PATCH_LONG
                    .iter()
                    .any(|long| *w == long || w.starts_with(&format!("{long}=")))
        }) {
            Some(word) => Err(history(&format!("git log {word}"))),
            None => Ok(()),
        },
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        line.split(' ').map(str::to_string).collect()
    }

    #[test]
    fn git_forms() {
        for refused in [
            "status -v",
            "status -sv",
            "--no-pager status --verbose",
            "diff --cached",
            "diff HEAD",
            "diff main ./x",
            "log -p",
            "log -pu",
            "log --stat=80",
            "log --word-diff=color",
            "show",
            "branch --sort=-signature",
            "branch --format=%(signature:grade)",
            "log --format=%GK",
            "log -S TOPSECRET",
            "log -L1,2:x",
            "blame HEAD -- ./a.txt",
            "-C /x status",
        ] {
            assert!(git(&words(refused)).is_err(), "{refused}");
        }
        for allowed in [
            "status",
            "status -s",
            "diff",
            "diff ./src",
            "diff --stat",
            "log --oneline",
            "log -n 3",
            "log --no-patch",
            "blame ./a.txt",
            "branch -a",
        ] {
            assert!(git(&words(allowed)).is_ok(), "{allowed}");
        }
    }
}
