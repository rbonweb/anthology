//! G0-FP: everyday commands a replay of real traffic found blocked as "too complex" (G0) while the shipped
//! Python guard allows them, and the neighbours of each shape that must still block.
//! 1. A bracket and a backtick in one word with no closed subscript (`echo '[`'`, `grep '\[`u`\]'`): bash
//!    runs a subscript's substitution only when the substitution and the subscript are closed, and no name
//!    ends in a backslash.
//! 2. A shell script given arguments, and `2>&1` (`sh run.sh 1 2>&1`): `>&1` writes no file, and the
//!    arguments after a script file are its positional parameters, not files it runs.
//! 3. The same with a file the text writes as an argument (`... > work/p.hex && sh cmp.sh work/p.hex`).
//! 4. Two pipelines that each end in `head` (`a | head; b | grep x | head`): a one-word command ended by
//!    no `(` defines no function, so the second `head` is not a function call fed by its pipe.

#[path = "support/gd.rs"]
mod gd;

use anthology_guard::{Rule, Verdict, check};

use gd::{HOME, bash};

fn verdict(command: &str) -> Verdict {
    let (tool, input) = bash(command);
    check(&tool, &input, HOME)
}

#[track_caller]
fn assert_all(commands: &[&str], want: Verdict) {
    let wrong: Vec<String> = commands
        .iter()
        .filter_map(|c| {
            let got = verdict(c);
            (got != want).then(|| format!("{c:?}: {got:?}"))
        })
        .collect();
    assert!(wrong.is_empty(), "want {want:?}:\n{}", wrong.join("\n"));
}

#[test]
fn a_bracket_and_a_backtick_without_a_closed_subscript_are_allowed() {
    assert_all(
        &[
            "echo '[`'",
            r#"grep "\[`u`\]" x"#,
            r"grep '\[`u`\]' x",
            r#"grep -n "\[\`unread\`\]" notes.md"#,
            "grep -n '[`]' notes.md",
            "echo 'a[`x'",
        ],
        Verdict::Allow,
    );
    // a closed subscript, as a word, an assignment value or an array element, stays too complex
    assert_all(
        &[
            "echo 'a[`npx playwright install`]'",
            "unset 'a[`npx playwright install`]'",
            "x='[$(npx playwright install)]'",
            "a=(['$(npx playwright install)']=1)",
            r"unset 'a[x\[$(npx playwright install)]]'",
        ],
        Verdict::TooComplex,
    );
}

#[test]
fn a_script_given_arguments_and_a_descriptor_duplicate_is_allowed() {
    assert_all(
        &[
            "sh run.sh 1 2>&1",
            "sh run.sh 1 >&2",
            "bash -e run.sh 2 3>&1-",
            "sh run.sh 1 2>&-",
            "cd /tmp && python3 gen.py 2 200 > work/p.hex && sh cmp.sh work/p.hex",
            "printf x > out.txt; bash -x run.sh out.txt",
            "printf x > out.txt; . ./env.sh out.txt",
        ],
        Verdict::Allow,
    );
    // a shell or `source` running the file the text wrote stays too complex, `>&file` included
    assert_all(
        &[
            "printf 'npx playwright install' > p.sh; sh p.sh 1 2>&1",
            "printf 'npx playwright install' > p.sh; sh -e p.sh",
            "printf 'npx playwright install' > p.sh; bash -o errexit p.sh",
            "printf 'npx playwright install' >&p.sh; sh p.sh",
            "printf 'npx playwright install' > p.sh; . p.sh 1",
            r#"printf 'npx playwright install' > p.sh; sh -c '. "$1"' _ p.sh"#,
        ],
        Verdict::TooComplex,
    );
}

#[test]
fn a_one_word_command_ended_by_no_paren_defines_no_function() {
    assert_all(
        &[
            r#"git ls-files | xargs grep -n "/U/\|rb" | head; git diff | grep "^+" | grep -n "/U/\|rb" | head"#,
            "ls | head; find . | xargs wc -l | head",
        ],
        Verdict::Allow,
    );
    // a function, `f()`, `f ()`, a subshell body or `function f`, fed a pipe that reaches a shell
    assert_all(
        &[
            "f() { sh; }; echo npx playwright install | f",
            "f () { sh; }; echo npx playwright install | f",
            "f() ( sh ); echo npx playwright install | f",
            "function f { sh; }; echo npx playwright install | f",
        ],
        Verdict::TooComplex,
    );
    assert_eq!(
        verdict(r#"f() { npx "$@"; }; f playwright install"#),
        Verdict::Block(Rule::G1)
    );
}
