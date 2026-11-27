//! Three guard bypasses that were on `build` before they were reported (found while reviewing other changes, each
//! proven to run its marker in bash 5.2 or zsh 5.9), and the everyday forms of the same shapes that must stay
//! allowed.
//! 1. A program that runs another program from an option the analysis did not read: `rg --pre`/`--hostname-bin`,
//!    ugrep's (the Claude Code shell `grep`) `--filter` and `--pager`, `sort --compress-program`, and
//!    `less`'s `LESSOPEN`/`LESSCLOSE`. The value is a command now, read as `tar -I` and `fd -x` are (G1 for an
//!    install). These programs pass the value a file name too (`rg --pre sh x p.sh` runs `sh p.sh`), which the
//!    text may not show, so a value that is not a known harmless one (`cat`, a compressor) is too complex.
//! 2. zsh runs an escaped backtick inside an arithmetic subscript (`(( a[\`cmd\`] ))`); bash does not, so a
//!    backtick there, escaped or not, is a command substitution.
//! 3. An unquoted zsh glob group between the words of a command (`docker exec c (flock|x) psql ...`) expands to
//!    words the analysis never saw, so the command is too complex rather than split at the paren.

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

// ------------------------------------------------------------------ 1. options that run a program

#[test]
fn rg_pre_and_hostname_bin_run_their_value() {
    assert_all(
        &[
            "rg --pre 'npx playwright install' x f",
            "rg --pre='npx playwright install' x f",
            "rg --pre-glob '*.txt' --pre='npx playwright install' x f",
            "rg --pre='npx playwright install' --pre-glob '*.txt' x f",
            // an abbreviation: older rg and clap-built tools take any unique prefix
            "rg --pr='npx playwright install' x f",
            "rg --hostname-bin='npx playwright install' --hyperlink-format=default x f",
            "rg --hostname-bin 'npx playwright install' --hyperlink-format=default x f",
        ],
        Verdict::Block(Rule::G1),
    );
}

/// rg runs the program with each searched file as its only argument, and ugrep, sort and less pass theirs too, so
/// a value that is a bare program (`--pre sh x p.sh` runs the file `p.sh`, `--pre=playwright x install` runs
/// `playwright install`) cannot be proved harmless from the text: too complex.
#[test]
fn a_bare_program_given_to_an_option_that_runs_it_is_too_complex() {
    assert_all(
        &[
            "rg --pre sh x p.sh",
            "rg --pre=sh x p.sh",
            "rg --pre=npx x f",
            "rg --pre playwright x install",
            "rg -i --pre=playwright x install",
            "rg --pre-glob '*.sh' --pre sh x p.sh",
            "rg --pr=npx x f",
            "rg --hostname-bin=npx --hyperlink-format=default x f",
            "rg --hostname-bin npx --hyperlink-format=default x f",
            "echo 'npx playwright install' > p.sh; rg --pre sh x p.sh",
            "ugrep --filter='txt:npx' x a.txt",
            "ugrep --filter='*:pdftotext % -' x f",
            "ugrep --filter='txt:cat,pdf:pdftotext % -' x f",
            "grep --filter='txt:npx' x f",
            "ugrep --pager=npx x f",
            "sort --compress-program=npx f",
            "sort --compress=npx f",
            "sort --com=npx f",
            "sort --co npx f",
            "sort --compress-program sh big",
            "gsort --compress-program=sh big",
            "LESSOPEN='|npx %s' less f",
            "LESSOPEN='|lesspipe %s' less f",
            "LESSCLOSE='npx %s %s' less f",
            "export LESSOPEN='|npx %s'; less f",
            "sudo rg --pre=sh x p.sh",
        ],
        Verdict::TooComplex,
    );
}

#[test]
fn ugrep_filter_and_pager_run_their_value() {
    assert_all(
        &[
            "ugrep --filter='txt:npx playwright install' x f",
            "ugrep --filter 'txt:npx playwright install' x f",
            "ugrep --filter='*:npx playwright install' x f",
            "ugrep --filter='txt:cat,*:npx playwright install' x f",
            "ugrep --filter='npx playwright install' x f",
            "grep --filter='txt:npx playwright install' x f",
            "grep -r --filter='*:npx playwright install' x .",
            "ug --filter='txt:npx playwright install' x f",
            "egrep --filter='txt:npx playwright install' x f",
            "ugrep --pager='npx playwright install' x f",
            "grep --pager='npx playwright install' -r x .",
            "ug --pager='npx playwright install' x f",
        ],
        Verdict::Block(Rule::G1),
    );
}

#[test]
fn sort_compress_program_runs_its_value() {
    assert_all(
        &[
            "sort --compress-program='npx playwright install' f",
            "sort --compress-program 'npx playwright install' f",
            "sort -S 100k --compress-program='npx playwright install' big",
            "sort --compress='npx playwright install' f",
            "sort --com='npx playwright install' f",
            "gsort --compress-program='npx playwright install' f",
        ],
        Verdict::Block(Rule::G1),
    );
}

#[test]
fn lessopen_and_lessclose_run_their_value() {
    assert_all(
        &[
            "LESSOPEN='|npx playwright install %s' less f",
            "LESSOPEN='||npx playwright install %s' less f",
            "LESSOPEN='-npx playwright install %s' less f",
            "LESSOPEN='npx playwright install %s' less f",
            "LESSOPEN='|npx playwright install %s' more f",
            "LESSCLOSE='npx playwright install %s %s' less f",
            "LESSOPEN='|cat %s' LESSCLOSE='npx playwright install %s %s' less f",
            "export LESSOPEN='|npx playwright install %s'; less f",
            "env LESSOPEN='|npx playwright install %s' less f",
        ],
        Verdict::Block(Rule::G1),
    );
}

#[test]
fn the_everyday_forms_of_those_programs_stay_allowed() {
    assert_all(
        &[
            "rg x f",
            "rg --pre-glob '*.pdf' x f",
            "rg --pre=cat x f",
            "rg --hostname-bin=cat --hyperlink-format=default x f",
            "rg -n --hidden --glob '!target' 'fn main' src",
            "rg --pretty x f",
            "rg --heading --hyperlink-format=none x f",
            "ugrep --filter-magic-label=x x f",
            "ugrep --file=pats.txt f",
            "ugrep --pager=less x f",
            "ugrep --pager x f",
            "grep -rn --include='*.rs' x .",
            "grep --color=always x f",
            "sort -u f",
            "sort --compress-program=gzip big",
            "sort --compress-program=zstd -S 1M big",
            "sort -k2,2 -t, f",
            "sort --check f",
            "sort --compress-program=gzip --compress-program=zstd big",
            "LESSOPEN= less f",
            "LESSKEY=x less f",
            "less f",
        ],
        Verdict::Allow,
    );
}

// ------------------------------------------------------------------ 2. arithmetic subscripts in zsh

#[test]
fn an_escaped_backtick_in_an_arithmetic_subscript_runs_in_zsh() {
    assert_all(
        &[
            r"(( grep + path[\`npx playwright install\`] ))",
            r"typeset -a a; (( a[\`npx playwright install\`] ))",
            r"(( a[\`npx playwright install\`] = 1 ))",
            r"(( a[\`npx playwright install\`]++ ))",
            r"echo $(( a[\`npx playwright install\`] ))",
            r"x=$(( path[\`npx playwright install\`] ))",
            r"for ((i=0; i<a[\`npx playwright install\`]; i++)); do :; done",
            r"while (( a[\`npx playwright install\`] )); do :; done",
            // the unescaped form was already read
            "(( a[`npx playwright install`] = 1 ))",
            "(( grep + path[`npx playwright install`] ))",
        ],
        Verdict::Block(Rule::G1),
    );
}

#[test]
fn an_unclosed_escaped_backtick_in_arithmetic_is_too_complex() {
    assert_all(
        &[
            r"(( a[\`id] ))",
            r"echo $(( a[\`id] ))",
            r"(( a[\`id\`\`] ))",
        ],
        Verdict::TooComplex,
    );
}

#[test]
fn an_escaped_backtick_command_in_arithmetic_reads_as_the_unescaped_one() {
    for command in [
        r"(( a[\`id\`] ))",
        r"(( a[\`id\`] = 1 ))",
        r"echo $(( a[\`id\`] ))",
    ] {
        let unescaped = command.replace('\\', "");
        assert_eq!(verdict(command), verdict(&unescaped), "{command}");
    }
}

#[test]
fn everyday_arithmetic_stays_allowed() {
    assert_all(
        &[
            "(( i++ ))",
            "(( a[1] = 2 ))",
            "(( x = y * 2 ))",
            "echo $(( 1 + 2 ))",
            "x=$(( a[1] + b[2] ))",
            "for ((i=0; i<3; i++)); do echo $i; done",
            "while (( i < 3 )); do i=$((i+1)); done",
            "if (( x > 1 )); then echo big; fi",
            r"echo 'a\`b'",
            r"echo a\`b",
        ],
        Verdict::Allow,
    );
}

// ------------------------------------------------------------------ 3. zsh glob groups between words

#[test]
fn a_glob_group_between_words_is_too_complex() {
    assert_all(
        &[
            r#"docker exec c (flock|x) psql -c "select 1; npx playwright install""#,
            r#"docker exec c (flock|x) psql -c "select count(*) from t""#,
            r#"docker exec c (flock) psql -c "select count(*) from t""#,
            r#"docker exec c ( flock|x ) psql -c "select count(*) from t""#,
            r#"docker exec -i c (flock|x) psql -U u -c "select count(*) from t""#,
            "echo x (a|b)",
            "ls (a|b)",
            "ls -l (a|b) c",
            "FOO=1 ls (a|b)",
            "env A=1 ls (a|b)",
            "echo a; ls (a|b)",
            "cat f | grep x (a|b)",
            "echo $(ls (a|b))",
            "echo `ls (a|b)`",
            "ls (a|b)(.)",
        ],
        Verdict::TooComplex,
    );
}

#[test]
fn a_glob_group_before_an_install_is_still_blocked() {
    assert_none_allowed(&["docker exec c (npx|x) playwright install"]);
}

#[test]
fn parentheses_in_everyday_shell_stay_allowed() {
    assert_all(
        &[
            "(cd d && ls)",
            "echo a && (cd d && ls)",
            "ls | (cat)",
            "if (true); then (echo a); fi",
            "if ! (false); then echo a; fi",
            "while (true); do break; done",
            "until (false); do break; done",
            "for f in a b; do (cd $f; ls); done",
            "{ (echo a); }",
            "f () { echo a; }",
            "f() { echo a; }",
            "function f () { echo a; }",
            "function f { (echo a); }",
            "case x in (a|b) echo hi;; esac",
            "case x in a|b) echo hi;; esac",
            "case x in a) (echo hi);; esac",
            "[[ $x == (a|b) ]]",
            "[[ $x =~ (a|b) ]]",
            "x=(1 2); echo ${x[1]}",
            "arr+=(a b)",
            "echo $((1 + (2 * 3)))",
            "diff <(ls a) <(ls b)",
            "echo hi | tee >(cat)",
            r"find . \( -name a -o -name b \)",
            "echo 'ls (a|b)'",
            r#"echo "ls (a|b)""#,
            r"echo \(a\|b\)",
        ],
        Verdict::Allow,
    );
}

// ------------------------------------------------------------------ review round: editors, awk, config files

/// vim and ex run the `:!` lines of the file the search hands them, so an editor is no harmless value for an
/// option that feeds it files (`rg --pre`, ugrep's `--filter` and `--pager`, `sort --compress-program`).
#[test]
fn an_editor_given_to_an_option_that_runs_it_is_too_complex() {
    assert_all(
        &[
            "rg --pre=vim x f",
            "rg --pre vim x f",
            "rg --pre=vi x f",
            "rg --pre=nvim x f",
            "rg --pre=ex x f",
            "rg --pre=view x f",
            "rg --pre=emacs x f",
            "rg --pre=nano x f",
            "rg --hostname-bin=vim --hyperlink-format=default x f",
            "ugrep --filter='txt:vim' x f",
            "ugrep --filter='*:vi' x f",
            "ugrep --filter='cat,*:vim' x f",
            "grep --filter=vi x f",
            "ugrep --pager=vim x f",
            "ugrep --pager=nano x f",
            "sort --compress-program=vim big",
            "sort --compress-program=nano big",
        ],
        Verdict::TooComplex,
    );
    assert_all(
        &[
            "rg --pre=cat x f",
            "rg --pre=less x f",
            "rg --pre=more x f",
            "ugrep --pager=less x f",
            "ugrep --filter='*:cat' x f",
            "sort --compress-program=zstd big",
        ],
        Verdict::Allow,
    );
}

/// awk (and gawk, mawk, nawk) runs commands from its program text: `system(`, a pipe to or from a string, `|&`.
#[test]
fn an_awk_program_that_runs_a_command_is_too_complex() {
    assert_all(
        &[
            r#"awk 'BEGIN{system("cmd")}'"#,
            r#"gawk 'BEGIN{system("cmd")}'"#,
            r#"mawk 'BEGIN{system("cmd")}'"#,
            r#"nawk 'BEGIN{system("cmd")}'"#,
            r#"original-awk 'BEGIN{system("cmd")}'"#,
            r#"awk 'BEGIN { system ("cmd") }'"#,
            r#"awk -F: 'BEGIN{system("cmd")}' f"#,
            r#"awk -F : -v x=1 'BEGIN{system("cmd")}' f"#,
            r#"awk '{print | "cmd"}' f"#,
            r#"awk '{print $1 | "sort"}' f"#,
            r#"awk '{print > "/dev/stderr"; print $1 | "cmd"}' f"#,
            r#"awk 'BEGIN{"cmd" | getline x; print x}'"#,
            r#"gawk 'BEGIN{"cmd" |& getline x}'"#,
            r#"gawk '{print |& "cmd"}' f"#,
            r#"awk 'BEGIN{x = 1; cmd = "id"; cmd | getline y}'"#,
            r#"awk '/a/ { n++ / 2 | "cmd" }' f"#,
            r#"awk '/a|b/ { print | "cmd" }' f"#,
            "awk 'BEGIN{\n  system(\"cmd\")\n}'",
            r#"awk -e 'BEGIN{system("cmd")}'"#,
            r#"awk -e'BEGIN{system("cmd")}'"#,
            r#"gawk --source='BEGIN{system("cmd")}'"#,
            r#"gawk --source 'BEGIN{system("cmd")}'"#,
            "awk 'BEGIN{system(c)}' c=id",
            r#"gawk '@load "x"; BEGIN{}'"#,
            "awk -f p.awk f",
            "awk -fp.awk f",
            "awk --file=p.awk f",
            "gawk -E p.awk f",
            "gawk -l ext 'BEGIN{}'",
            "gawk -i lib.awk '{print}' f",
            "gawk --include=lib.awk '{print}' f",
            "sudo awk 'BEGIN{system(\"cmd\")}'",
            "cat f | awk '{print | \"cmd\"}'",
        ],
        Verdict::TooComplex,
    );
}

#[test]
fn an_awk_install_in_a_command_is_not_allowed() {
    assert_none_allowed(&[
        r#"awk 'BEGIN{system("npx playwright install")}'"#,
        r#"awk '{print | "npx playwright install"}' f"#,
    ]);
}

#[test]
fn everyday_awk_stays_allowed() {
    assert_all(
        &[
            "awk '{print $1}' f",
            "awk -F: '{print $1}' /etc/passwd",
            "awk -F'|' '{print $1}' f",
            "awk -F '|' '{print $2}' f",
            "awk -v OFS='|' '{print $1, $2}' f",
            "awk '/foo|bar/' f",
            "awk '/foo|bar/ {print $2}' f",
            "awk '$1 == \"a\" || $2 == \"b\"' f",
            "awk '$0 ~ /a|b/ {n++} END {print n}' f",
            "awk 'NR > 1 && /x|y/' f",
            "awk '{gsub(/a|b/, \"\"); print}' f",
            "awk 'BEGIN{printf \"%s|%s\\n\", 1, 2}'",
            "awk '{print \"a | b\"}' f",
            "awk '{print \"system(x)\"}' f",
            "awk '{ # a pipe | in a comment\n print }' f",
            "awk '!seen[$0]++' f",
            "awk '{ s += $1 } END { print s / NR }' f",
            "awk '{print $1 / 2}' f",
            "awk -F, '{print $1}' f g",
            "gawk -i inplace '{print}' f",
            "awk 'BEGIN { x = 1 } END { print x }' f",
            "echo a | awk '{print $1}'",
        ],
        Verdict::Allow,
    );
}

/// ripgrep's config file and ugrep's `--config`, `---FILE` and `ug` (which reads `.ugrep` from the working
/// folder and the home folder) can hold any option, `--pre` and `--filter` among them.
#[test]
fn a_config_file_for_rg_or_ugrep_is_too_complex() {
    assert_all(
        &[
            "RIPGREP_CONFIG_PATH=cfg rg x f",
            "RIPGREP_CONFIG_PATH=/tmp/cfg rg --files",
            "env RIPGREP_CONFIG_PATH=cfg rg x f",
            "export RIPGREP_CONFIG_PATH=cfg; rg x f",
            "RIPGREP_CONFIG_PATH=cfg sudo rg x f",
            "ugrep --config=cfg x f",
            "ugrep --config x f",
            "grep --config=cfg x f",
            "ugrep ---cfg x f",
            "grep ---cfg x f",
            "ug x f",
            "ug+ x f",
            "ug -r x .",
        ],
        Verdict::TooComplex,
    );
    assert_all(
        &[
            "RIPGREP_CONFIG_PATH= rg x f",
            "rg --no-config x f",
            "ugrep -r x .",
            "grep -rn x .",
            "ugrep --colour=never x f",
        ],
        Verdict::Allow,
    );
}
