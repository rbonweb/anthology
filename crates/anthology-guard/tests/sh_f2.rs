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
