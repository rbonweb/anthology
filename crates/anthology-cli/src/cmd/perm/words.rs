//! Word-level refusals on top of the classifier's table, read from the same parse the classifier
//! cross-checks (`anthology_shell::analyze`): every stage's program and arguments, quotes removed.
//!
//! - `grep`, `egrep` and `fgrep` with `GREP_OPTIONS` set (BSD grep honors it), and `rg` with
//!   `RIPGREP_CONFIG_PATH` set (it can add any option): no decision.
//! - git, any word holding a signature atom (`%G…`, `%(signature…)`, `--sort=signature`): it runs
//!   gpg or ssh-keygen.
//! - git: each subcommand reads only the options of its allowlist ([`Opts`]), and any other option
//!   gives no decision. The lists leave out every form that prints history the index scan cannot
//!   see (a file committed, then removed): `log` takes no patch, diff, stat or check option at all,
//!   `status` no `-v`, `diff` no `--cached` and no operand that could be a revision (only `./…` and
//!   `/…` cannot), `blame` the same. `show`, `cat-file`, `grep` and every subcommand not listed
//!   give no decision.

use anthology_core::Env;
use anthology_shell::Entry;

use super::shown;

/// What a git subcommand may take; anything else gives no decision.
struct Opts {
    /// Short flags; they may be clustered (`-sb`).
    short: &'static str,
    /// Short options with a value: the rest of the cluster, or the next word.
    short_value: &'static str,
    /// Short options whose value, possibly empty, is the rest of the cluster (`-uno`).
    short_glued: &'static str,
    /// Short options followed by digits in the same word (`-U3`).
    short_digits: &'static str,
    /// `-N`, a count (`git log -5`).
    number: bool,
    /// Long flags, without a value.
    long: &'static [&'static str],
    /// Long options written `--name=value`.
    long_eq: &'static [&'static str],
    /// Long options written `--name=value` or `--name value`.
    long_value: &'static [&'static str],
    operands: Operands,
}

#[derive(Clone, Copy, PartialEq)]
enum Operands {
    /// Any operand: the classifier holds each as a path that must exist.
    Any,
    /// Only `./…` and `/…`, which git cannot read as a revision.
    Local,
    None,
}

const NONE: Opts = Opts {
    short: "",
    short_value: "",
    short_glued: "",
    short_digits: "",
    number: false,
    long: &[],
    long_eq: &[],
    long_value: &[],
    operands: Operands::None,
};

/// `log`: the review's allowlist, and nothing that prints a patch, a stat or a check.
const LOG: Opts = Opts {
    short_value: "n",
    number: true,
    long: &[
        "--oneline",
        "--graph",
        "--decorate",
        "--pretty",
        "--all",
        "--branches",
        "--tags",
        "--first-parent",
        "--reverse",
    ],
    long_eq: &[
        "--max-count",
        "--skip",
        "--format",
        "--pretty",
        "--decorate",
        "--date",
        "--since",
        "--until",
        "--after",
        "--before",
        "--author",
        "--committer",
        "--grep",
    ],
    operands: Operands::Any,
    ..NONE
};

const STATUS: Opts = Opts {
    short: "sbz",
    short_glued: "u",
    long: &[
        "--short",
        "--branch",
        "--long",
        "--porcelain",
        "--untracked-files",
        "--ignored",
        "--no-renames",
        "--renames",
        "--ahead-behind",
        "--no-ahead-behind",
        "--show-stash",
        "--column",
        "--no-column",
    ],
    long_eq: &[
        "--porcelain",
        "--untracked-files",
        "--ignored",
        "--column",
        "--find-renames",
    ],
    operands: Operands::Any,
    ..NONE
};

/// `diff` of the work tree against the index: tracked files, which the index scan holds.
const DIFF: Opts = Opts {
    short: "puwbRaqz",
    short_digits: "U",
    long: &[
        "--stat",
        "--shortstat",
        "--numstat",
        "--name-only",
        "--name-status",
        "--exit-code",
        "--quiet",
        "--check",
        "--minimal",
        "--patience",
        "--histogram",
        "--color",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--ignore-space-at-eol",
        "--ignore-space-change",
        "--ignore-all-space",
        "--ignore-cr-at-eol",
        "--ignore-blank-lines",
        "--summary",
        "--full-index",
        "--binary",
        "--no-renames",
        "--text",
        "--raw",
        "--patch",
        "--no-patch",
        "--function-context",
        "--word-diff",
        "--color-words",
    ],
    long_eq: &[
        "--stat",
        "--color",
        "--word-diff",
        "--color-words",
        "--diff-filter",
        "--unified",
        "--abbrev",
        "--find-renames",
    ],
    operands: Operands::Local,
    ..NONE
};

const BLAME: Opts = Opts {
    short: "wefnpstlb",
    short_value: "L",
    long: &[
        "--porcelain",
        "--line-porcelain",
        "--incremental",
        "--show-email",
        "--show-name",
        "--show-number",
        "--root",
        "--minimal",
    ],
    long_eq: &["--abbrev", "--date"],
    operands: Operands::Local,
    ..NONE
};

/// `ls-files` prints names, modes and object ids, never content.
const LS_FILES: Opts = Opts {
    short: "cdmoiskzftvu",
    short_value: "x",
    long: &[
        "--cached",
        "--deleted",
        "--modified",
        "--others",
        "--ignored",
        "--stage",
        "--unmerged",
        "--killed",
        "--directory",
        "--no-empty-directory",
        "--eol",
        "--full-name",
        "--error-unmatch",
        "--exclude-standard",
        "--deduplicate",
        "--debug",
    ],
    long_eq: &["--abbrev"],
    long_value: &["--exclude"],
    operands: Operands::Any,
    ..NONE
};

const REV_PARSE: Opts = Opts {
    short: "q",
    long: &[
        "--show-toplevel",
        "--show-prefix",
        "--show-cdup",
        "--is-inside-work-tree",
        "--is-inside-git-dir",
        "--is-bare-repository",
        "--is-shallow-repository",
        "--git-dir",
        "--absolute-git-dir",
        "--git-common-dir",
        "--verify",
        "--quiet",
        "--symbolic",
        "--symbolic-full-name",
        "--short",
        "--abbrev-ref",
        "--all",
        "--branches",
        "--tags",
        "--remotes",
    ],
    long_eq: &["--short", "--abbrev-ref"],
    operands: Operands::Any,
    ..NONE
};

const DESCRIBE: Opts = Opts {
    long: &[
        "--tags",
        "--all",
        "--long",
        "--always",
        "--exact-match",
        "--first-parent",
        "--contains",
    ],
    long_eq: &["--abbrev", "--candidates"],
    long_value: &["--match", "--exclude"],
    operands: Operands::Any,
    ..NONE
};

const BRANCH: Opts = Opts {
    short: "arv",
    long: &[
        "--all",
        "--remotes",
        "--verbose",
        "--list",
        "--show-current",
        "--color",
        "--no-color",
        "--column",
        "--no-column",
        "--omit-empty",
        "--no-abbrev",
        "--ignore-case",
    ],
    long_eq: &["--color", "--column", "--abbrev"],
    long_value: &[
        "--sort",
        "--contains",
        "--no-contains",
        "--merged",
        "--no-merged",
        "--points-at",
        "--format",
    ],
    ..NONE
};

const REMOTE: Opts = Opts {
    short: "v",
    long: &["--verbose"],
    ..NONE
};

pub fn check(entries: &[Entry], env: &Env) -> Result<(), String> {
    for entry in entries {
        // `prog` is the program word's name, lowercased; the table takes only bare names.
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
        return Err("git without a subcommand".into());
    };
    let opts = match sub.as_str() {
        "log" => &LOG,
        "status" => &STATUS,
        "diff" => &DIFF,
        "blame" => &BLAME,
        "ls-files" => &LS_FILES,
        "rev-parse" => &REV_PARSE,
        "describe" => &DESCRIBE,
        "branch" => &BRANCH,
        "remote" => &REMOTE,
        "show" | "cat-file" | "grep" => {
            return Err(format!(
                "git history: git {sub} prints content the index does not hold"
            ));
        }
        other if other.starts_with('-') => {
            return Err(format!("git global option {}", shown(other)));
        }
        other => return Err(format!("git subcommand not read: {}", shown(other))),
    };
    read(opts, sub, &args[at + 1..])
}

/// Reads the words after a git subcommand against its allowlist.
fn read(opts: &Opts, sub: &str, words: &[String]) -> Result<(), String> {
    let refuse = |word: &str| {
        Err(format!(
            "git history: an option of git {sub} this check does not read: {}",
            shown(word)
        ))
    };
    let mut i = 0;
    let mut dashes = false;
    while let Some(word) = words.get(i) {
        i += 1;
        if dashes || !word.starts_with('-') {
            match opts.operands {
                Operands::Any => {}
                Operands::Local if word.starts_with("./") || word.starts_with('/') => {}
                Operands::Local => {
                    return Err(format!(
                        "git history: git {sub} with an operand that could be a revision ({})",
                        shown(word)
                    ));
                }
                Operands::None => {
                    return Err(format!("git {sub} with an operand: {}", shown(word)));
                }
            }
            continue;
        }
        if word == "--" {
            dashes = true;
            continue;
        }
        if word == "-" {
            return refuse(word);
        }
        if word.starts_with("--") {
            let known = match word.split_once('=') {
                Some((name, _)) => opts.long_eq.contains(&name) || opts.long_value.contains(&name),
                None if opts.long.contains(&word.as_str()) => true,
                None if opts.long_value.contains(&word.as_str()) => {
                    // The value is the next word.
                    i += 1;
                    i <= words.len()
                }
                None => false,
            };
            if !known {
                return refuse(word);
            }
            continue;
        }
        let body = &word[1..];
        if opts.number && body.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        for (at, c) in body.char_indices() {
            let tail = &body[at + c.len_utf8()..];
            if opts.short.contains(c) {
                continue;
            }
            if opts.short_value.contains(c) {
                if tail.is_empty() {
                    if i >= words.len() {
                        return refuse(word);
                    }
                    i += 1;
                }
                break;
            }
            if opts.short_glued.contains(c) {
                break;
            }
            if opts.short_digits.contains(c)
                && !tail.is_empty()
                && tail.bytes().all(|b| b.is_ascii_digit())
            {
                break;
            }
            return refuse(word);
        }
    }
    Ok(())
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
            // Round 3: `log` reads only its allowlist.
            "log -U3",
            "log -U0",
            "log -U1",
            "log --check",
            "log --no-patch",
            "log --no-merges",
            "log --unknown-option",
            "log --author",
            "log -n",
            "status --verbose=1",
            "status -x",
            "diff -U",
            "diff -Ux",
            "diff --no-index",
            "blame -C ./a.txt",
            "branch --delete",
            "branch x",
            "remote add",
            "ls-files --with-tree=HEAD",
            "rev-parse --local-env-vars",
            "describe --dirty",
            "stash",
            "log -",
        ] {
            assert!(git(&words(refused)).is_err(), "{refused}");
        }
        for allowed in [
            "status",
            "status -s",
            "status -sb --porcelain=v2 -uno",
            "diff",
            "diff ./src",
            "diff --stat",
            "diff -U3 --check -- ./src",
            "log --oneline",
            "log -n 3",
            "log --oneline -n 5",
            "log -n5 -3 --skip=2 --max-count=4",
            "log --format=%h%x20%s --graph --decorate --all --date=iso",
            "log --since=1.week --author=x --grep=fix --reverse -- src",
            "blame ./a.txt",
            "blame -L 1,5 -- ./a.txt",
            "branch -a",
            "branch -vv --sort=-committerdate --format=%(refname)",
            "remote -v",
            "ls-files -s --exclude x",
            "rev-parse --show-toplevel HEAD",
            "describe --tags --match v*",
        ] {
            assert!(git(&words(allowed)).is_ok(), "{allowed}");
        }
    }
}
