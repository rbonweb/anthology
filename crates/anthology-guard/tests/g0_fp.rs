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
//! 5. A regex bracket expression holding a backtick, written in double quotes with `\"` and `\``
//!    (`grep -E "['\"\`]A['\"\`]" d`): a `[` at the start of a word or after punctuation follows no name, so
//!    it opens no subscript. bash 5.2 and zsh 5.9 agree on that and on `\`` being a literal backtick in
//!    double quotes; a subscript is evaluated after quote removal, so `unset "a[\`id\`]"` still runs `id`.

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

#[track_caller]
fn assert_none_allowed(commands: &[&str]) {
    let wrong: Vec<String> = commands
        .iter()
        .filter(|c| verdict(c) == Verdict::Allow)
        .map(|c| format!("{c:?}"))
        .collect();
    assert!(wrong.is_empty(), "want no Allow:\n{}", wrong.join("\n"));
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

#[test]
fn a_regex_bracket_with_escaped_quotes_and_backticks_is_allowed() {
    assert_all(
        &[
            // the field command: `\"`, `\``, `{1,}`, non-ASCII text, and a pipe into `grep -v` and `head`
            r#"grep -rnE "['\"\`][A-Z][a-z]+( [a-zA-Z’']+){1,}[.!?…]?['\"\`]" dir1 dir2 --include='*.ts' | grep -v "a/\|b\|c('" | head -40"#,
            r#"grep -rnE "['\"\`][A-Z][a-z]+( [a-zA-Z’']+){1,}[.!?…]?['\"\`]" dir1 dir2 --include='*.ts'"#,
            // the bracket shape that was too complex, with each other feature taken away
            r#"grep -E "['\"\`]A['\"\`]" d"#,
            r#"grep -E "[\`]A[\`]" d"#,
            r"grep -E '[`]A[`]' d",
            r#"grep -E "[A-Z]+ [\`]A?[\`]" d"#,
            // each feature alone
            r#"grep "a\`" f"#,
            r#"grep "\`a\`" f"#,
            r#"grep "a\"b" f"#,
            r#"grep "'\"\`" f"#,
            r#"grep -E "(a){1,}" f"#,
            r#"grep -E "[a-zA-Z’']+[.!?…]?" f"#,
            r#"grep "a’b…" f"#,
            r#"grep a f | grep -v "a/\|b\|c('" | head -40"#,
            // an escaped backtick is a literal one, not a substitution
            r#"grep "a\`npx playwright install\`" f"#,
            r#"echo "\`npx playwright install\`""#,
            // an unescaped backtick or `$(` in double quotes keeps its reading (`id` runs nothing risky)
            r#"grep "a`id`" f"#,
            r#"echo "$(id)""#,
        ],
        Verdict::Allow,
    );
    // an unescaped substitution or an unterminated string keeps its verdict: the inner command is checked
    assert_all(
        &[
            r#"grep "a`npx playwright install`" f"#,
            r#"echo "$(npx playwright install)""#,
            r#"echo "\`npx playwright install"#,
            // a bracket with no name before it: the substitution in it is read as a command (`unset` etc.)
            r#"unset "[\`npx playwright install\`]""#,
            r"unset '[`npx playwright install`]'",
        ],
        Verdict::Block(Rule::G1),
    );
}

#[test]
fn a_subscript_after_a_name_stays_too_complex_whatever_quotes_the_backtick() {
    // bash and zsh evaluate `name[...]` after quote removal, so `\`` in double quotes runs just as `'`'` does
    assert_all(
        &[
            r#"unset "a[\`npx playwright install\`]""#,
            r"unset 'a[`npx playwright install`]'",
            r#"let "a[\`npx playwright install\`]=1""#,
            r#"[[ -v "a[\`npx playwright install\`]" ]]"#,
            r#"printf -v "a[\`npx playwright install\`]" x"#,
            r#"read "a[\`npx playwright install\`]" <<< x"#,
            r#"declare "a_1[\`npx playwright install\`]=1""#,
            r#"unset "é[\`npx playwright install\`]""#,
            r#"unset "a[0][\`npx playwright install\`]""#,
            r#"unset "a[[\`npx playwright install\`]]""#,
            r#"unset "a?[\`npx playwright install\`]""#,
            // a name an expansion gives
            r#"unset "${x}[\`npx playwright install\`]""#,
            r#"unset "$x[\`npx playwright install\`]""#,
            r#"unset "$-[\`npx playwright install\`]""#,
            r#"unset "$@[\`npx playwright install\`]""#,
            r#"unset "$[\`npx playwright install\`]""#,
            // a name a substitution gives: the lexer takes the substitution out of the word
            r#"unset "$(echo a)[\`npx playwright install\`]""#,
            r#"unset "`echo a`[\`npx playwright install\`]""#,
            // an array literal: its elements are subscripts, so any `[` counts
            r"a=( ['`npx playwright install`']=1 )",
            r#"a=( ["\`npx playwright install\`"]=1 )"#,
            r"a=(['$(npx playwright install)']=1)",
            r"x+=( ['`npx playwright install`']=1 )",
            r"declare -a x='([`npx playwright install`]=1)'",
            r"declare -a x=( ['`npx playwright install`']=1 )",
        ],
        Verdict::TooComplex,
    );
}

#[test]
fn an_expansion_elsewhere_in_the_command_may_put_a_name_before_a_bracket() {
    // The word with the bracket holds no name, but an expansion later in the command can put one in front of
    // it (`unset "a$1"` makes `a[...]`), so the "a bracket follows a name" reading is used only for a command
    // with no expansion, no backtick substitution and no consumer of text. Each of the first lines ran its
    // marker in bash 5.2 and zsh 5.9.
    assert_none_allowed(&[
        r#"set -- '[`npx playwright install`]'; unset "a$1""#,
        r#"set -- '[$(npx playwright install)]'; (( a$1 ))"#,
        r#"a=(1); set -- '[$(npx playwright install)]'; echo $((a$1))"#,
        r#"for s in '[$(npx playwright install)]'; do unset "a$s"; done"#,
        r#"s=.'[$(npx playwright install)]'; unset "a${s#.}""#,
        r#"unset "a${s:-[\`npx playwright install\`]}""#,
        r#"unset "a${s-[\`npx playwright install\`]}""#,
        r#"s=1; unset "a${s:+[\`npx playwright install\`]}""#,
        r#"s=1; unset "a${s/1/[\`npx playwright install\`]}""#,
        // a name given without `$`: xargs puts the line in its replacement string, a shell reads the
        // decoded text a printf wrote
        r#"echo a | xargs -I@ unset "@[\`npx playwright install\`]""#,
        r#"printf 'unset "a\0441"' | sh -s "[\`npx playwright install\`]""#,
        // an unescaped backtick substitution anywhere
        r#"echo `echo a` > /dev/null; unset "[\`npx playwright install\`]""#,
    ]);
    // a command with none of these keeps the new reading
    assert_all(
        &[
            r#"grep -E "['\"\`]A['\"\`]" d | head -3"#,
            r#"echo a b; grep "[\`]x[\`]" f"#,
        ],
        Verdict::Allow,
    );
}
