//! P3: what `anthology_shell::classify_readonly` proves, the guard allows.
//!
//! The permission mod calls the classifier only when the engine's own verdict is `ask`, after the guard's
//! PreToolUse hook has let the command through, so a guard block always stands. This file checks the other
//! side of that: a command made of the plain read-only shapes is never one the guard blocks, so the
//! classifier never proves something the guard refuses for a rule (G1 to G6) or as too complex (G0). A path
//! the guard protects (a browser profile, the memory folder) is a different matter: the classifier is
//! structural and never reads the file system, so those commands stay for the path check and the guard.

#[path = "support/gd.rs"]
mod gd;

use anthology_guard::{Verdict, check};
use anthology_shell::{ReadOnly, classify_readonly};
use proptest::prelude::*;

use gd::{HOME, bash};

fn guard(command: &str) -> Verdict {
    let (tool, input) = bash(command);
    check(&tool, &input, HOME)
}

fn proven(command: &str) -> bool {
    matches!(classify_readonly(command, HOME), ReadOnly::Proven { .. })
}

#[test]
fn the_plain_read_only_commands_are_proven_and_allowed() {
    let commands = [
        "ls",
        "ls -la src tests",
        "cat README.md",
        "head -n 20 src/main.rs",
        "tail -n 5 log.txt",
        "wc -l src/main.rs",
        "grep -rn 'foo.*bar' src",
        "rg -n TODO src",
        "find . -name '*.rs' -type f",
        "stat -c %s README.md",
        "file --mime-type logo.png",
        "du -sh .",
        "pwd",
        "which cargo",
        "basename /a/b.txt .txt",
        "dirname src/lib.rs",
        "realpath .",
        "tree -L 2 src",
        "sort a.txt | uniq -c",
        "cut -d, -f1 a.csv",
        "cat a | tr a-z A-Z",
        "diff -u a.txt b.txt",
        "jq -r '.[] | .id' a.json",
        "git status",
        "git log --oneline -5",
        "git diff --stat",
        "git show --stat HEAD",
        "git blame -L 1,10 src/lib.rs",
        "git ls-files src",
        "git rev-parse --show-toplevel",
        "git describe --tags",
        "git branch -a",
        "git remote -v",
        "cd /work/proj && ls",
        "cd ./sub && git status | head -n 5",
        "ls 2>&1 | head -n 5",
        "cat < a.txt",
    ];
    for command in commands {
        assert!(proven(command), "{command}");
        assert_eq!(guard(command), Verdict::Allow, "{command}");
    }
}

/// The corpus of commands the guard blocks is never proven read-only.
#[test]
fn a_command_the_guard_blocks_is_not_proven() {
    for command in [
        "npx playwright install",
        "npm install puppeteer",
        "git -c alias.cl=clone cl -u 'npx playwright install' . ../c",
        "`echo agent-browser` open x",
        "$(echo open) -a \"Google Chrome\"",
        "$(((true)); echo agent-browser) open x",
        "echo $(( $(`echo agent-browser` open x) ))",
        "ls $(( $(`echo agent-browser` open x) ))",
        "cat <(npx playwright install)",
    ] {
        assert_ne!(guard(command), Verdict::Allow, "{command}");
        assert!(!proven(command), "{command}");
    }
}

fn program() -> impl Strategy<Value = &'static str> {
    prop_oneof![
        Just("ls"),
        Just("ls -la"),
        Just("cat"),
        Just("head -n 5"),
        Just("tail -n 20"),
        Just("wc -l"),
        Just("grep -rn foo"),
        Just("grep -c 'a b'"),
        Just("rg -n TODO"),
        Just("find . -name '*.rs'"),
        Just("find src -type f -maxdepth 2"),
        Just("stat -c %s"),
        Just("file --mime-type"),
        Just("du -sh"),
        Just("pwd"),
        Just("which cargo"),
        Just("basename"),
        Just("dirname"),
        Just("realpath"),
        Just("tree -L 2"),
        Just("sort -u"),
        Just("uniq -c"),
        Just("cut -d, -f1"),
        Just("tr a-z A-Z"),
        Just("diff -u"),
        Just("jq -r '.name'"),
        Just("git status -sb"),
        Just("git log --oneline -5"),
        Just("git diff --stat"),
        Just("git show --stat"),
        Just("git blame -w"),
        Just("git ls-files"),
        Just("git rev-parse --show-toplevel"),
        Just("git describe --tags"),
        Just("git branch -vv"),
        Just("git remote -v"),
    ]
}

/// Mostly plain operands, so that a fair share of the generated commands is proven; the rest are the
/// shapes the classifier must refuse.
fn operand() -> impl Strategy<Value = &'static str> {
    prop_oneof![
        20 => prop_oneof![
            Just(""),
            Just("src"),
            Just("a.txt"),
            Just("./sub/b.rs"),
            Just("/work/proj/c.md"),
            Just("'x y'"),
            Just("--"),
            Just("src/*.rs"),
            Just("."),
            Just("main"),
            Just("2>&1"),
            Just(">/dev/null"),
            Just("< in.txt"),
        ],
        5 => prop_oneof![
            Just("-n"),
            Just("-la"),
            Just("*.rs"),
            Just(".*"),
            Just(".."),
            Just("HEAD:f"),
            Just("> out.txt"),
            Just("$X"),
            Just("$(ls)"),
            Just("`ls`"),
        ],
    ]
}

fn join() -> impl Strategy<Value = &'static str> {
    prop_oneof![
        10 => Just(" | "),
        6 => Just(" && "),
        1 => Just("; "),
        1 => Just(" || "),
        1 => Just(" & "),
    ]
}

fn prefix() -> impl Strategy<Value = &'static str> {
    prop_oneof![
        6 => Just(""),
        4 => Just("cd /work/proj && "),
        4 => Just("cd ./sub && "),
        1 => Just("cd .. && "),
        1 => Just("cd src && "),
        1 => Just("FOO=bar "),
    ]
}

/// A command of one to three stages of the vocabulary, joined and maybe led by a `cd`.
fn command() -> impl Strategy<Value = String> {
    (
        prefix(),
        proptest::collection::vec((program(), operand(), operand()), 1..4),
        proptest::collection::vec(join(), 3),
    )
        .prop_map(|(head, stages, joins)| {
            let mut command = String::from(head);
            for (index, (program, one, two)) in stages.iter().enumerate() {
                if index > 0 {
                    command.push_str(joins[index - 1]);
                }
                command.push_str(&format!("{program} {one} {two}"));
            }
            command
        })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 3000, ..ProptestConfig::default() })]

    /// Whatever the classifier proves, from stages of the plain vocabulary with every kind of operand and
    /// joiner thrown in, the guard allows.
    #[test]
    fn proven_implies_the_guard_allows(command in command()) {
        if proven(&command) {
            prop_assert_eq!(guard(&command), Verdict::Allow, "{}", command);
        }
    }
}

/// The property above is not vacuous: of the generated commands a real share is proven, and a real share
/// is not.
#[test]
fn the_generator_reaches_both_sides() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;

    let mut runner = TestRunner::deterministic();
    let strategy = command();
    let (mut yes, mut no) = (0, 0);
    for _ in 0..2000 {
        let command = strategy.new_tree(&mut runner).expect("a value").current();
        if proven(&command) {
            yes += 1;
        } else {
            no += 1;
        }
    }
    assert!(yes >= 150, "proven: {yes} of 2000");
    assert!(no >= 150, "not proven: {no} of 2000");
}
