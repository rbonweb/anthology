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
fn pe1_a_positional_program_the_analysis_did_not_fill_is_too_complex() {
    assert_all(
        &[
            // a shell reading its script from stdin, with its own arguments
            "bash -s npx <<< '$1 playwright install'",
            "echo '$1 playwright install' | bash -s npx",
            r#"bash /dev/stdin npx playwright install <<< '"$@"'"#,
            r#"source /dev/stdin npx playwright install <<< '"$@"'"#,
            // a shell's arguments the analysis cannot see
            r#"find /usr/local/bin -name npx -exec sh -c '"$1" playwright install' _ {} \;"#,
            r#"find /usr/local/bin -name npx -exec sh -c '"$@" playwright install' sh {} +"#,
            // a function called where the analysis does not put its arguments in place
            r#"f() { "$@"; }; trap 'f npx playwright install' EXIT"#,
            r#"f() { "$@"; }; eval 'f npx playwright install'"#,
            r#"f() ( "$@" ); f npx playwright install"#,
            r#"() { "$@"; } npx playwright install"#,
            r#"function { "$@"; } npx playwright install"#,
            // positional parameters set some other way
            r#"read -A argv <<< 'npx playwright install'; "$@""#,
            r#"sh -c 'shift; "$1" playwright install' _ ls npx"#,
            r#""$@""#,
            "$1 playwright install",
            // the handler zsh and bash call with a missing command's words
            "command_not_found_handler() { $2 playwright install; }; nosuchcmd npx",
            "command_not_found_handle() { $2 playwright install; }; nosuchcmd npx",
            "command_not_found_handle() { :; }",
        ],
        Verdict::TooComplex,
    );
}

#[test]
fn pe1_a_function_run_where_the_analysis_does_not_read_its_call_is_blocked() {
    assert_blocked(&[
        r#"f() "$@"; f npx playwright install"#,
        r#"f() { :; }; f() { "$@"; }; f npx playwright install"#,
        r#"eval 'f() { "$@"; }'; f npx playwright install"#,
        r#"sh -c 'f() { "$@"; }; trap "f npx playwright install" EXIT' _ ls"#,
        r#"f() { "$@"; }; export -f f; bash -c 'f npx playwright install'"#,
        r#"f() { "$@"; }; echo playwright install | xargs f npx"#,
        // a dynamic name or a bare nameref set to a prompt
        r#"x=PS4; declare "$x"='$(npx playwright install)'; set -x; echo"#,
        "declare -n p; p=PS4; p='$(npx playwright install)'; set -x; echo",
    ]);
}

#[test]
fn pe6_a_table_named_in_any_word_is_too_complex() {
    assert_all(
        &[
            "printf -v 'BASH_CMDS[u]' /usr/local/bin/npx; u playwright install",
            "shopt -s expand_aliases; printf -v 'BASH_ALIASES[u]' npx\nu playwright install",
            "declare -n r=BASH_CMDS; r[u]=/usr/local/bin/npx; u playwright install",
            "set -A commands u /usr/local/bin/npx; u playwright install",
            "set -A functions u 'npx playwright install'; u",
            "set -A aliases u npx\nu playwright install",
            "env 'BASH_FUNC_u%%=() { npx playwright install; }' bash -c u",
        ],
        Verdict::TooComplex,
    );
}

#[test]
fn pe3_prompts_startup_files_heredocs_and_dynamic_options_are_too_complex() {
    assert_all(
        &[
            "declare -n p=PS4; p='$(npx playwright install)'; set -x; echo",
            "BASH_ENV='$(npx playwright install)' bash -c :",
            "ENV='$(npx playwright install)' bash --posix -i -c :",
            "x='$(npx playwright install)'; cat <<EOF\n${(e)x}\nEOF",
            "x='$(npx playwright install)'; cat <<EOF\n${x@P}\nEOF",
            "o=glob; setopt ${o}subst; x='*(e:npx playwright install:)'; touch q; echo $x",
            "o=glob; set -o ${o}subst; x='*(e:npx playwright install:)'; touch q; echo $x",
        ],
        Verdict::TooComplex,
    );
}

#[test]
fn pe2_a_glob_qualifier_after_a_group_or_in_a_default_value_is_too_complex() {
    assert_all(
        &[
            "touch a b; ls (a|b)(e:'npx playwright install':)",
            "touch a.rs; echo ${x:-a.rs(e:'npx playwright install':)}",
        ],
        Verdict::TooComplex,
    );
}

#[test]
fn zsh_precommand_words_coproc_globbed_and_placeholder_programs_are_read() {
    assert_blocked(&[
        "noglob npx playwright install",
        "nocorrect npx playwright install",
        "=npx playwright install",
        "coproc c { npx playwright install; }",
        "coproc npx playwright install",
    ]);
    assert_all(
        &[
            "x=npx; noglob $x playwright install",
            "/usr/local/bin/np? playwright install",
            "/usr/local/bin/n*x playwright install",
            r"find /usr/local/bin -name npx -exec {} playwright install \;",
        ],
        Verdict::TooComplex,
    );
}

/// One test per input of the carve-out re-review of #159: each ran a marker `npx` in bash 5.2 or zsh 5.9
/// and was allowed before the fix.
macro_rules! too_complex {
    ($($name:ident: $command:expr;)*) => {
        $(
            #[test]
            fn $name() {
                assert_eq!(verdict($command), Verdict::TooComplex, "{}", $command);
            }
        )*
    };
}

too_complex! {
    // (a) a prefix name set where the analysis does not look, or PWD, which an earlier call set
    carve_a_zsh_default_assignment_to_home:
        r#": ${HOME::='/tmp;npx playwright install;:'}; sh -c "ls $HOME/x""#;
    carve_a_zsh_print_v_home:
        r#"print -v HOME '/tmp;npx playwright install;:'; sh -c "ls $HOME/x""#;
    carve_a_bash_printf_v_attached_home:
        r#"printf -vHOME %s '/tmp;npx playwright install;:'; sh -c "ls $HOME/x""#;
    carve_a_pwd_in_a_double_quoted_script: r#"sh -c "ls $PWD/z""#;
    carve_a_home_set_through_a_name_built_from_pieces:
        r#"x=HO; x+=ME; printf -v "$x" %s '/tmp;npx playwright install;:'; sh -c "ls $HOME/x""#;
    // (b) a read value reached through a plain name, or read by select
    carve_b_a_plain_name_copy_in_arithmetic:
        "while read -r line; do n=line; echo $(( n )); done <<< 'a[$(npx playwright install)]'";
    carve_b_a_plain_name_copy_in_a_subscript:
        r#"a=(1); while read -r line; do n=line; echo "${a[n]}"; done <<< 'b[$(npx playwright install)]'"#;
    carve_b_a_plain_name_copy_in_an_arithmetic_test:
        "while read -r line; do n=line; [[ n -eq 1 ]]; done <<< 'a[$(npx playwright install)]'";
    carve_b_select_sets_reply:
        "select x in a; do echo $(( REPLY )); break; done <<< 'a[$(npx playwright install)]'";
    carve_b_a_plain_name_copy_of_a_mapfile_array:
        "mapfile -t m <<< 'a[$(npx playwright install)]'; n=m; echo $(( n ))";
    carve_b_a_subscripted_name_copy:
        "while read -r line; do n='line[0]'; echo $(( n )); done <<< 'a[$(npx playwright install)]'";
    // (c) a direct call whose words are not literal, or positional parameters moved by zsh
    carve_c_a_glob_in_a_direct_call: r#"f() { "$@"; }; f /usr/local/bin/np? playwright install"#;
    carve_c_zparseopts_moves_the_parameters:
        r#"f() { zparseopts -D a=o; "$@"; }; f -a npx playwright install"#;
    carve_c_a_numeric_assignment_sets_a_parameter: r#"f() { 1=npx; "$@"; }; f ls playwright install"#;
}

/// One test per input of the second carve-out re-review of #159: each ran a marker `npx` in bash 5.2 or zsh
/// 5.9 and was allowed before the fix. Each must be too complex (G0) or blocked by G1.
macro_rules! g0_or_g1 {
    ($($name:ident: $command:expr;)*) => {
        $(
            #[test]
            fn $name() {
                let got = verdict($command);
                assert!(
                    matches!(got, Verdict::TooComplex | Verdict::Block(anthology_guard::Rule::G1)),
                    "{}: {got:?}",
                    $command
                );
            }
        )*
    };
}

g0_or_g1! {
    // (a) a prefix name built by brace expansion or matched by a zsh pattern
    round3_a_read_into_a_brace_built_name:
        r#"IFS= read -r HO{M,}E < f; sh -c "ls $HOME/x""#;
    round3_a_export_of_a_brace_built_name:
        r#"export HO{M,}E='/tmp;npx playwright install;:'; sh -c "ls $HOME/x""#;
    round3_a_printf_v_of_a_brace_built_name:
        r#"printf -v HO{ME,'%s'} '/tmp;npx playwright install;:'; sh -c "ls $HOME/x""#;
    round3_a_zsh_print_v_of_a_brace_built_name:
        r#"print -v HO{ME,} '/tmp;npx playwright install;:'; sh -c "ls $HOME/x""#;
    round3_a_zsh_typeset_m_pattern:
        r#"typeset -m 'HO?E=/tmp;npx playwright install;:'; sh -c "ls $HOME/x""#;
    round3_a_brace_built_name_before_a_program_prefix:
        r#"export HO{M,}E=/tmp/x; "$HOME/npx" playwright install"#;
    round3_a_pattern_named_prefix_before_a_program_prefix:
        r#"typeset -m 'HO?E=/tmp/x'; "$HOME/npx" playwright install"#;
    // (b) a read value reached through a name set without an assignment word
    round3_b_a_default_value_expansion_in_a_read_loop:
        "while read -r line; do : ${n:=line}; echo $(( n )); done <<< 'a[$(npx playwright install)]'";
    round3_b_a_for_name_in_a_read_loop:
        "while read -r line; do for n in line; do echo $(( n )); done; done <<< 'a[$(npx playwright install)]'";
    // (c) zsh positional setters, and zsh extendedglob characters in a direct call word
    round3_c_zsh_append_to_a_positional: r#"f() { 1+=px; "$@"; }; f n playwright install"#;
    round3_c_zsh_print_v_a_positional: r#"f() { print -v 1 npx; "$@"; }; f ls playwright install"#;
    round3_c_read_into_a_positional: r#"f() { read -r 1 <<< npx; "$@"; }; f ls playwright install"#;
    round3_c_extendedglob_hash_in_a_call_word:
        r#"setopt extendedglob; f() { "$@"; }; f /usr/local/bin/np#x playwright install"#;
    round3_c_extendedglob_caret_in_a_call_word:
        r#"setopt extendedglob; f() { "$@"; }; f /usr/local/bin/np^y playwright install"#;
    round3_c_a_shell_script_argument_outside_the_charset:
        r#"sh -c '"$1" playwright install' _ /usr/local/bin/np#x"#;
}

#[test]
fn a_standard_directory_prefix_is_read_through() {
    // the program is read with the prefix in place, so its rules still apply
    assert_blocked(&[
        r#""$HOME/x/npx" playwright install"#,
        r#""${HOME}/x/npx" playwright install"#,
        r#""$PWD/npx" playwright install"#,
        r#"cd /x; "$PWD/npx" playwright install"#,
        r#""$CLAUDE_PROJECT_DIR"/node_modules/.bin/npx playwright install"#,
        r#"sh -c "$HOME/x/npx playwright install""#,
    ]);
    // the prefix is not the standard form, or the command changes it
    assert_all(
        &[
            r#"HOME=/tmp; "$HOME/x""#,
            r#"export HOME=/tmp; "$HOME/x""#,
            r#"read -r HOME < f; "$HOME/x""#,
            r#"unset HOME; "$HOME/x""#,
            r#"for HOME in /tmp; do "$HOME/x"; done"#,
            r#"(( HOME = 1 )); "$HOME/x""#,
            r#": ${TMPDIR:=/tmp}; "$TMPDIR/x""#,
            r#"declare -n h=HOME; h=/tmp; "$HOME/x""#,
            r#"eval 'HOME=/tmp'; "$HOME/x""#,
            r#"cd /x; "$PWD/x""#,
            r#"pushd /x; "$PWD/x""#,
            r#""$HOME"x/npx"#,
            r#""$HOME:h/x""#,
            r#"$HOME/x"#,
            r#""$HOME/$x""#,
            r#""$HOME/x"*"#,
            r#""$HOME/x"{a,b}"#,
            r#""$EDITOR/x""#,
            r#"set -a; . ./.env; "$HOME/x""#,
            r#"HOME=/tmp sh -c "$HOME/x""#,
            r#"sh -c "echo $x""#,
            // PWD is not read through (the cwd can hold anything an earlier call made)
            r#""$PWD"/node_modules/.bin/vitest run"#,
            r#""$PWD/x""#,
            r#"trap "rm -f $PWD/out.tmp" EXIT"#,
            // the name anywhere else, or another expansion, turns the carve-out off
            r#"echo $HOME; "$HOME/x""#,
            r#""$HOME/x" "$y""#,
            // review of #159 round 3: outside the allowlist, so no read-through (accepted over-blocks)
            r#""${HOME}/.cargo/bin/cargo" build"#,
            r#"sh -c "cd $HOME/proj && make""#,
            r#"bash -c "$HOME/.cargo/bin/cargo build""#,
            r#""$HOME/.cargo/bin/cargo" build --features 'a b'~"#,
            r#""$HOME/.cargo/bin/cargo" build # note"#,
            r#""$HOME/.cargo/bin/cargo" build <<< x"#,
            r#""$HOME/.cargo/bin/cargo" build \
                --release"#,
        ],
        Verdict::TooComplex,
    );
}

#[test]
fn a_positional_program_outside_the_allowlist_is_too_complex() {
    // review of #159 round 3: a body with a number word or a program that may set a parameter, or a call
    // word outside `[A-Za-z0-9_./:=,+@%-]`, is too complex (accepted over-blocks)
    assert_all(
        &[
            r#"retry() { for i in 1 2 3; do "$@" && return; sleep 1; done; }; retry curl -fsS localhost:3000"#,
            r#"f() { echo 1; "$@"; }; f make test"#,
            r#"f() { local x; "$@"; }; f make test"#,
            r#"f() { command "$@"; }; f make test"#,
            r#"f() { "$@"; }; f make 'a b'"#,
            r#"f() { "$@"; }; f make ~/x"#,
            r#"sh -c '"$1" test' _ 'm a'"#,
        ],
        Verdict::TooComplex,
    );
}

#[test]
fn a_read_loop_with_any_arithmetic_is_too_complex() {
    // review of #159 round 3: no tracking of what a read value reached, so a read loop that only counts is
    // too complex as well (an accepted over-block)
    assert_all(
        &[
            "while read -r l; do n=$((n+1)); done < f; echo $n",
            "i=0; while read -r l; do i=$((i + 1)); a[i]=1; done < f",
            "while read -r l; do (( n++ )); done < f",
            "while read -r l; do n=$((n+1)); done < f",
            "while read -r l; do n=$((n+l)); done < f",
            "while read -r l; do m=$l; n=$((m+1)); done < f",
            "while read -r l; do n=$((n+$1)); done < f",
            "for x in $(cat f); do n=$((x+1)); done; read y < g",
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
            // a program under a standard directory, read with the prefix in place
            r#""$HOME/.cargo/bin/cargo" build"#,
            r#""$HOME"/.cargo/bin/cargo build"#,
            r#""$CLAUDE_PLUGIN_ROOT/bin/x" --help"#,
            r#""$CLAUDE_PROJECT_DIR/scripts/test.sh""#,
            r#""$VIRTUAL_ENV/bin/python" -m pytest"#,
            r#""$TMPDIR/x/run""#,
            r#"cd src && "$HOME/.cargo/bin/cargo" build"#,
            r#""$HOME/.cargo/bin/cargo" test -p anthology-guard --features 'a,b' 2>&1 | tail -n 5"#,
            // everyday commands
            "cargo test -p anthology-guard",
            "git status && git diff --stat",
            "npm test",
            "npm run build",
            "docker run --rm -v \"$PWD\":/w -w /w rust:1 cargo test",
            r#"for f in src/*.rs; do rustfmt --check "$f"; done"#,
            "make -j$(nproc)",
            r#"eval "$(ssh-agent -s)""#,
            r#"f() { "$@"; }; f make test; f cargo build"#,
            r#"sh -c '"$1" --version' _ git"#,
        ],
        Verdict::Allow,
    );
}

#[test]
fn a_double_quoted_script_expansion_suggests_single_quotes() {
    let hook = |command: &str| {
        let raw = format!(
            r#"{{"tool_name":"Bash","tool_input":{{"command":"{}"}}}}"#,
            command.replace('\\', "\\\\").replace('"', "\\\"")
        );
        anthology_guard::decide(raw.as_bytes(), HOME, false)
    };
    for command in [r#"x=1; sh -c "echo $x""#, r#"x=1; eval "echo $x""#] {
        let out = hook(command);
        assert_eq!(out.code, 2, "{command}");
        assert!(out.stderr.contains("(G0)"), "{}", out.stderr);
        assert!(out.stderr.contains("single quotes"), "{}", out.stderr);
    }
    // not for another G0, nor for a script that is already in single quotes
    for command in [
        "x=npx; $x playwright install",
        r#"echo 'env-cmd -f x eval "$x"' | sh"#,
    ] {
        let out = hook(command);
        assert!(out.stderr.contains("(G0)"), "{}", out.stderr);
        assert!(!out.stderr.contains("single quotes"), "{}", out.stderr);
    }
}
