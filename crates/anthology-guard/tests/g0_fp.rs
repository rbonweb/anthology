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
//!    when every word from `exec` through `psql` and each option value is a plain literal (no expansion,
//!    glob or brace: the shell may split or expand one into more words, `docker exec $(echo c su) psql -c
//!    ...`), the options are the listed ones (not `--env-file`, not `-e PATH=...`), the program word is
//!    exactly `psql`, and the `-c` value holds no backslash (`\!` and `\o |` make psql start a shell), also
//!    none that an expansion could bring.

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

/// The script a `-c` word hands over, when it is read as a shell script: G4 names the program in it.
const MARKER: &str = "agent-browser open x";
/// SQL the docker reading cannot parse as a shell script (`count(*)`): G0 when it is read as one.
const SQL: &str = "select count(*) from t";

/// A command shape with `BODY` where the `-c` value goes.
#[track_caller]
fn shaped(shape: &str, body: &str) -> String {
    assert!(shape.contains("BODY"), "{shape}");
    shape.replace("BODY", body)
}

#[test]
fn sql_given_to_psql_through_docker_exec_is_data_not_a_script() {
    assert_all(
        &[
            r#"docker exec db psql -c "select count(*) from t""#,
            r#"docker exec db psql -Atc "select count(*) from t""#,
            "docker exec db psql -c 'select count(*) from t'",
            r#"docker exec -i db psql -U u -d d -c "select count(*) from t""#,
            r#"docker exec db psql -c "select count(*) from t" -d x"#,
            // the psql options the guard knows: a plain name for each value, plain flags
            r#"docker exec db psql -U u -h h -p 5432 -d d -c "select count(*) from t""#,
            r#"docker exec db psql --username u --host h --port 5432 --dbname d -c "select count(*) from t""#,
            r#"docker exec db psql -h /var/run/postgresql -c "select count(*) from t""#,
            r#"docker exec db psql -A -t -q -X -c "select count(*) from t""#,
            r#"docker exec db psql --csv -c "select count(*) from t""#,
            r#"docker exec db psql --command "select count(*) from t""#,
            r#"docker exec db psql --command "select 1" -c "select count(*) from t""#,
            r#"docker exec db psql -c "select count(*) from t" --command "select 1" d"#,
            r#"docker exec db psql d -c "select count(*) from t""#,
            r#"docker exec db psql -c "select count(*) from t" d u"#,
            r#"docker exec db psql -AtXc "select count(*) from t" d"#,
            // several -c values, each plain, with the reporting variables and the detach and input options
            r#"docker exec -u u c psql -U u -d db -v ON_ERROR_STOP=1 -c "create table t (a int)" -c "insert into t values (1)""#,
            r#"docker exec c psql -c "select count(*) from t" -c "select max(a) from t" --command "select 1""#,
            r#"docker exec c psql --set ON_ERROR_STOP=1 --variable AUTOCOMMIT=off -c "select count(*) from t""#,
            r#"docker exec -d c psql -U u -c "insert into t values (1)" -c "select count(*) from t""#,
            r#"docker exec --detach c psql -c "select count(*) from t""#,
            r#"docker exec -i c psql -c "select count(*) from t" -c "select 1""#,
            r#"docker exec -it -d c psql -c "select count(*) from t""#,
            r#"docker compose exec -d db psql -v ON_ERROR_STOP=1 -c "select count(*) from t""#,
            // a quoted `*`, `?` and `[` are SQL
            r#"docker exec db psql -c "select * from t where a like 'x_' and b = (select a[1] from u limit 1)""#,
            // docker compose and docker-compose read the same way
            r#"docker compose exec db psql -c "select count(*) from t""#,
            r#"docker-compose exec db psql -Atc "select count(*) from t""#,
            // each value option of exec takes the next word, each boolean option takes none
            r#"docker exec -e PGPASSWORD=x db psql -c "select count(*) from t""#,
            r#"docker exec --env PGUSER=u db psql -c "select count(*) from t""#,
            r#"docker exec -e PGPASSWORD db psql -c "select count(*) from t""#,
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
            r#"docker exec -it -u postgres -w /tmp -e PGPASSWORD=x db psql -c "select count(*) from t""#,
            // the neighbours that were already allowed
            r#"psql -c "select count(*) from t""#,
            r#"docker exec db psql -c "select 1""#,
            r#"docker exec db mysql -e "select count(*) from t""#,
            r#"docker exec db echo "a(b)""#,
            r#"docker run --rm img echo "count(*)""#,
            // docker exempts G1 and G2: running the install in the container is what G1 asks for
            r#"docker exec c sh -c "npx playwright install""#,
            "docker exec c npx playwright install",
        ],
        Verdict::Allow,
    );
}

#[test]
fn a_psql_value_with_a_backslash_or_an_expansion_character_is_still_read_as_a_script() {
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
        // the value is a script when it holds a character an expansion could turn into a backslash
        format!(r"docker exec c psql -c '$x; {MARKER}'"),
        format!(r"docker exec c psql -c '~; {MARKER}'"),
        format!(r"docker exec c psql -c '^; {MARKER}'"),
        format!(r"docker exec c psql -c 'a#; {MARKER}'"),
    ] {
        assert_eq!(verdict(&command), Verdict::Block(Rule::G4), "{command}");
    }
    assert_eq!(
        verdict(r"docker exec c psql -c '\! echo a(b)'"),
        Verdict::TooComplex
    );
    // the same SQL without a backslash is data
    assert_eq!(
        verdict(r#"docker exec c psql -c "select count(*) from t" d"#),
        Verdict::Allow
    );
}

/// Shapes where a word between `exec` and `psql`, or an option value, is not a plain literal: the shell
/// splits or expands it into other words at run time (`docker exec $(echo c su) psql -c CMD` runs
/// `su psql -c CMD` in the container `c`), so psql is not the program and the `-c` is `su`'s. Each keeps the
/// reading it has had (G4 for a script, G0 for SQL it cannot parse), and the run-time proof of each is in
/// bash 5.2 and zsh 5.9.
const SPLIT_WORD_SHAPES: &[&str] = &[
    r#"docker exec $(echo c su) psql -c "BODY""#,
    r#"docker exec `echo c su` psql -c "BODY""#,
    r#"docker exec {c,su} psql -c "BODY""#,
    r#"C='c su'; docker exec $C psql -c "BODY""#,
    r#"C='c su'; docker exec ${=C} psql -c "BODY""#,
    r#"docker exec c* psql -c "BODY""#,
    r#"docker exec -e $(echo A=b c su) psql -c "BODY""#,
    r#"docker exec -u $(echo u c su) psql -c "BODY""#,
    r#"docker exec "$(echo c su)" psql -c "BODY""#,
    r#"docker exec c$(echo)x psql -c "BODY""#,
    r#"docker exec c ps$(echo)ql -c "BODY""#,
    r#"docker exec c psq? -c "BODY""#,
    // zsh runs `=name` as the path of a program, and `~` is the home folder
    r#"docker exec =c psql -c "BODY""#,
    r#"docker exec -u =x c psql -c "BODY""#,
    r#"docker exec ~ psql -c "BODY""#,
    r#"docker exec c psql -c "BODY" $(echo x)"#,
];

/// Shapes where the `-c` value gets a backslash at run time: a parameter, a glob, an ANSI-C quote or an
/// expansion makes `\! CMD` out of text with none (`psql -c "$Q"` with `Q='\! CMD'`).
const EXPANDED_VALUE_SHAPES: &[&str] = &[
    r#"B='\'; docker exec c psql -c "${B}! BODY""#,
    r#"docker exec c psql -c "${(#):-92}! BODY""#,
    r#"docker exec c psql -c $'\\! BODY'"#,
    r#"docker exec c psql -c $'\x5c! BODY'"#,
];

/// Shapes that set what runs as `psql` (or the program itself): a plant in the container is named `psql`
/// and found through `PATH`, an env file or a variable can set `PATH`, `PAGER`, `LD_PRELOAD` or
/// `PGPASSCOMMAND`.
const PLANTED_PSQL_SHAPES: &[&str] = &[
    r#"docker exec -e PATH=/tmp:/usr/bin c psql -c "BODY""#,
    r#"docker exec --env PATH=/tmp c psql -c "BODY""#,
    r#"docker exec -e PATH c psql -c "BODY""#,
    r#"docker exec --env-file f c psql -c "BODY""#,
    r#"docker exec -e PGPASSCOMMAND=x c psql -c "BODY""#,
    r#"docker exec -e PAGER=x c psql -c "BODY""#,
    r#"docker exec -e LD_PRELOAD=/tmp/x.so c psql -c "BODY""#,
    r#"docker exec -e A=b c psql -c "BODY""#,
];

/// A psql in the wrong place, or not named exactly: the `-c` belongs to another program.
const NOT_PSQL_SHAPES: &[&str] = &[
    r#"docker exec c su psql -c "BODY""#,
    r#"docker exec c su -l psql -c "BODY""#,
    r#"docker exec c runuser psql -c "BODY""#,
    r#"docker exec c script -q psql -c "BODY""#,
    r#"docker exec c flock /tmp/psql -c "BODY""#,
    r#"docker exec -u psql c python3 -c "BODY""#,
    r#"docker exec -w /srv/psql c python3 -c "BODY""#,
    r#"docker exec -e PGUSER=psql c python3 -c "BODY""#,
    r#"docker run --name psql img python3 -c "BODY""#,
    r#"docker run img psql -c "BODY""#,
    r#"docker compose run psql python3 -c "BODY""#,
    r#"docker compose run c psql -c "BODY""#,
    r#"docker compose exec2 c psql -c "BODY""#,
    r#"docker-compose run c psql -c "BODY""#,
    r#"docker compose c psql -c "BODY""#,
    // an option exec does not list: nothing says where the program is
    r#"docker exec --unknown c psql -c "BODY""#,
    r#"docker exec -u=u c psql -c "BODY""#,
    r#"docker exec -T c psql -c "BODY""#,
    r#"docker -H h exec c psql -c "BODY""#,
    // the exact name: not another case, not a path, not behind a wrapper
    r#"docker exec c PSQL -c "BODY""#,
    r#"docker exec c /usr/bin/psql -c "BODY""#,
    r#"docker exec c env psql -c "BODY""#,
    // no container: `psql` is the container, and the `-c` is nobody's psql
    r#"docker exec psql -c "BODY""#,
];

/// A redirect anywhere in the command: the lexer takes `c<1-5>` for a `<` and a `>` redirect, and the `>`
/// swallows the next word, but zsh reads `<1-5>` as a numeric glob word (`c1` to `c5`), so the words the guard
/// sees are not the words that run (`docker exec c<1-5> flock psql -c ...` runs `flock psql -c ...` in the
/// container `c1`, and `flock` runs the value with `sh`). Any redirect in the command keeps the old reading.
const REDIRECT_SHAPES: &[&str] = &[
    r#"docker exec c<1-5> flock psql -c "BODY""#,
    r#"docker exec c<-> flock psql -c "BODY""#,
    r#"docker exec c<1-5>flock psql -c "BODY""#,
    r#"docker exec c<1-> flock psql -c "BODY""#,
    r#"docker exec -u u<1-5> c psql -c "BODY""#,
    r#"docker exec -e PGUSER=u<1-5> c psql -c "BODY""#,
    r#"docker exec c psql -c "BODY" > /dev/null"#,
    r#"docker exec c psql -c "BODY" 2>&1"#,
    r#"docker exec c psql -c "BODY" < /dev/null"#,
    r#"docker exec c psql -c "BODY" <<< x"#,
];

/// The options of psql other than the few the guard knows: `-o '|CMD'`, `--output`, `-f -` fed by a
/// here-string, `-L` and `-P pager=...` make psql run a shell or a program, an attached value or a conninfo
/// string (`host=h`, `postgres://h/d`) is not a plain name, and a second `-c`, `--command=` or an
/// unknown option is not a shape the guard has checked. Each keeps the old reading.
/// An unquoted paren anywhere in the command: the lexer ends the segment at it, but zsh reads `(-o|-x)` in a
/// word list as a glob group that expands to one of its words when a file of that name exists, so
/// `psql -c SQL (-o|-x) '|cmd'` is `psql -c SQL -o '|cmd'` and runs `cmd`. Any paren keeps the old reading.
const PAREN_SHAPES: &[&str] = &[
    r#"docker exec c psql -c "BODY" (-o|-x) '|agent-browser open x'"#,
    r#"docker exec c psql -c "BODY" (-o) '|agent-browser open x'"#,
    r#"docker exec c psql -c "BODY" (-o|-x)"#,
    r#"(echo x); docker exec c psql -c "BODY""#,
    r#"docker exec c psql -c "BODY"; (echo x)"#,
    r#"echo x | (cat); docker exec c psql -c "BODY""#,
    r#"f() { echo x; }; docker exec c psql -c "BODY""#,
];

#[test]
fn a_text_the_lexer_could_not_balance_keeps_its_reading() {
    // the whitespace fallback splits it, so its words are not the command's: a script is still found in a
    // `-c` value (G4); SQL there was Allow on the base and stays so
    for command in [
        format!(r#"docker exec c psql -c "{MARKER}" '"#),
        format!(r#"docker exec c psql -c "{MARKER}" "x"#),
        format!(r#"docker exec c psql -U u -c "{MARKER}" '"#),
    ] {
        assert_eq!(verdict(&command), Verdict::Block(Rule::G4), "{command}");
    }
}

const PSQL_OPTION_SHAPES: &[&str] = &[
    r#"docker exec c psql -o '|agent-browser open x' -c "BODY""#,
    r#"docker exec c psql -c "BODY" -o '|agent-browser open x'"#,
    r#"docker exec c psql --output='|agent-browser open x' -c "BODY""#,
    r#"docker exec c psql --output '|agent-browser open x' -c "BODY""#,
    r#"docker exec -i c psql -f - -c "BODY" <<< '\! agent-browser open x'"#,
    r#"docker exec c psql -f x.sql -c "BODY""#,
    r#"docker exec c psql --file x.sql -c "BODY""#,
    r#"docker exec c psql -L x.log -c "BODY""#,
    r#"docker exec c psql --log-file x.log -c "BODY""#,
    r#"docker exec c psql -P pager=x -c "BODY""#,
    r#"docker exec c psql -Ppager=x -c "BODY""#,
    r#"docker exec c psql --pset=pager=x -c "BODY""#,
    r#"docker exec c psql -v X=1 -c "BODY""#,
    r#"docker exec c psql -Uu -c "BODY""#,
    r#"docker exec c psql --username=u -c "BODY""#,
    r#"docker exec c psql -hh -c "BODY""#,
    r#"docker exec c psql -d host=h -c "BODY""#,
    r#"docker exec c psql -d postgres://h/d -c "BODY""#,
    r#"docker exec c psql --dbname=postgres://h/d -c "BODY""#,
    r#"docker exec c psql host=h -c "BODY""#,
    r#"docker exec c psql postgres://h/d -c "BODY""#,
    r#"docker exec c psql -c "BODY" host=h"#,
    r#"docker exec c psql -U -c "BODY""#,
    // a variable psql is not known to leave alone, an expansion or a missing value
    r#"docker exec c psql -v PROMPT1=x -c "BODY""#,
    r#"docker exec c psql -v ON_ERROR_STOP -c "BODY""#,
    r#"docker exec c psql -v ON_ERROR_STOP=$x -c "BODY""#,
    r#"docker exec c psql -v ON_ERROR_STOP=1 -c"#,
    r#"docker exec c psql --set HISTFILE=x -c "BODY""#,
    r#"docker exec c psql -c "BODY" -c"#,
    r#"docker exec c psql -c "BODY" a b c"#,
    // a backslash meta-command in any value: `\!` and `\copy ... to program` run a shell, and `--command` is not
    // a word the script scan reads, so its value was never looked at
    r#"docker exec c psql -c "BODY" --command '\! agent-browser open x'"#,
    r#"docker exec c psql --command '\! agent-browser open x' -c "BODY""#,
    r#"docker exec c psql -c "BODY" --command '\o |agent-browser open x'"#,
    r#"docker exec c psql -c "BODY" -c "\copy t to program 'agent-browser open x'""#,
    r#"docker exec c psql -c "\copy t to program 'agent-browser open x'" -c "BODY""#,
    r#"docker exec c psql -c "BODY" --command '$x'"#,
    r#"docker exec c psql -c "BODY" --command"#,
    r#"docker exec c psql -d -c "BODY""#,
    r#"docker exec c psql -c "BODY" -"#,
    r#"docker exec c psql -cA "BODY""#,
    r#"docker exec c psql -- -c "BODY""#,
    r#"docker exec c psql --unknown -c "BODY""#,
];

#[test]
fn a_command_that_is_not_plain_literal_text_keeps_its_reading() {
    let mut wrong: Vec<String> = Vec::new();
    for (name, shapes) in [
        ("split word", SPLIT_WORD_SHAPES),
        ("expanded value", EXPANDED_VALUE_SHAPES),
        ("planted psql", PLANTED_PSQL_SHAPES),
        ("not psql", NOT_PSQL_SHAPES),
        ("redirect", REDIRECT_SHAPES),
        ("psql option", PSQL_OPTION_SHAPES),
        ("paren", PAREN_SHAPES),
    ] {
        for shape in shapes.iter().filter(|s| s.contains("BODY")) {
            for (body, want) in [
                (MARKER, Verdict::Block(Rule::G4)),
                (SQL, Verdict::TooComplex),
            ] {
                let command = shaped(shape, body);
                let got = verdict(&command);
                if got != want {
                    wrong.push(format!("{name}: {command}: {got:?}, want {want:?}"));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn a_value_that_a_variable_or_a_glob_fills_in_is_never_allowed() {
    // the value's backslash arrives by expansion (`Q='\! CMD'`, `-c *` with a file of that name): the
    // verdict is the one these had before, never an allow
    for command in [
        format!(r#"Q='\! {MARKER}'; docker exec c psql -c "$Q""#),
        format!(r#"Q='\! {SQL}'; docker exec c psql -c "$Q""#),
        format!(r#"Q='\! {MARKER}'; docker exec c psql -c $Q"#),
        format!(r#"Q='\! {MARKER}'; docker exec c psql -Atc "$Q""#),
        r"docker exec c psql -c *".to_string(),
        r"docker exec c psql -c ?".to_string(),
        r"docker exec c psql -c [a-z]*".to_string(),
        r"docker exec c psql -Atc *".to_string(),
        r"docker exec c psql -c *.sql -c *".to_string(),
        format!(r"docker exec c psql -c '`x`; {MARKER}'"),
        // a brace in a SQL value: the shell may split it into more words
        format!(r#"docker exec c psql -c "{SQL}"{{,}}"#),
        format!(r#"docker exec c psql -c "{SQL}" -d x{{a,b}}"#),
    ] {
        assert_eq!(verdict(&command), Verdict::TooComplex, "{command}");
    }
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
fn a_psql_whose_arguments_come_from_somewhere_else_keeps_its_reading() {
    // `xargs` adds stdin words to the command, a wrapper's own words are not the command's
    assert_all(
        &[
            r#"echo x | xargs docker exec c psql -c "select count(*) from t""#,
            r#"echo x | xargs -I{} docker exec c psql -c "select count(*) from t" {}"#,
            r#"env -S 'docker exec c psql -c "select count(*) from t"'"#,
        ],
        Verdict::TooComplex,
    );
    // a wrapper that leaves the words as they are changes nothing
    assert_all(
        &[
            r#"sudo docker exec c psql -c "select count(*) from t""#,
            r#"time docker exec c psql -c "select count(*) from t""#,
            r#"FOO=1 docker exec c psql -c "select count(*) from t""#,
        ],
        Verdict::Allow,
    );
}
