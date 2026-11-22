//! SH-F2: three command-checking gaps, each bash-verified (bash 5.2) to install, and the everyday forms of the
//! same shapes that must stay allowed.
//! 1. xargs or parallel fed a process substitution (`xargs npx < <(echo X)`, `xargs -a <(echo X) npx`): the
//!    substitution's literal output feeds the layer as a pipe would (G1).
//! 2. Stdin read into a shell through a variable (`echo X | while read l; do sh -c "$l"; done`): the reader
//!    counts as a shell reading stdin, so a fed loop is too complex (G0).
//! 3. A default value in an argument (`npm ${x:-i} puppeteer`): the default is one more reading (G1).

#[path = "support/gd.rs"]
mod gd;

use anthology_guard::{Rule, Verdict, check};

use gd::{HOME, bash};

fn verdict(command: &str) -> Verdict {
    let (tool, input) = bash(command);
    check(&tool, &input, HOME)
}

#[test]
fn xargs_fed_a_process_substitution_installs() {
    for command in [
        "xargs npx < <(echo playwright install)",
        "xargs -a <(echo playwright install) npx",
        "xargs --arg-file <(echo playwright install) npx",
        "xargs --arg-file=<(echo playwright install) npx",
        "xargs -a<(echo playwright install) npx",
        "xargs npm < <(printf 'i\\npuppeteer\\n')",
        "parallel npx < <(echo playwright install)",
    ] {
        assert_eq!(verdict(command), Verdict::Block(Rule::G1), "{command}");
    }
}

#[test]
fn stdin_read_into_a_shell_through_a_variable_is_too_complex() {
    for command in [
        r#"echo npx playwright install | while read l; do sh -c "$l"; done"#,
        r#"echo npx playwright install | while read -r l; do eval "$l"; done"#,
        r#"echo npx playwright install | while IFS= read -r l; do bash -c "$l"; done"#,
        "echo npx playwright install | while read l; do $l; done",
        r#"printf 'npx playwright install\n' | while read -r cmd; do eval "${cmd}"; done"#,
        r#"echo npx playwright install | for l in $(cat); do eval "$l"; done"#,
        r#"echo "npx playwright install" | { read l; eval "$l"; }"#,
        r#"echo "npx playwright install" | (read l; eval "$l")"#,
        r#"echo "npx playwright install" | { l=$(cat); sh -c "$l"; }"#,
        r#"while read l; do sh -c "$l"; done <<< "npx playwright install""#,
        "while read l; do sh -c \"$l\"; done <<EOF\nnpx playwright install\nEOF",
        r#"while read l; do sh -c "$l"; done < <(echo npx playwright install)"#,
        r#"(read l; eval "$l") <<< "npx playwright install""#,
        r#"( sh ) <<< "npx playwright install""#,
        r#"{ sh; } <<< "npx playwright install""#,
    ] {
        assert_eq!(verdict(command), Verdict::TooComplex, "{command}");
    }
    // `xargs -I{} sh -c {}` was already read: the fed line is the script
    assert_eq!(
        verdict("echo npx playwright install | xargs -I{} sh -c {}"),
        Verdict::Block(Rule::G1)
    );
}

#[test]
fn a_default_value_in_an_argument_is_read() {
    for command in [
        "npm ${x:-i} puppeteer",
        "npm ${x-i} puppeteer",
        "x=1; npm ${x:+i} puppeteer",
        "x=1; npm ${x+i} puppeteer",
        "npm ${x:=i} puppeteer",
        "npm ${x=i} puppeteer",
        r#"npm "${x:-i}" puppeteer"#,
        r#"npm ${x:-"i"} puppeteer"#,
        "npm i ${x:-puppeteer}",
        "npm ${x:-i puppeteer}",
        "npx playwright ${x:-install}",
        "npm ${x:-i}${y:-} puppeteer",
        "npm i${x:-} puppeteer",
    ] {
        assert_eq!(verdict(command), Verdict::Block(Rule::G1), "{command}");
    }
    // the reading is checked by the other rules too
    assert_eq!(
        verdict("cat ${F:-~/.config/google-chrome/Default/Cookies}"),
        Verdict::Block(Rule::G3)
    );
}

#[test]
fn everyday_loops_xargs_and_defaults_stay_allowed() {
    for command in [
        r#"while read l; do echo "$l"; done < f"#,
        r#"while read -r line; do echo "$line"; done < <(git ls-files)"#,
        r#"git ls-files | while read -r f; do wc -l "$f"; done"#,
        r#"git branch --merged | while read b; do git branch -d "$b"; done"#,
        r#"echo a b | { read x y; echo "$x"; }"#,
        r#"for f in $(git ls-files); do wc -l "$f"; done"#,
        r#"while read -r a b; do echo "$a=$b"; done <<< "$(env)""#,
        "find . -name '*.txt' | xargs -I{} cp {} dst/",
        "ls | xargs -I {} cp {} /tmp/",
        "xargs -a <(git ls-files) wc -l",
        "xargs wc -l < <(git ls-files)",
        "xargs -I{} cp {} dst/ < <(echo a.txt)",
        "cp ${SRC:-src}/a.txt ${DST:-dst}/",
        "mkdir -p ${BUILD_DIR:-build}",
        "npm run ${SCRIPT:-build}",
        "ls ${XDG_CONFIG_HOME:-${HOME}/.config}",
        r#"echo "${1:-no args given}""#,
        "git log -n ${N:-10} --oneline",
        "npm install ${PKG:-lodash}",
    ] {
        assert_eq!(verdict(command), Verdict::Allow, "{command}");
    }
}

// ------------------------------------------------------------------ the #103 review (review-sh-f2.md)

#[test]
fn review_i1_xargs_fed_through_a_compound_a_function_cat_or_exec() {
    for command in [
        "{ xargs npx; } < <(echo playwright install)",
        "( xargs npx ) < <(echo playwright install)",
        "while true; do xargs npx; break; done < <(echo playwright install)",
        "f() { xargs npx; }; f < <(echo playwright install)",
        "cat <(echo playwright install) | xargs npx",
        "cat < <(echo playwright install) | xargs npx",
        "exec < <(echo playwright install); xargs npx",
        "parallel npx :::: <(echo playwright install)",
    ] {
        assert_eq!(verdict(command), Verdict::Block(Rule::G1), "{command}");
    }
    for command in [
        r#"{ xargs npx; } <<< "playwright install""#,
        r#"( xargs npx ) <<< "playwright install""#,
        r#"bash -c 'xargs npx' <<< "playwright install""#,
        r#"exec <<< "playwright install"; xargs npx"#,
        r#"f() { xargs npx; }; f <<< "playwright install""#,
    ] {
        assert_eq!(verdict(command), Verdict::TooComplex, "{command}");
    }
}

#[test]
fn review_i2_reading_through_positionals_copies_namerefs_order_and_functions() {
    for command in [
        r#"echo npx playwright install | while read l; do sh -c 'eval "$1"' _ "$l"; done"#,
        r#"echo npx playwright install | while read l; do sh -c '$1' _ "$l"; done"#,
        r#"echo npx playwright install | while read l; do bash -c '"$@"' _ $l; done"#,
        r#"echo npx playwright install | { read l; x=$l; eval "$x"; }"#,
        r#"echo npx playwright install | { read l; n=l; eval "${!n}"; }"#,
        r#"echo npx playwright install | { read l; declare -n r=l; eval "$r"; }"#,
        r#"echo npx playwright install | while :; do eval "$l"; read l || break; done"#,
        r#"f() { eval "$l"; }; echo npx playwright install | { read l; f; }"#,
        r#"f() { read l; eval "$l"; }; f <<< "npx playwright install""#,
        r#"f() { read l; eval "$l"; }; echo npx playwright install | f"#,
        r#"echo npx playwright install | while read l; do sh <(echo "$l"); done"#,
        r#"echo npx playwright install | while read l; do . <(echo "$l"); done"#,
        "f() { sh; }; echo npx playwright install | f",
    ] {
        assert_eq!(verdict(command), Verdict::TooComplex, "{command}");
    }
}

#[test]
fn review_i3_subscripted_and_ansi_c_defaults() {
    for command in [
        "npm ${a[0]:-i} puppeteer",
        "npm ${a[@]:-i} puppeteer",
        "${a[0]:-npx} playwright install",
        "npm ${x:-$'i'} puppeteer",
        r"npm ${x:-$'\x69'} puppeteer",
        r#"npm ${x:-$"i"} puppeteer"#,
    ] {
        assert_eq!(verdict(command), Verdict::Block(Rule::G1), "{command}");
    }
}

#[test]
fn review_m2_a_broken_later_line_keeps_the_here_string_closures() {
    for command in [
        "( sh ) <<< \"npx playwright install\"\necho \"a",
        "{ sh; } <<< \"npx playwright install\"\necho \"a",
        "while read l; do sh -c \"$l\"; done <<< \"npx playwright install\"\necho \"a",
        "while read l; do sh -c \"$l\"; done <<EOF\nnpx playwright install\nEOF\necho \"a",
    ] {
        assert_eq!(verdict(command), Verdict::TooComplex, "{command:?}");
    }
    // a heredoc of a plain command, then a broken line, is read as before
    assert_eq!(
        verdict("git commit -m \"$(cat <<'EOF'\nfix: it's done\nEOF\n)\"\necho \"a"),
        Verdict::Allow
    );
}

#[test]
fn review_m3_the_ruled_over_blocks_and_the_depth_cap() {
    // Ruling (gap 2): a read variable inside a `-c` script, or run as the program, is too complex
    for command in [
        r#"git ls-files | while read f; do bash -c "wc -l $f"; done"#,
        r#"git ls-files | while read f; do "$f" --check; done"#,
    ] {
        assert_eq!(verdict(command), Verdict::TooComplex, "{command}");
    }
    // ... but a positional parameter the script only reads as an argument is not
    for command in [
        r#"git ls-files | while read -r f; do sh -c 'wc -l "$1"' _ "$f"; done"#,
        r#"find . -name '*.md' | while read f; do bash -c 'echo "$0"' "$f"; done"#,
        r#"git ls-files | while read f; do x=$f; echo "$x"; done"#,
        r#"build() { make "$@"; }; echo y | build"#,
        "parallel -j4 convert {} {.}.png :::: files.txt",
    ] {
        assert_eq!(verdict(command), Verdict::Allow, "{command}");
    }
    // Ruling (gap 3): 8 nested default values are read, 9 are too complex
    let nested = |n: usize| format!("npm {}i{} puppeteer", "${a:-".repeat(n), "}".repeat(n));
    assert_eq!(verdict(&nested(8)), Verdict::Block(Rule::G1));
    assert_eq!(verdict(&nested(9)), Verdict::TooComplex);
}

// ------------------------------------------------------------------ SH-F4 (review-sh-f3.md)

#[test]
fn sh_f4_positional_parameters_and_function_arguments_are_put_in_place() {
    // a positional parameter or a function argument given as literal text is read in place (I-1)
    for command in [
        r#"sh -c '"$@"' _ npx playwright install"#,
        r#"sh -c 'eval "$1"' _ "npx playwright install""#,
        "bash -c '$1 $2 $3' _ npx playwright install",
        r#"sh -c 'npx "$1" install' _ playwright"#,
        r#"echo npx playwright install | xargs -I{} sh -c 'eval "$1"' _ {}"#,
        r#"echo npx playwright install | xargs sh -c '"$@"' _"#,
        r#"f() { npx "$@"; }; f playwright install"#,
        r#"f() { eval "$1"; }; f "npx playwright install""#,
        r#"function f { npx "$@"; }; f playwright install"#,
    ] {
        assert_eq!(verdict(command), Verdict::Block(Rule::G1), "{command}");
    }
    // ... and a read variable passed to a function reaches its body (I-1)
    for command in [
        r#"f() { eval "$1"; }; echo npx playwright install | while read l; do f "$l"; done"#,
        r#"f() { "$@"; }; echo npx playwright install | while read l; do f $l; done"#,
        r#"f() { sh -c "$1"; }; echo npx playwright install | while read l; do f "$l"; done"#,
        "f() {\n  local cmd=$1\n  eval \"$cmd\"\n}; echo npx playwright install | while read l; do f \"$l\"; done",
    ] {
        assert_eq!(verdict(command), Verdict::TooComplex, "{command:?}");
    }
    // everyday positional and function forms stay allowed
    for command in [
        r#"sh -c 'echo "$1"' _ hello"#,
        r#"find . -name '*.md' -exec sh -c 'wc -l "$1"' _ {} \;"#,
        r#"bash -c 'for f in "$@"; do wc -l "$f"; done' _ a.txt b.txt"#,
        r#"greet() { echo "hello $1"; }; greet world"#,
        r#"run() { "$@"; }; run make test"#,
        r#"retry() { for i in 1 2 3; do "$@" && return; sleep 1; done; }; retry curl -fsS localhost:3000"#,
        r#"f() { cd "$1" && git pull; }; f repo"#,
    ] {
        assert_eq!(verdict(command), Verdict::Allow, "{command}");
    }
}

#[test]
fn review_sh_f4_i1_an_unquoted_positional_in_a_function_body_is_split() {
    for command in [
        r#"f() { $1; }; f "npx playwright install""#,
        r#"f() { npx $1; }; f "playwright install""#,
    ] {
        assert_eq!(verdict(command), Verdict::Block(Rule::G1), "{command}");
    }
    // the whole word is still read too, so a quoted path with a space keeps its rule
    assert_eq!(
        verdict(
            r#"f() { cat "$1"; }; f "~/Library/Application Support/Google/Chrome/Default/Cookies""#
        ),
        Verdict::Block(Rule::G3)
    );
    for command in [
        r#"gh_pr() { gh pr create --title "$1" --body "$2"; }; gh_pr fix "npx playwright install was flaky""#,
        r#"f() { echo "$1"; }; f "npx playwright install""#,
        r#"f() { git commit -m "$1"; }; f "add npm i puppeteer to the docs""#,
    ] {
        assert_eq!(verdict(command), Verdict::Allow, "{command}");
    }
}

#[test]
fn sh_f4_a_piped_function_that_wraps_xargs_is_too_complex() {
    // Ruling (review-sh-f3 M-2): stricter than the inline form, which is read (`find ... | xargs rm -f`)
    for command in [
        "rmall() { xargs rm -f; }; find . -name '*.tmp' | rmall",
        "f() { xargs -n1 echo; }; ls | f",
    ] {
        assert_eq!(verdict(command), Verdict::TooComplex, "{command}");
    }
    assert_eq!(
        verdict("find . -name '*.tmp' | xargs rm -f"),
        Verdict::Allow
    );
}
