//! DT-4: a program word hidden in a substitution (SH-FW final F-8) inside a `$((...))` is too complex,
//! as it is outside one. The arithmetic text itself runs no program word, but bash runs the commands of
//! a `$(...)` or a backtick in it, program words included. Before the fix the lexer dropped the F-8
//! holders of every segment lexed from a `$((...))`, the nested substitutions' commands included, so
//! these commands were allowed. The shipped Python guard allows them too (it has no F-8).

#[path = "support/gd.rs"]
mod gd;

use anthology_guard::{Verdict, check};

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
fn a_hidden_program_word_inside_an_arithmetic_expansion_is_too_complex() {
    assert_all(
        &[
            "echo $(( $(`echo agent-browser` open x) ))",
            r#"echo $(( $($(echo open) -a "Google Chrome") ))"#,
            "echo $(( 1 + $(`echo agent-browser` open x) ))",
            "x=$(( $(`echo agent-browser` open x) ))",
            "echo $(( $(true; `echo agent-browser` open x) ))",
            r#"echo $(( $(`echo op``echo en` -a "Google Chrome") ))"#,
        ],
        Verdict::TooComplex,
    );
}

#[test]
fn the_same_commands_outside_an_arithmetic_expansion_are_too_complex() {
    assert_all(
        &[
            "`echo agent-browser` open x",
            r#"$(echo open) -a "Google Chrome""#,
            "echo $( $(`echo agent-browser` open x) )",
            r#"echo $( $($(echo open) -a "Google Chrome") )"#,
            r#"echo "$(( $(`echo agent-browser` open x) ))""#,
            "(( $(`echo agent-browser` open x) ))",
        ],
        Verdict::TooComplex,
    );
}

#[test]
fn an_arithmetic_expansion_still_runs_no_program_word_of_its_own() {
    assert_all(
        &[
            "echo $(( $(date +%s) - 1 ))",
            "echo $(( `wc -l < notes.md` + 1 ))",
            "echo $(( $(which agent-browser) ))",
            "n=$(( $(ls | wc -l) * 2 ))",
        ],
        Verdict::Allow,
    );
}
