//! PE-1 to PE-6 (security review): commands whose program, script or evaluated text comes from a value
//! the guard cannot see. Each command below ran a marker `npx` in bash 5.2 or zsh 5.9 (zsh is the shell
//! on macOS), checked in a container, and each was allowed before the fix. They are too complex now (G0),
//! or blocked by the rule a reading of them hits. The ordinary forms at the end stay allowed.

#[path = "support/gd.rs"]
mod gd;

use anthology_guard::{Verdict, check};

use gd::{HOME, bash};

fn verdict(command: &str) -> Verdict {
    let (tool, input) = bash(command);
    check(&tool, &input, HOME)
}

/// Every command gets `want`; the failure lists every one that does not.
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

/// Every command is blocked, by any rule.
#[track_caller]
fn assert_blocked(commands: &[&str]) {
    let allowed: Vec<&str> = commands
        .iter()
        .copied()
        .filter(|c| verdict(c) == Verdict::Allow)
        .collect();
    assert!(allowed.is_empty(), "allowed:\n{}", allowed.join("\n"));
}

#[test]
fn pe1_a_program_word_from_a_parameter_expansion_is_too_complex() {
    assert_all(
        &[
            r#"x=eval; $x "npx playwright install""#,
            r#"x=bash; $x -c "npx playwright install""#,
            "x=npx; $x playwright install",
            "x=npx; ${x} playwright install",
            r#"x=npx; "$x" playwright install"#,
            "a=(npx); ${a[0]} playwright install",
            "a=(npx); ${a[1]} playwright install",
            "set -- npx; $1 playwright install",
            "set npx; $1 playwright install",
            "argv=(npx); $1 playwright install",
            // zsh: a subscript, `=cmd` and word splitting of the value
            "x=npx; $x[1,3] playwright install",
            "x=npx; ${x[1,3]} playwright install",
            "x=npx; =$x playwright install",
            "x='npx playwright install'; ${=x}",
            "x='npx playwright install'; $=x",
            "x='npx playwright install'; ${(z)x}",
        ],
        Verdict::TooComplex,
    );
}

#[test]
fn pe1_wrappers_and_compound_commands_are_looked_through() {
    // blocked by G1 or too complex: another reading of a wrapper's words or of `set`'s words names the
    // install too
    assert_blocked(&[
        "x=npx; env $x playwright install",
        "x=npx; command $x playwright install",
        "x=npx; nice $x playwright install",
        "x=npx; time $x playwright install",
        "x=npx; exec $x playwright install",
        r#"set -- npx playwright install; "$@""#,
    ]);
    assert_all(
        &[
            "x=npx; FOO=1 $x playwright install",
            "x=npx; echo playwright install | xargs $x",
            "x=npx; if $x playwright install; then :; fi",
            "x=npx; while $x playwright install; do break; done",
            "x=npx; ! $x playwright install",
            "x=npx; { $x playwright install; }",
            "x=npx; ( $x playwright install )",
            r"x=npx; find . -maxdepth 0 -exec $x playwright install \;",
            r#"x=npx; f() { "$@"; }; f "$x" playwright install"#,
        ],
        Verdict::TooComplex,
    );
    // a default value as the program word is read as that program too, so its rule still names itself
    assert_blocked(&["x=; ${x:-npx} playwright install"]);
}

#[test]
fn pe1_a_script_from_a_parameter_expansion_is_too_complex() {
    assert_all(
        &[
            "x='npx playwright install'; eval $x",
            r#"x='npx playwright install'; eval "$x""#,
            r#"x='npx playwright install'; sh -c "$x""#,
            r#"x='npx playwright install'; bash -c "echo; $x""#,
            r#"x='npx playwright install'; env -S "$x""#,
            r#"x='npx playwright install'; find . -maxdepth 0 -exec sh -c "$x" \;"#,
        ],
        Verdict::TooComplex,
    );
}

#[test]
fn pe2_a_zsh_glob_qualifier_is_too_complex() {
    assert_all(
        &[
            "ls *(e:'npx playwright install':)",
            "ls *(+npx)",
            "ls a.rs(e:'npx playwright install':)",
            "ls a.rs(+npx)",
            r#"echo "a.rs"(+npx)"#,
            "ls ?.rs(+npx)",
            "ls [a].rs(+npx)",
            "ls **/*(+npx)",
            "ls (a|b).rs(+npx)",
            "ls *.rs(.e:'npx playwright install':)",
            "setopt extendedglob; ls a.rs(#q+npx)",
            "ls *(e{npx playwright install})",
            "ls *(e[npx playwright install])",
        ],
        Verdict::TooComplex,
    );
    // zsh's `=(cmd)` runs `cmd`
    assert_blocked(&["cat =(npx playwright install)"]);
}

#[test]
fn pe3_a_value_expanded_as_code_is_too_complex() {
    assert_all(
        &[
            // bash prompt expansion runs a `$(...)` in the value
            r#"x='$(npx playwright install)'; echo "${x@P}""#,
            r#"x='$(npx playwright install)'; echo "${x[@]@P}""#,
            r#"x='$(npx playwright install)'; y=x; echo "${!y@P}""#,
            // zsh's (e) flag
            "x='$(npx playwright install)'; echo ${(e)x}",
            r#"x='$(npx playwright install)'; echo "${(e)x}""#,
            "x='$(npx playwright install)'; echo ${(ef)x}",
            "x='$(npx playwright install)'; echo ${(eL)x}",
            "x='$(npx playwright install)'; echo ${${x}:+${(e)x}}",
            "x='$(npx playwright install)'; echo ${(e)${x}}",
            "x='$(npx playwright install)'; echo ${(e)x:-y}",
            // zsh GLOB_SUBST: the value's glob qualifier runs
            "x='*(e:npx playwright install:)'; touch q; echo ${~x}",
            "x='*(e:npx playwright install:)'; touch q; echo $~x",
            "x='*(e:npx playwright install:)'; touch q; setopt globsubst; echo $x",
            "x='*(e:npx playwright install:)'; touch q; echo ${(e)~x}",
            // a prompt with a substitution in it: bash's `set -x` trace, zsh's PROMPT_SUBST
            "PS4='$(npx playwright install)'; set -x; echo",
            "setopt promptsubst xtrace; PS4='$(npx playwright install)'; echo",
            "setopt promptsubst; print -P '$(npx playwright install)'",
        ],
        Verdict::TooComplex,
    );
}

#[test]
fn pe4_a_read_value_evaluated_as_a_subscript_or_expression_is_too_complex() {
    assert_all(
        &[
            r#"a=(1); read s <<< '[$(npx playwright install)]'; unset "a$s""#,
            r#"a=(1); mapfile -t m <<< '[$(npx playwright install)]'; unset "a${m[0]}""#,
            r#"a=(1); readarray -t m <<< '[$(npx playwright install)]'; unset "a${m[0]}""#,
            "a=(1); read s <<'EOF'\n[$(npx playwright install)]\nEOF\nunset \"a$s\"",
            "a=(1); read s <<EOF\n[\\$(npx playwright install)]\nEOF\nunset \"a$s\"",
            "read s <<< 'a[$(npx playwright install)]'; echo $(( s ))",
            "read s <<< 'a[$(npx playwright install)]'; [[ $s -eq 1 ]]",
            "read s <<< 'a[$(npx playwright install)]'; [[ s -eq 1 ]]",
            r#"read s <<< 'a[$(npx playwright install)]'; x=abc; echo "${x:$s}""#,
            r#"read s <<< 'a[$(npx playwright install)]'; x=abc; echo "${x:s}""#,
            "read s <<< 'a[$(npx playwright install)]'; declare -i n; n=$s",
            r#"read s <<< '[$(npx playwright install)]'; declare "a$s=1""#,
            r#"read s <<< '$(npx playwright install)'; printf -v "a[$s]" x"#,
            r#"read s <<< '$(npx playwright install)'; printf -v t %s "$s"; printf -v "a[$t]" x"#,
            "read s <<< 'b[$(npx playwright install)]'; a[$s]=1",
            "read s <<< 'b[$(npx playwright install)]'; a=(1); echo ${a[$s]}",
            "read s <<< 'b[$(npx playwright install)]'; a=(1); echo ${a[s]}",
            "read s <<< 'b[$(npx playwright install)]'; a=(1); echo ${a[@]:$s}",
            "read s <<< 'b[$(npx playwright install)]'; echo ${!s}",
            "read s <<< 'b[$(npx playwright install)]'; [[ -v $s ]]",
            r#"read s <<< 'b[$(npx playwright install)]'; test -v "$s""#,
            "read s <<< 'b[$(npx playwright install)]'; for ((i=s;i<1;i++)); do :; done",
            "read s <<< 'b[$(npx playwright install)]'; read -t $s x </dev/null",
            r#"while read -r k v; do export "$k=$v"; done < .env"#,
            // the same with the value set from quoted text
            r#"x='$(npx playwright install)'; printf -v "a[$x]" y"#,
            r#"x='$(npx playwright install)'; a=(1); unset "a[$x]""#,
            r#"k='a[$(npx playwright install)]'; export "$k=1""#,
            r#"k='a[$(npx playwright install)]'; f(){ local "$k=1"; }; f"#,
            r#"k='a[$(npx playwright install)]'; typeset "$k=1""#,
        ],
        Verdict::TooComplex,
    );
}

#[test]
fn pe5_a_subscript_with_nested_brackets_or_quotes_is_too_complex() {
    // `unset` reads its subscripts' substitutions as commands (G1 here); any other word is too complex
    assert_blocked(&[
        "a=(1); unset 'a[[]$(npx playwright install)]'",
        r#"a=(1); unset 'a["]"$(npx playwright install)]'"#,
        "a=(1); unset 'a[[]`npx playwright install`]'",
        "a=(1); declare -n r='a[[]$(npx playwright install)]'; echo $r",
        r#"a=(1); declare -n r='a["]"$(npx playwright install)]'; echo $r"#,
    ]);
    // a word that names such a subscript, which bash evaluates when the name is expanded
    assert_all(
        &[
            "a=(1); x='a[[]$(npx playwright install)]'; echo ${!x}",
            r#"a=(1); x='a["]"$(npx playwright install)]'; echo ${!x}"#,
            "a=(1); x='a[[]`npx playwright install`]'; echo ${!x}",
            // the subscript scan itself, in any word, as it is for a subscript with no nesting
            // (`echo 'a[$(id)]'` was too complex before)
            "echo 'a[[]$(npx playwright install)]'",
            r#"echo 'a["]"$(npx playwright install)]'"#,
        ],
        Verdict::TooComplex,
    );
}

#[test]
fn pe6_a_renamed_program_is_too_complex() {
    assert_all(
        &[
            "alias u='npx playwright install'; u",
            "shopt -s expand_aliases; alias u='npx playwright install'\nu",
            "shopt -s expand_aliases; alias u=npx\nu playwright install",
            "alias -g U='npx playwright install'\necho U",
            "BASH_ALIASES[u]=npx; shopt -s expand_aliases\nu playwright install",
            "aliases[u]=npx\nu playwright install",
            "functions[u]='npx playwright install'; u",
            "hash -p /usr/local/bin/npx u; u playwright install",
            "BASH_CMDS[u]=/usr/local/bin/npx; u playwright install",
            "hash u=/usr/local/bin/npx; u playwright install",
            "commands[u]=/usr/local/bin/npx; u playwright install",
        ],
        Verdict::TooComplex,
    );
}

#[test]
fn ordinary_commands_stay_allowed() {
    assert_all(
        &[
            "echo $HOME",
            "ls $DIR",
            r#"cd "$x" && ls"#,
            "git log $REF",
            r#"for f in *.rs; do wc -l "$f"; done"#,
            "ls *.rs",
            "echo ${x:-default}",
            "echo $#",
            r#"printf '%s\n' "$x""#,
            r#"export PATH="$HOME/bin:$PATH""#,
            r#"git commit -m "fix(guard): message""#,
            "set -euo pipefail; ls",
            // functions and positional parameters the analysis puts in place
            "f() { echo hi; }; f",
            r#"run() { "$@"; }; run make test"#,
            r#"retry() { for i in 1 2 3; do "$@" && return; sleep 1; done; }; retry curl -fsS localhost:3000"#,
            r#"sh -c 'echo "$1"' _ hello"#,
            r#"find . -name '*.md' -exec sh -c 'wc -l "$1"' _ {} \;"#,
            // arithmetic and patterns, where a parenthesis follows a word
            "echo $(( a*(2) ))",
            r#"echo "$(( a*(2) ))""#,
            "(( y = 2*(3) ))",
            "x=1; echo $(( $x + 1 ))",
            "sh -c 'n=$(( 2 * 3 )); echo $n'",
            "(( $# > 0 )) && echo yes",
            "[[ abc =~ ^(a|b) ]]",
            // expansions that run no code
            r#"x='$(date)'; echo "${x@Q}""#,
            r#"x='$(date)'; echo "${x@E}""#,
            // read loops that only print or pass a value on
            r#"while read -r line; do echo "$line"; done < f"#,
            r#"git log --oneline | while read -r l; do echo "${l:0:7}"; done"#,
            r#"mapfile -t files < <(git ls-files); echo "${files[@]}""#,
            r#"while read -r f; do [[ $f == *.rs ]] && echo "$f"; done < list"#,
            // aliases and hashes that are only listed or removed
            "alias",
            "alias ll",
            "unalias ll",
            "hash -r",
            // a command's output in arithmetic (a known limit, see DECISIONS.md)
            "n=$(wc -l < f); echo $((n * 2))",
        ],
        Verdict::Allow,
    );
}
