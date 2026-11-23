//! Anthology fork (RG1): atl's clap subcommand list, pinned against the shared shell
//! parser (`anthology_shell`).
//!
//! The guard and atl's Anthology gate decide from `anthology_shell::analyze`, which
//! reads `atl <sub> ...` by `ATL_PASSTHROUGH` (the rest is a command) and
//! `ATL_RUNNERS` (a command or a script), and every other subcommand as wrapping a
//! fixed tool (`atl git push` runs `git push`). An upstream sync that adds a
//! subcommand fails here until someone decides which of the three it is: a new
//! runner the parser does not know would let `atl <new> git push` hide what runs.
//!
//! Not named `fork_*`: the `FORK_TESTS` list is untouched.

mod common;

/// How `anthology_shell` reads `atl <sub> ...`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Form {
    /// The rest of the line is a command (`ATL_PASSTHROUGH`).
    Passthrough,
    /// A command or a `-c`/`--shell` script (`ATL_RUNNERS`).
    Runner,
    /// The subcommand names (or wraps) a fixed tool; its words stand as they are.
    Fixed,
}

use Form::{Fixed, Passthrough, Runner};

/// Every subcommand `atl --help` lists, plus the hidden alias `proxy`, as of
/// 1.2.1-anthology.6. Keep it sorted.
const PINNED: &[(&str, Form)] = &[
    ("ast-grep", Fixed),
    ("aws", Fixed),
    ("bun", Fixed),
    ("bunx", Fixed),
    ("bypass", Passthrough),
    ("cargo", Fixed),
    ("cc-economics", Fixed),
    ("config", Fixed),
    ("ctest", Fixed),
    ("curl", Fixed),
    ("deno", Fixed),
    ("deps", Fixed),
    ("diff", Fixed),
    ("discover", Fixed),
    ("docker", Fixed),
    ("dotnet", Fixed),
    ("ecs", Fixed),
    ("env", Fixed),
    ("err", Runner),
    ("find", Fixed),
    ("format", Fixed),
    ("gain", Fixed),
    ("gh", Fixed),
    ("git", Fixed),
    ("glab", Fixed),
    ("go", Fixed),
    ("golangci-lint", Fixed),
    ("gradlew", Fixed),
    ("grep", Fixed),
    ("gt", Fixed),
    ("help", Fixed),
    ("hook", Fixed),
    ("hook-audit", Fixed),
    ("init", Fixed),
    ("jest", Fixed),
    ("json", Fixed),
    ("kubectl", Fixed),
    ("learn", Fixed),
    ("lint", Fixed),
    ("llm-proxy", Fixed),
    ("log", Fixed),
    ("ls", Fixed),
    ("mvn", Fixed),
    ("mvnd", Fixed),
    ("mypy", Fixed),
    ("next", Fixed),
    ("ng", Fixed),
    ("npm", Fixed),
    ("npx", Fixed),
    ("oc", Fixed),
    ("opencode", Fixed),
    ("paratest", Fixed),
    ("pest", Fixed),
    ("php", Fixed),
    ("phpstan", Fixed),
    ("phpt", Fixed),
    ("phpunit", Fixed),
    ("pint", Fixed),
    ("pip", Fixed),
    ("pipe", Fixed),
    ("playwright", Fixed),
    ("pnpm", Fixed),
    ("prettier", Fixed),
    ("prisma", Fixed),
    ("proxy", Passthrough),
    ("psql", Fixed),
    ("pytest", Fixed),
    ("rake", Fixed),
    ("read", Fixed),
    ("recall", Fixed),
    ("rewrite", Fixed),
    ("rg", Fixed),
    ("rspec", Fixed),
    ("rubocop", Fixed),
    ("ruff", Fixed),
    ("run", Runner),
    ("sbt", Fixed),
    ("session", Fixed),
    ("smart", Fixed),
    ("sqlfluff", Fixed),
    ("summary", Runner),
    ("test", Runner),
    ("tree", Fixed),
    ("trust", Fixed),
    ("tsc", Fixed),
    ("untrust", Fixed),
    ("uv", Fixed),
    ("verify", Fixed),
    ("vitest", Fixed),
    ("wc", Fixed),
    ("wget", Fixed),
];

fn atl(args: &[&str]) -> (bool, String) {
    let out = common::atl_command().args(args).output().expect("run atl");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

/// The names in the `Commands:` section of `atl --help`.
fn listed_subcommands() -> Vec<String> {
    let (ok, help) = atl(&["--help"]);
    assert!(ok, "atl --help failed");
    let mut names: Vec<String> = help
        .lines()
        .skip_while(|line| line.trim_end() != "Commands:")
        .skip(1)
        .take_while(|line| !line.trim().is_empty())
        .filter(|line| line.starts_with("  ") && !line.starts_with("    "))
        .filter_map(|line| line.split_whitespace().next().map(str::to_string))
        .collect();
    names.sort();
    names
}

#[test]
fn the_atl_subcommand_list_is_pinned() {
    let mut pinned: Vec<String> = PINNED
        .iter()
        .map(|(name, _)| name.to_string())
        .filter(|name| name != "proxy")
        .collect();
    pinned.sort();
    let listed = listed_subcommands();
    let added: Vec<&String> = listed.iter().filter(|n| !pinned.contains(n)).collect();
    let removed: Vec<&String> = pinned.iter().filter(|n| !listed.contains(n)).collect();
    assert!(
        added.is_empty() && removed.is_empty(),
        "atl's subcommands changed (added {added:?}, removed {removed:?}). For each new one, \
         decide whether it runs a command or a script: if so, add it to \
         anthology_shell::ATL_PASSTHROUGH or ATL_RUNNERS (and to the gate's atl_inner), then \
         pin it in PINNED with its form."
    );
}

#[test]
fn the_pinned_list_is_sorted_and_unique() {
    let names: Vec<&str> = PINNED.iter().map(|(name, _)| *name).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(names, sorted);
}

#[test]
fn proxy_is_still_a_hidden_alias_of_bypass() {
    let (ok, help) = atl(&["proxy", "--help"]);
    assert!(ok, "atl proxy --help failed");
    assert!(help.contains("Usage: atl bypass"), "{help}");
}

#[test]
fn the_shared_parser_reads_runners_and_passthroughs_as_pinned() {
    for (name, form) in PINNED {
        assert_eq!(
            anthology_shell::ATL_PASSTHROUGH.contains(name),
            *form == Passthrough,
            "{name}: ATL_PASSTHROUGH disagrees with PINNED"
        );
        assert_eq!(
            anthology_shell::ATL_RUNNERS.contains(name),
            *form == Runner,
            "{name}: ATL_RUNNERS disagrees with PINNED"
        );
    }
    for name in anthology_shell::ATL_PASSTHROUGH
        .iter()
        .chain(anthology_shell::ATL_RUNNERS)
    {
        assert!(
            PINNED.iter().any(|(pinned, _)| pinned == name),
            "{name} is not an atl subcommand"
        );
    }
}

/// The program `analyze` finds for a one-command line.
fn programs(line: &str) -> Vec<(String, Vec<String>)> {
    anthology_shell::analyze(line, "/home/u")
        .entries
        .into_iter()
        .map(|entry| (entry.prog, entry.args))
        .collect()
}

#[test]
fn the_shared_parser_finds_what_each_form_runs() {
    let git_push = || ("git".to_string(), vec!["push".to_string()]);
    for (name, form) in PINNED {
        let line = format!("atl {name} git push");
        let found = programs(&line);
        match form {
            // The rest is the command.
            Passthrough | Runner => assert_eq!(found, vec![git_push()], "{line}"),
            // A fixed tool: read exactly as the tool itself (`atl npx x` as `npx x`,
            // which the parser peels as a package runner).
            Fixed => assert_eq!(found, programs(&format!("{name} git push")), "{line}"),
        }
    }
    // A word that is no subcommand falls through to the program it names, which
    // is also how a hidden or future subcommand is read.
    assert_eq!(
        programs("atl echo hi"),
        vec![("echo".to_string(), vec!["hi".to_string()])]
    );
    // Scripts: `run -c`, and `--shell` for the other runners.
    for line in [
        "atl run -c 'git push'",
        "atl run --command 'git push'",
        "atl err --shell bash git push",
        "atl test --shell sh 'git push'",
        "atl summary --shell bash git push",
    ] {
        assert!(programs(line).contains(&git_push()), "{line}");
    }
}
