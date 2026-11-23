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
//! 6. SQL given to `psql -c` through `docker exec` (`docker exec db psql -c "select count(*) from t"`): the
//!    docker reading took every `-c` word for a shell option and parsed its SQL as a script. `psql -c` of
//!    plain SQL runs no shell, and `docker exec` runs its argv with none, so the quoted SQL is data. Only
//!    for the program word `psql` (exactly) right after `docker exec`'s listed options and the container,
//!    and only for a value with no backslash: `\!` and `\o |` make psql start a shell.

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
        // an alias holds text a later word runs, so the name and the bracket meet there
        r#"shopt -s expand_aliases; alias u='a[0]=1; unset "a"[\`"npx playwright install"\`]'; u"#,
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

#[test]
fn sql_given_to_psql_through_docker_exec_is_data_not_a_script() {
    assert_all(
        &[
            r#"docker exec db psql -c "select count(*) from t""#,
            r#"docker exec db psql -Atc "select count(*) from t""#,
            "docker exec db psql -c 'select count(*) from t'",
            r#"docker exec -i db psql -U u -d d -c "select count(*) from t""#,
            r#"docker exec db psql -c "select count(*) from t" -c "select max(id) from t""#,
            r#"docker exec db psql -c "select count(*) from t" -d x"#,
            // docker compose and docker-compose read the same way
            r#"docker compose exec db psql -c "select count(*) from t""#,
            r#"docker-compose exec db psql -Atc "select count(*) from t""#,
            // each value option of exec takes the next word, each boolean option takes none
            r#"docker exec -e A=b db psql -c "select count(*) from t""#,
            r#"docker exec --env A=b db psql -c "select count(*) from t""#,
            r#"docker exec --env-file f db psql -c "select count(*) from t""#,
            r#"docker exec -u postgres db psql -c "select count(*) from t""#,
            r#"docker exec --user postgres db psql -c "select count(*) from t""#,
            r#"docker exec -w /tmp db psql -c "select count(*) from t""#,
            r#"docker exec --workdir /tmp db psql -c "select count(*) from t""#,
            r#"docker exec --detach-keys ctrl-x db psql -c "select count(*) from t""#,
            r#"docker exec -i db psql -c "select count(*) from t""#,
            r#"docker exec -t db psql -c "select count(*) from t""#,
            r#"docker exec -d db psql -c "select count(*) from t""#,
            r#"docker exec -it db psql -c "select count(*) from t""#,
            r#"docker exec --privileged db psql -c "select count(*) from t""#,
            r#"docker exec --interactive db psql -c "select count(*) from t""#,
            r#"docker exec --tty db psql -c "select count(*) from t""#,
            r#"docker exec --detach db psql -c "select count(*) from t""#,
            r#"docker exec -it -u postgres -w /tmp -e A=b db psql -c "select count(*) from t""#,
            // the neighbours that were already allowed
            r#"psql -c "select count(*) from t""#,
            r#"docker exec db psql -c "select 1""#,
            r#"docker exec db mysql -e "select count(*) from t""#,
            r#"docker exec db echo "a(b)""#,
            r#"docker exec db echo "a*b""#,
            r#"docker run --rm img echo "count(*)""#,
            // docker exempts G1 and G2: running the install in the container is what G1 asks for
            r#"docker exec c sh -c "npx playwright install""#,
            r#"docker exec c bash -c "npx playwright install""#,
            "docker exec c npx playwright install",
        ],
        Verdict::Allow,
    );
}

/// The script a `-c` word hands over, when it is read as a shell script: G4 names the program in it.
const MARKER: &str = "agent-browser open x";

#[test]
fn a_psql_value_with_a_backslash_is_still_read_as_a_script() {
    // `\!` and `\o |` start a shell in psql: a value with any backslash keeps the shell reading (G4 here)
    for command in [
        format!(r"docker exec c psql -c '\! {MARKER}'"),
        format!(r"docker exec c psql -c '\o |{MARKER}'"),
        format!(r"docker exec c psql -Atc '\! {MARKER}'"),
        format!(r#"docker exec c psql -c "select 1" -c '\! {MARKER}'"#),
        format!(r"docker exec c psql -c '\! {MARKER}' dbname"),
        format!(r#"docker exec c psql -c '\! {MARKER}' -c "select count(*) from t""#),
        format!(r"docker compose exec c psql -c '\! {MARKER}'"),
        format!(r"docker-compose exec c psql -c '\! {MARKER}'"),
    ] {
        assert_eq!(verdict(&command), Verdict::Block(Rule::G4), "{command}");
    }
    assert_eq!(
        verdict(r"docker exec c psql -c '\! echo a(b)'"),
        Verdict::TooComplex
    );
    // the same SQL without a backslash is data
    assert_eq!(
        verdict(r#"docker exec c psql -c "select count(*) from t" -c "select 1""#),
        Verdict::Allow
    );
}

#[test]
fn only_a_psql_in_program_position_after_the_exec_options_is_data() {
    // a word named psql anywhere else (a runner's argument, a flock path, an option value, a run name) is
    // not the program: the `-c` after it is that program's, read as a shell script (G4 here)
    for command in [
        format!(r#"docker exec c su psql -c "{MARKER}""#),
        format!(r#"docker exec c su -l psql -c "{MARKER}""#),
        format!(r#"docker exec c runuser psql -c "{MARKER}""#),
        format!(r#"docker exec c script -q psql -c "{MARKER}""#),
        format!(r#"docker exec c flock /tmp/psql -c "{MARKER}""#),
        format!(r#"docker exec -u psql c python3 -c "{MARKER}""#),
        format!(r#"docker exec -w /srv/psql c python3 -c "{MARKER}""#),
        format!(r#"docker exec -e psql c python3 -c "{MARKER}""#),
        format!(r#"docker run --name psql img python3 -c "{MARKER}""#),
        format!(r#"docker run img psql -c "{MARKER}""#),
        format!(r#"docker compose run psql python3 -c "{MARKER}""#),
        // an option exec does not list: nothing says where the program is
        format!(r#"docker exec --unknown c psql -c "{MARKER}""#),
        format!(r#"docker exec -u=u c psql -c "{MARKER}""#),
        // the exact name: not another case, not a path
        format!(r#"docker exec c PSQL -c "{MARKER}""#),
        format!(r#"docker exec c /usr/bin/psql -c "{MARKER}""#),
        format!(r#"docker exec c env psql -c "{MARKER}""#),
    ] {
        assert_eq!(verdict(&command), Verdict::Block(Rule::G4), "{command}");
    }
    assert_all(
        &[
            r#"docker run img psql -c "select count(*) from t""#,
            r#"docker exec --unknown c psql -c "select count(*) from t""#,
            r#"docker exec -u=u c psql -c "select count(*) from t""#,
            r#"docker exec c PSQL -c "select count(*) from t""#,
            r#"docker exec c /usr/bin/psql -c "select count(*) from t""#,
            r#"docker exec c env psql -c "select count(*) from t""#,
            r#"docker exec c su -c "echo a(b)" u"#,
        ],
        Verdict::TooComplex,
    );
}

#[test]
fn a_subscript_tail_passed_through_a_function_argument_is_too_complex() {
    // A function defined and called in one command gets the bracket tail as `$1` and joins it after a name in
    // `unset`, `$((...))` or `((...))`. bash 5.2 and zsh 5.9 run the substitution there. The guard cannot see
    // the join from the word that holds the tail, so the command is too complex, not allowed.
    assert_all(
        &[
            r#"f() { unset "a$1"; }; f '[$(npx playwright install)]'"#,
            r#"f() { unset "a$1"; }; f '[`npx playwright install`]'"#,
            r#"f() { (( a$1 )); }; f '[$(npx playwright install)]'"#,
            r#"f() { echo $((a$1)); }; f '[$(npx playwright install)]'"#,
            r#"a=(1); f() { echo $((a$1)); }; f '[$(npx playwright install)]'"#,
            r#"function f { unset "a$1"; }; f '[$(npx playwright install)]'"#,
            r#"function f() { unset "a$1"; }; f '[$(npx playwright install)]'"#,
            // the call comes through a variable, a loop over the arguments or a local copy
            r#"s='[$(npx playwright install)]'; f() { unset "a$1"; }; f "$s""#,
            r#"f() { for s; do unset "a$s"; done; }; f '[$(npx playwright install)]'"#,
            r#"f() { local s=$1; (( a$s )); }; f '[$(npx playwright install)]'"#,
            r#"f() { s=$1; echo $((a$s)); }; f '[$(npx playwright install)]'"#,
            r#"f() { unset "a$@"; }; f '[$(npx playwright install)]'"#,
            r#"f() { unset "a${1}"; }; f '[$(npx playwright install)]'"#,
            // the call comes first in the text, and the call sits in a pipeline, a list or a subshell
            r#"f() { unset "a$1"; }; true && f '[$(npx playwright install)]'"#,
            r#"f() { unset "a$1"; }; (f '[$(npx playwright install)]')"#,
            r#"f() { unset "a$1"; }; echo x | f '[$(npx playwright install)]'"#,
        ],
        Verdict::TooComplex,
    );
}

#[test]
fn docker_exec_still_reads_what_psql_does_not_own() {
    // a shell given a script through docker is still read as one, also after a psql word, and a shell
    // word ends psql's `-c` words
    assert_all(
        &[
            r#"docker exec c sh -c "echo a(b)""#,
            r#"docker exec c bash -c "echo a(b)""#,
            r#"docker exec db psql -c "select count(*) from t" sh -c "echo a(b)""#,
            r#"docker exec db psql -c "select 1" sh -c "echo a" -c "echo a(b)""#,
            r#"docker exec db su -c "echo a(b)" u"#,
        ],
        Verdict::TooComplex,
    );
    assert_eq!(
        verdict(&format!(
            r#"docker exec c psql -c "select 1" sh -c "{MARKER}""#
        )),
        Verdict::Block(Rule::G4)
    );
    // an expansion in the argument runs on the host, whatever program gets the word
    assert_all(
        &[
            r#"docker exec c psql -c "$(npx playwright install)""#,
            r#"docker exec c psql -c "select `npx playwright install`""#,
            r#"docker exec c psql -Atc "select $(npx playwright install)""#,
            r#"docker exec c psql -c "select count(*) from t" && npx playwright install"#,
        ],
        Verdict::Block(Rule::G1),
    );
}

#[test]
fn a_subscript_tail_stored_in_a_word_and_joined_in_any_context_is_too_complex() {
    // every way to store the tail (`set --`, `for s in`, `read` with a here-string, `mapfile`, a default
    // value) against every way to join it (`unset`, `$((...))`, `((...))`), the pairs the other tests leave out
    assert_all(
        &[
            // `set --` with `unset` and both arithmetic contexts is in the test above
            r#"set -- '[`npx playwright install`]'; echo $((a$1))"#,
            r#"set -- '[$(npx playwright install)]'; s=$1; unset "a$s""#,
            r#"for s in '[$(npx playwright install)]'; do (( a$s )); done"#,
            r#"for s in '[$(npx playwright install)]'; do echo $((a$s)); done"#,
            r#"for s in '[`npx playwright install`]'; do unset "a$s"; done"#,
            r#"read s <<< '[$(npx playwright install)]'; (( a$s ))"#,
            r#"read s <<< '[$(npx playwright install)]'; echo $((a$s))"#,
            r#"read s <<< '[`npx playwright install`]'; unset "a$s""#,
            r#"read s <<< '[$(npx playwright install)]'; set -- "$s"; unset "a$1""#,
            r#"mapfile -t m <<< '[$(npx playwright install)]'; (( a${m[0]} ))"#,
            r#"mapfile -t m <<< '[$(npx playwright install)]'; echo $((a${m[0]}))"#,
            r#"mapfile -t m <<< '[$(npx playwright install)]'; set -- "${m[@]}"; unset "a$1""#,
            r#"mapfile -t m <<< '[$(npx playwright install)]'; unset "a${m[@]}""#,
            // a default value that takes the tail from another word
            r#"s='[$(npx playwright install)]'; echo $((a${u:-$s}))"#,
            r#"s='[$(npx playwright install)]'; (( a${u:-$s} ))"#,
            r#"set -- '[$(npx playwright install)]'; unset "a${u:-$1}""#,
        ],
        Verdict::TooComplex,
    );
    // a default value that holds the tail in the same word: the substitution is in view, so the command it
    // runs is checked and blocked (G1 here), never allowed
    assert_none_allowed(&[
        r#"echo $((a${s:-[$(npx playwright install)]}))"#,
        r#"(( a${s:-[$(npx playwright install)]} ))"#,
        r#"echo $((a${s-[`npx playwright install`]}))"#,
        r#"unset "a${s:-[$(npx playwright install)]}""#,
        r#"s=1; echo $((a${s:+[$(npx playwright install)]}))"#,
        r#"f() { unset "a${1:-[$(npx playwright install)]}"; }; f"#,
    ]);
}
