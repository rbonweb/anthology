//! Fix-wave H in Rust (lane SH, part H): heredoc delimiters and bodies, `$'...'`, arithmetic, `${...}` and
//! backticks in the lexer.
//!
//! Every "blocked" row is a payload bash runs, from `task-FW-H-report.md` (its tables and port rules 1-16),
//! `review-fw-h.md` (rounds 0 and 1) and the fix-wave H diff of `tests/test_guard_rules.py`. Where the vendored
//! Python reference (`port/difftest/oracle/guard_rules_fw.py`) allows one, Rust still has to block it (D-8:
//! R1-M1). Every "clear" row is a negative control from the same files: bash data, or a command an agent writes
//! every day. `fixtures/fw_h_base_blocked.json` is the reference's `base_blocked_bash_runs.json`: 566 commands
//! the pre-fix-wave-H guard blocked and bash 5.2 runs.
//!
//! `analyze()` does not decide G1; the guard does. `g1` below is guard_rules.py's `_g1` cut down to the
//! playwright family, the only install these payloads spell. A row is blocked when the analysis is complete and
//! an entry or alternate reading is that install; it is flagged when the analysis is incomplete (the guard's
//! "too complex", G0) or an entry is opaque.

use std::time::{Duration, Instant};

use anthology_shell::{Analysis, Entry, Opaque, analyze};

const HOME: &str = "/home/tester";

// ------------------------------------------------------------------ G1 (playwright), as the guard reads an entry

const PLAYWRIGHT: &[&str] = &[
    "playwright",
    "playwright-core",
    "@playwright/test",
    "@playwright/cli",
];
const PLAYWRIGHT_SUBCOMMANDS: &[&str] = &[
    "install",
    "install-deps",
    "test",
    "codegen",
    "open",
    "cr",
    "chromium",
    "ff",
    "firefox",
    "wk",
    "webkit",
    "screenshot",
    "pdf",
];

/// guard_rules.py `_g1` for the playwright programs.
fn g1(e: &Entry) -> bool {
    !e.exempt
        && PLAYWRIGHT.contains(&e.prog.as_str())
        && e.args
            .iter()
            .find(|a| !a.starts_with('-'))
            .is_some_and(|s| PLAYWRIGHT_SUBCOMMANDS.contains(&s.as_str()))
}

fn installs(a: &Analysis) -> bool {
    a.entries.iter().chain(&a.alternates).any(g1)
}

fn flagged(a: &Analysis) -> bool {
    a.incomplete || a.entries.iter().any(|e| !e.opaque.is_empty())
}

fn summary(a: &Analysis) -> String {
    let show = |e: &Entry| format!("{} {:?} {:?}", e.prog, e.args, e.opaque);
    format!(
        "entries {:?}, alternates {:?}, incomplete {}",
        a.entries.iter().map(show).collect::<Vec<_>>(),
        a.alternates.iter().map(show).collect::<Vec<_>>(),
        a.incomplete
    )
}

/// Every command satisfies `ok`; the failure lists every row that does not, not only the first.
#[track_caller]
fn assert_all(what: &str, commands: &[&str], ok: fn(&Analysis) -> bool) {
    let bad: Vec<String> = commands
        .iter()
        .filter_map(|c| {
            let a = analyze(c, HOME);
            (!ok(&a)).then(|| format!("{c:?}: {}", summary(&a)))
        })
        .collect();
    assert!(
        bad.is_empty(),
        "{what} ({}):\n{}",
        bad.len(),
        bad.join("\n")
    );
}

/// bash runs the install: the analysis shows it, complete.
#[track_caller]
fn assert_blocked(commands: &[&str]) {
    assert_all("not blocked", commands, |a| !a.incomplete && installs(a));
}

/// bash keeps the install as data: complete, and no entry or alternate is an install.
#[track_caller]
fn assert_clear(commands: &[&str]) {
    assert_all("false block", commands, |a| !a.incomplete && !installs(a));
}

/// An everyday command: clear, and no entry is marked opaque either.
#[track_caller]
fn assert_plain(commands: &[&str]) {
    assert_all("not plain", commands, |a| {
        !a.incomplete && !installs(a) && a.entries.iter().all(|e| e.opaque.is_empty())
    });
}

/// `unit` repeated and cut to `bytes` (test_guard_rules.py `rep`).
fn rep(unit: &str, bytes: usize) -> String {
    let mut text = unit.repeat(bytes / unit.len() + 1);
    text.truncate(bytes);
    text
}

// test diff HEREDOC_RUNS, report rule 1: the word before `<<` is an ordinary word;
// `\<<EOF` is the word `<` and a `<EOF` redirection
const WORD_BEFORE_OPERATOR: &[&str] = &[
    "cat - \"2\"<<EOF\n$(npx playwright install)\nEOF",
    "\"2\"<<EOF\n$(npx playwright install)\nEOF",
    "cat '2'<<EOF\n$(npx playwright install)\nEOF",
    "cat \\2<<EOF\n$(npx playwright install)\nEOF",
    "cat $'2'<<EOF\n$(npx playwright install)\nEOF",
    "cat \"0\"<<-EOF\n\t$(npx playwright install)\n\tEOF",
    "cat \"2\"<< EOF\n$(npx playwright install)\nEOF",
    "cat \"2\"<<EOF\n`npx playwright install`\nEOF",
    "echo \"$(cat \"2\"<<EOF\n$(npx playwright install)\nEOF\n)\"",
    "echo $(cat \"2\"<<EOF\n$(npx playwright install)\nEOF\n)",
    "cat x\"y\"<<EOF\n$(npx playwright install)\nEOF",
    "cat 2>&1<<EOF\n$(npx playwright install)\nEOF",
];

// SH-FW final F-8(b): `\<<EOF` is not a heredoc, so the next line is a command that starts with a substitution:
// bash runs its output as the program, which is too complex (it was complete with the install in view)
const F8_PROGRAM_LINE: &[&str] = &[
    "cat \\<<EOF\n$(npx playwright install)\nEOF",
    "cat <<$(echo 'x')\nx\n$(echo x)\nnpx playwright install\n$(echo 'x')",
    "cat <<`echo 'x'`\nx\n`echo x`\nnpx playwright install\n`echo 'x'`",
    "x=$(cat <<''\n$(npx playwright install)\n\n)",
];

/// SH-FW final F-8: a substitution or a `${x:-...}` with text around it at program position is too complex, so
/// the analysis is incomplete (the guard blocks it whatever it holds).
#[track_caller]
fn assert_too_complex(commands: &[&str]) {
    assert_all("not too complex", commands, |a| a.incomplete);
}

// test diff HEREDOC_RUNS, report rule 1: a quoted target or here-string word before `<<` is not an fd
const NOT_AN_FD_NUMBER: &[&str] = &[
    "cat 2>&\"1\"<<EOF\n$(npx playwright install)\nEOF",
    "cat 2>&'1'<<EOF\n$(npx playwright install)\nEOF",
    "cat >\"1\"<<EOF\n$(npx playwright install)\nEOF",
    "cat <<<\"2\"<<EOF\n$(npx playwright install)\nEOF",
    "cat - \"2\"<<<x <<EOF\n$(npx playwright install)\nEOF",
];

// test diff HEREDOC_RUNS, report rule 2: a backslash-newline inside `<<` / `<<-` keeps the operator
const OPERATOR_CONTINUATIONS: &[&str] = &[
    "cat <\\\n<'EOF'\nit's\nEOF\nnpx playwright install\n'",
    "cat <\\\n<EOF\n$(npx playwright install)\nEOF",
    "cat <<\\\n-EOF\n\t$(npx playwright install)\n\tEOF",
    "cat << \\\n EOF\n$(npx playwright install)\nEOF",
];

// test diff HEREDOC_RUNS, report rule 3: each heredoc on a line has its own quoting
const OWN_QUOTING: &[&str] = &[
    "cat <<A \"2\"<<B\nx\nA\n$(npx playwright install)\nB",
    "cat - \"2\"<<'A' <<B\nx\nA\n$(npx playwright install)\nB",
];

// test diff HEREDOC_RUNS, report rules 3 and 5: inside `$(...)` a partly quoted delimiter is the whole
// word; `<<<` is not `<<`
const IN_SUBSTITUTION: &[&str] = &[
    "echo \"$(cat <<\"E\"OF\nx\nEOF\n)\"; npx playwright install; echo \"\nE\n)\"",
    "echo \"$(cat <<E\\OF\nx\nEOF\n)\"; npx playwright install; echo \"\nE\n)\"",
    "echo \"$(cat <<<EOF\n)\"; npx playwright install; echo \"\nEOF\n)\"",
];

// test diff HEREDOC_RUNS, report rule 6: the terminator is the exact line (`<<-` strips tabs only)
const EXACT_TERMINATOR: &[&str] = &[
    "cat <<EOF\nEOF \nit's\nEOF\nnpx playwright install\n'",
    "cat <<EOF\n EOF\nit's\nEOF\nnpx playwright install\n'",
    "cat <<EOF\nEOF\r\nit's\nEOF\nnpx playwright install\n'",
    "cat <<-EOF\n  EOF\nit's\nEOF\nnpx playwright install\n'",
];

// test diff HEREDOC_RUNS, report rule 7: an unquoted body joins backslash-newline lines
const BODY_JOINS: &[&str] = &[
    "cat <<EOF\nEO\\\nF\nnpx playwright install\nEOF",
    "cat <<EOF\nx\\\nEOF\nit's\nEOF\nnpx playwright install\n'",
    "cat <<-EOF\n\t\\\n\tEOF\nnpx playwright install\nEOF",
    "cat <<EOF\n$\\\n(npx playwright install)\nEOF",
];

// test diff HEREDOC_RUNS, report rules 4 and 10: any word is a delimiter; `$'E\x4fF'` ends at `EOF`
const ANY_WORD: &[&str] = &[
    "cat <<.\n'$(npx playwright install)'\n.",
    "cat <<123\n'$(npx playwright install)'\n123",
    "cat <<$X\n'$(npx playwright install)'\n$X",
    "cat <<$(echo X)\n'$(npx playwright install)'\n$(echo X)",
    "cat <<$'E\\x4fF'\nx\nEOF\nnpx playwright install\nEx4fF",
];

// test diff HEREDOC_RUNS, report rule 11: in `((...))` and `$((...))`, `<<` is a shift
const ARITHMETIC: &[&str] = &[
    "((x=1<<2))\nnpx playwright install\n2",
    "((x=1<<y))\nnpx playwright install\ny",
    "for ((i=0; i<<1; i++)); do :; done\nnpx playwright install\n1",
    "echo \"$( ((x=1<<y))\n)\"; npx playwright install; echo \"\ny\n)\"",
    "x=$(\n((x=1<<y))\nnpx playwright install\ny\n)",
    "echo $((1<<y))\nnpx playwright install\ny",
    "echo \"$((1<<y\n))\"; npx playwright install; echo \"\ny\n)\"",
    "x=$((1<<y\n)); npx playwright install\ny",
    "(( $(npx playwright install) 1 ))",
    "((npx playwright install) )",
    "( (echo a) <<EOF\n$(npx playwright install)\nEOF\n)",
];

// test diff HEREDOC_RUNS, report rule 9: an unterminated body runs to the end of the input
const UNTERMINATED: &[&str] = &[
    "cat <<EOF\n'$(npx playwright install)'",
    "cat <<EOF\n'`npx playwright install`'",
];

// test diff ANSI_C_RUNS, report rule 10: `$'...'` as bash 5 decodes it
const ANSI_C: &[&str] = &[
    "$'\\x6epx' playwright install",
    "$'\\156px' playwright install",
    "$'\\u006epx' playwright install",
    "$'\\U0000006e'px playwright install",
    "npx $'pl\\x61ywright' install",
    "$'np\\0zz'x playwright install",
    "npx playwright $'\\x69nstall'",
];

// review-fw-h C1 (test diff ROUND1_RUNS): `$'...'` ends at the first `'` not taken by a backslash pair
const C1: &[&str] = &[
    "$'\\c'; npx playwright install\n'",
    "$'\\c\\\\'; npx playwright install\n'",
    "echo $'a\\c'; npx playwright install\n'",
    "echo $'\\c'\nnpx playwright install\n'",
    "x=$'\\c\\\\' npx playwright install\n'",
];

// review-fw-h C2 (test diff ROUND1_RUNS): a `$((` that does not close with `))` is `$( (...) )`
const C2: &[&str] = &[
    "echo $((cat <<EOF\n'$(npx playwright install)'\nEOF\n) )",
    "x=$((cat <<EOF\n'$(npx playwright install)'\nEOF\n) )",
    "echo \"$((cat <<EOF\n'$(npx playwright install)'\nEOF\n) )\"",
];

// review-fw-h C3 (test diff ROUND1_RUNS): inside `$(...)` a delimiter line holding `)` ends the body
const C3: &[&str] = &[
    "x=$(cat <<EOF\nEOF)\nnpx playwright install\nEOF\n)",
    "echo \"$(cat <<EOF\nit's\nEOF)\"; npx playwright install; echo \"\nEOF\n)\"",
    "x=$(cat <<'EOF'\nEOF)\nnpx playwright install\nEOF\n)",
    "x=$(cat <<-EOF\n\tEOF)\nnpx playwright install\nEOF\n)",
    "x=$(cat <<EOF\nEOF) ; npx playwright install\nEOF\n)",
    "x=$(cat <<EOF\nEOFx)\nnpx playwright install\nEOF\n)",
];

// review-fw-h C4 (test diff ROUND1_RUNS): arithmetic expands like a double-quoted string
const C4: &[&str] = &[
    "(( '$(npx playwright install)' ))",
    "echo $(( '$(npx playwright install)' ))",
    "echo \"$(( '$(npx playwright install)' ))\"",
    "echo $[ '$(npx playwright install)' ]",
];

// review-fw-h C5 (test diff ROUND1_RUNS): `${...}` and comments inside `$(...)` hold no heredoc
const C5: &[&str] = &[
    "echo \"$(echo ${y#<<E}\n)\"; npx playwright install; echo \"\nE}\n)\"",
    "echo \"$(echo x # <<E\n)\"; npx playwright install; echo \"\nE\n)\"",
];

// review-fw-h C6 (test diff ROUND1_RUNS): a `${...}` delimiter ends at its own `}`
const C6: &[&str] = &["cat <<${x:-'}'}EOF\nit's\n${x:-'}'}EOF\nnpx playwright install\n'"];

// review-fw-h C7 (test diff ROUND1_RUNS): inside backticks, an escaped backtick is a nested one
const C7: &[&str] = &[
    "cat <<EOF\n`echo \\`npx playwright install\\``\nEOF",
    "echo `echo \\`npx playwright install\\``",
    "echo \"`echo \\`npx playwright install\\``\"",
];

// review-fw-h C8 (test diff ROUND1_RUNS): substitutions inside `${...}` run
const C8: &[&str] = &[
    "echo ${x:-$(npx playwright install)}",
    "echo \"${x:-'$(npx playwright install)'}\"",
    "cat <<EOF\n${x:-$(npx playwright install)}\nEOF",
];

// task-FW-H-report round 1 (test diff ROUND1_RUNS): found while fixing
const FOUND_WHILE_FIXING: &[&str] = &[
    "echo \"$(echo $'a\\'b # )')\"; npx playwright install",
    "echo $'\\c\\'' ; npx playwright install\n'",
    "echo $(( 1 + $(( '$(npx playwright install)' )) ))",
];

// review-fw-h I4 (test diff ROUND1_BASH32_RUNS): bash 3.2 reads a `$'...'` delimiter its own way
const I4: &[&str] = &[
    "cat <<$'\\u0041'\nit's\n\\u0041\nnpx playwright install\n'",
    "cat <<$'\\u0041'\nx\n\\u0041\nx=$(cat <<B\n'$(npx playwright install)'\n)",
    "cat <<$'\\c?'\nit's\n\u{1f}\nnpx playwright install\n'",
];

// review-fw-h R1-C1 (test diff ROUND2_RUNS): a `(` alone on a delimiter line does not end the body
const R1_C1: &[&str] = &[
    "x=$(cat <<EOF\nEOF (\n'$(npx playwright install)'\nEOF\n)",
    "x=$(cat <<EOF\nEOF(\n'$(npx playwright install)'\nEOF\n)",
    "x=$(cat <<EOF\nEOF'('\n'$(npx playwright install)'\nEOF\n)",
    "x=$(cat <<-EOF\n\tEOF (\n'$(npx playwright install)'\nEOF\n)",
    "echo \"$(cat <<EOF\nEOF'('\n'$(npx playwright install)'\nEOF\n)\"",
    "echo \"$(cat <<EOF\nEOF (\n'$(npx playwright install)'\nEOF\n)\"",
    "x=$(cat <<EOF\nEOF (\nEOF\n)\nnpx playwright install",
];

// review-fw-h R1-C1 (test diff ROUND2_BASH32_RUNS): bash 3.2 runs these, bash 5.2 keeps them data
const R1_C1_BASH32: &[&str] = &[
    "cat <<${a:-'x'}\nx\n${a:-x}\nnpx playwright install\n${a:-'x'}",
    "cat <<$(echo \"x\")\nit's\n$(echo x)\nnpx playwright install\n'",
    "cat <<$(echo \\x)\nit's\n$(echo x)\nnpx playwright install\n'",
    "x=$(cat <<EOF\nEOF # )\n'$(npx playwright install)'\nEOF\n)",
    "x=$(cat <<EOF\nEOF (x)\n'$(npx playwright install)'\nEOF\n)",
    "x=$(cat <<'EOF'\nEOF # )\nit's\nEOF\nnpx playwright install\n')",
];

// task-FW-H-report tables, rows the test diff does not repeat
const REPORT_RUNS: &[&str] = &[
    "cat 2<<EOF\n$(npx playwright install)\nEOF",
    "cat <<${X:-Y}\n'$(npx playwright install)'\n${X:-Y}",
    "cat <<E#F\n'$(npx playwright install)'\nE#F",
    "cat <<EOF#c\n'$(npx playwright install)'\nEOF#c",
    "cat <<`echo X`\n'$(npx playwright install)'\n`echo X`",
    "cat <<E\\\nOF\n'$(npx playwright install)'\nEOF",
    "cat <<EOF;true\n'$(npx playwright install)'\nEOF",
    "cat <<EOF|cat\n'$(npx playwright install)'\nEOF",
    "cat <<-EOF\n\tEOF\t\nit's\nEOF\nnpx playwright install\n'",
    "echo \"$(cat <<EOF\nx\n)\"; npx playwright install",
    "$'npx' playwright install",
];

// test diff HEREDOC_DATA: bash keeps these as data
const HEREDOC_DATA: &[&str] = &[
    "cat \"2\"<<'A'\n$(npx playwright install)\nA",
    "cat <<\"E\"OF\n$(npx playwright install)\nEOF",
    "cat <<E\\OF\n$(npx playwright install)\nEOF",
    "cat <<$'EOF'\n$(npx playwright install)\nEOF",
    "cat <<$\"EOF\"\n$(npx playwright install)\nEOF",
    "cat <<-\\EOF\n$(npx playwright install)\n\tEOF",
    "cat <<'a b'\n$(npx playwright install)\na b",
    "cat <<''\n$(npx playwright install)\n\necho ok",
    "cat <<\"$(echo X)\"\n$(npx playwright install)\n$(echo X)",
    "cat <<'EOF'\nEO\\\nF\nnpx playwright install\nEOF",
    "cat <<EOF\nx\\\nEOF\nnpx playwright install\nEOF",
    "cat <<-EOF\n\tEO\\\n\tF\nnpx playwright install\nEOF",
    "cat <<EOF\nEOF\\\\\nnpx playwright install\nEOF",
    "echo \"$(cat <<\"E\"OF\nnpx playwright install\nE\nEOF\n)\"",
];

// test diff ANSI_C_DATA
const ANSI_C_DATA: &[&str] = &[
    "echo $'\\x6epx playwright install'",
    "$'n\\0px' playwright install",
    "$'\\x6e\\x70x\\x' playwright install",
];

// test diff ROUND1_DATA
const ROUND1_DATA: &[&str] = &[
    "echo ${x:-'}' ; npx playwright install ; '}",
    "cat <<'EOF'\n${x:-$(npx playwright install)}\nEOF",
    "echo \"$(cat <<'EOF'\nEOF is (here)\nEOF\n)\"",
];

// test diff EVERYDAY_HEREDOCS: the heredoc forms an agent writes every day
const EVERYDAY_HEREDOCS: &[&str] = &[
    "git commit -m \"$(cat <<'EOF'\nfeat: parse heredocs (round 1)\n\nIt's done: see `docs/x.md`, $HOME and $(date).\nnpx playwright install is not run here.\n\nCo-Authored-By: Claude <noreply@anthropic.com>\nEOF\n)\"",
    "git commit -m \"$(cat <<'EOF'\nfix: EOF handling\n\nEOFs and EOF lines are fine.\nEOF\n)\"",
    "gh pr create --draft --title \"fix: x\" --body \"$(cat <<'EOF'\n## Summary\n- a (b)\n- c's d\n\n```\nnpx playwright install\n```\n\nGenerated with [Claude Code](https://claude.com/claude-code)\nEOF\n)\"",
    "cat > notes.md <<'EOF'\n# Notes\n\nRun `npx playwright install` only in Docker.\nEOF",
    "cat <<EOF > out.txt\nHello $USER, today is $(date +%F) (UTC).\nEOF",
    "if true; then\n\tcat <<-EOF\n\t\tindented $(pwd)\n\tEOF\nfi",
    "bash <<'EOF'\nset -euo pipefail\nls -la\nEOF",
    "git commit -F - <<'EOF'\nfix: EOF handling (parser)\n\n((x)) and $((1<<2)) are fine.\nEOF",
    "for ((i = 0; i < 3; i++)); do echo \"$i\"; done",
    "echo \"${HOME:-/tmp}\" \"${PATH//:/ }\" ${#PATH}",
    "echo `date` \"`pwd`\"",
];

// review-fw-h M2: `&`, backslash-newline, `>` is the `&>` operator (report rule 16)
const M2: &[&str] = &["npx playwright install &\\\n>/dev/null"];

// review-fw-h R1-M1: the lenient fallback (an unbalanced quote later on) still decodes `$'...'` (report rule
// 15). bash 5.2 runs all three; the reference allows them: Rust stricter (D-8).
const R1_M1: &[&str] = &[
    "$'\\x6epx' playwright install\n'",
    "npx $'pl\\x61ywright' install\necho '",
    "$'\\x6epx' playwright install\ncat <<'",
];

// review-fw-h R1-M2: bash ends the quoted body at `EOF (see below)` and runs the backticked line
const R1_M2: &[&str] = &[
    "git commit -m \"$(cat <<'EOF'\nfix: x\n\nEOF (see below)\n`npx playwright install`\nEOF\n)\"",
];

#[test]
fn heredoc_delimiter_and_body_follow_bash() {
    for table in [
        WORD_BEFORE_OPERATOR,
        NOT_AN_FD_NUMBER,
        OPERATOR_CONTINUATIONS,
        OWN_QUOTING,
        IN_SUBSTITUTION,
        EXACT_TERMINATOR,
        BODY_JOINS,
        ANY_WORD,
        ARITHMETIC,
        UNTERMINATED,
        REPORT_RUNS,
    ] {
        assert_blocked(table);
    }
    assert_too_complex(F8_PROGRAM_LINE);
    assert_clear(HEREDOC_DATA);
}

#[test]
fn ansi_c_strings_are_decoded_as_bash_5_does() {
    assert_blocked(ANSI_C);
    assert_clear(ANSI_C_DATA);
    // report rule 10 and its table: the decoded program word (the reference's entries agree)
    let decoded = [
        ("$'\\cA' x", "\u{1}"),
        ("$'\\c?' x", "\u{7f}"),
        ("$'\\e' x", "\u{1b}"),
        ("$'\\E' x", "\u{1b}"),
        ("$'\\z' x", "\\z"),
        ("$'\\8' x", "\\8"),
        ("$'\\1567' x", "n7"),
        ("$'\\x6e6' x", "n6"),
        ("$'\\c\\\\x' x", "\u{1c}x"),
        ("$'\\U110000' x", "\u{fffd}"),
        ("$'a\\c' x", "a\\c"),
        ("$'\\u' x", "\\u"),
        ("$'\\x' x", "\\x"),
    ];
    let wrong: Vec<String> = decoded
        .iter()
        .filter_map(|&(command, prog)| {
            let a = analyze(command, HOME);
            let got = a.entries.first().map(|e| e.prog.as_str());
            (got != Some(prog)).then(|| format!("{command:?}: {got:?}, want {prog:?}"))
        })
        .collect();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn review_round_0_findings_are_closed() {
    for table in [C1, C2, C3, C4, C5, C7, C8, FOUND_WHILE_FIXING, I4, M2] {
        assert_blocked(table);
    }
    // F-8(a): `${x:-'}'}EOF` at program position has text after its expansion
    assert_too_complex(C6);
    assert_clear(ROUND1_DATA);
}

#[test]
fn review_r1_c1_paren_lines_are_read_every_way() {
    assert_blocked(R1_C1);
    assert_blocked(R1_C1_BASH32);
    // F-8(b): the backticked line is a command at program position in the reading that ends the body early
    assert_too_complex(R1_M2);
}

#[test]
fn lenient_fallback_decodes_ansi_c_strings() {
    // review-fw-h R1-M1, Rust stricter (D-8): the reference allows all three (its lenient split keeps
    // `$'\x6epx'` undecoded); bash 5.2 runs them. Rust's lenient split also reads each closed `$'...'` decoded.
    assert_blocked(R1_M1);
}

#[test]
fn everyday_heredocs_stay_clear() {
    assert_plain(&["git commit -m \"$(cat <<'EOF'\nmsg\nEOF\n)\""]);
    assert_plain(EVERYDAY_HEREDOCS);
    // the interpreter's own mark (`InterpreterCode`) is expected; nothing else may be
    let a = analyze(
        "python3 - <<'EOF'\nimport sys\nprint(\"it's\", sys.argv)\nEOF",
        HOME,
    );
    assert!(
        !a.incomplete
            && !installs(&a)
            && a.entries
                .iter()
                .all(|e| e.opaque.iter().all(|o| *o == Opaque::InterpreterCode)),
        "{}",
        summary(&a)
    );
}

#[test]
fn everyday_arithmetic_has_no_opaque_mark() {
    // test diff EVERYDAY_HEREDOCS. The entries match the reference (`1 2`, `x + 1`, `* 2`, ...); a `*` program
    // word from a `$((...))` is multiplication, so it carries no DynamicCommandWord mark.
    assert_plain(&["x=$((1 << 2)); y=$(( (x + 1) * 2 )); echo \"$x $y\""]);
}

/// The 100 KB commands test_guard_rules.py keeps under the work budget (`work_budget_leaves_ordinary_...`).
fn ordinary_100_kb() -> Vec<(&'static str, String)> {
    let text = rep(
        "Some text (with parens), it's $HOME, `code` and $(date).\n",
        100_000,
    );
    vec![
        (
            "commit -m",
            format!("git commit -m \"$(cat <<'EOF'\nfeat: x\n\n{text}EOF\n)\""),
        ),
        (
            "commit -F",
            format!("git commit -F - <<'EOF'\nfeat: x\n\n{text}EOF"),
        ),
        (
            "EOF ( lines",
            format!(
                "git commit -m \"$(cat <<'EOF'\nfeat: x\n\n{}\nEOF\n)\"",
                rep(&format!("EOF (see below\n{}", &text[..500]), 100_000)
            ),
        ),
        (
            "script",
            rep(
                "if [ -f x ]; then\n  cat <<EOF > f\n  $(pwd) ${HOME:-x}\nEOF\nfi\nfor ((i=0;i<3;i++)); do echo \"$i\"; done\n",
                100_000,
            ),
        ),
    ]
}

#[test]
fn ordinary_100_kb_commands_stay_clear() {
    for (name, command) in ordinary_100_kb() {
        let a = analyze(&command, HOME);
        assert!(!a.incomplete && !installs(&a), "{name}: {}", summary(&a));
    }
}

#[test]
fn base_blocked_fixture_stays_blocked() {
    let corpus = json_strings(include_str!("fixtures/fw_h_base_blocked.json"));
    assert_eq!(corpus.len(), 566);
    let rows: Vec<&str> = corpus.iter().map(String::as_str).collect();
    assert_all("neither blocked nor flagged", &rows, |a| {
        installs(a) || flagged(a)
    });
}

#[test]
fn what_the_reference_calls_too_complex_is_flagged() {
    // test diff `test_work_budget_blocks_what_it_cannot_check`: the reference says G0 for these 100 KB inputs
    // and for the lenient fallback over 12 000 nested shells
    let mut commands: Vec<String> = [
        "$(cat <<EOF\n'",
        "((x=$(cat <<EOF\n",
        "\"$(cat <<X\n",
        "${$(",
    ]
    .iter()
    .map(|unit| rep(unit, 100_000))
    .collect();
    commands.push("bash -c ".repeat(12_000) + "'");
    let rows: Vec<&str> = commands.iter().map(String::as_str).collect();
    assert_all("not flagged", &rows, flagged);
}

#[test]
fn odd_inputs_do_not_panic() {
    // test diff `test_check_never_raises_on_odd_inputs`, the fix-wave H additions
    let odd = [
        "<<$(".repeat(2000),
        "cat <<$'\\".into(),
        "cat <<\"".into(),
        "<<-".into(),
        "cat <<`".into(),
        "cat <<\"$(\"".into(),
        "cat <<E\\".into(),
        "$'\\c".into(),
        "$'\\x".into(),
        "$'\\u'".into(),
        "$'\\U110000' x".into(),
        "$'\\777'".into(),
        "cat <<''\n".into(),
        "cat <<EOF\n".to_string() + &"a\\\n".repeat(2000),
        "$(cat <<EOF\n".to_string() + &"x\\\n".repeat(2000) + ")",
    ];
    for command in &odd {
        analyze(command, HOME);
    }
}

/// A JSON array of strings (the fixture), without a JSON dependency.
fn json_strings(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '"' {
            continue;
        }
        let mut s = String::new();
        loop {
            match chars.next().expect("unterminated JSON string") {
                '"' => break,
                '\\' => match chars.next().expect("bad escape") {
                    'n' => s.push('\n'),
                    't' => s.push('\t'),
                    'r' => s.push('\r'),
                    'b' => s.push('\u{8}'),
                    'f' => s.push('\u{c}'),
                    'u' => {
                        let hex: String = chars.by_ref().take(4).collect();
                        let code = u32::from_str_radix(&hex, 16).expect("bad \\u escape");
                        s.push(char::from_u32(code).expect("surrogate \\u escape"));
                    }
                    other => s.push(other),
                },
                other => s.push(other),
            }
        }
        out.push(s);
    }
    out
}

// ------------------------------------------------------------------ cost

/// A timing shape: its text at a given size in bytes.
type Build = Box<dyn Fn(usize) -> String>;

/// Every timing shape of review-fw-h (round 0 I1-I3, round 1 R1-I1 and "other new costs"), the test diff's
/// scaling and budget tests, and the report's cheapness cases, built to `bytes`.
fn shapes() -> Vec<(&'static str, Build)> {
    fn r(head: &'static str, unit: &'static str) -> Build {
        Box::new(move |n| format!("{head}{}", rep(unit, n)))
    }
    let distinct: String = (0..12_000).map(|i| format!("cat <<A{i}\n")).collect();
    let many: String = (0..200).map(|i| format!(" <<A{i}")).collect();
    vec![
        // review-fw-h I1 (and the test diff's 1 s units)
        ("<<\"$(", r("", "<<\"$(")),
        ("$(<<\"", r("", "$(<<\"")),
        ("$(cat <<\"x", r("", "$(cat <<\"x")),
        ("$(x <<'", r("", "$(x <<'")),
        ("$(<<$'", r("", "$(<<$'")),
        // review-fw-h I2 (and the test diff's scaling and budget units)
        ("\"$(cat <<X\\n", r("", "\"$(cat <<X\n")),
        ("\"$(cat <<EOF\\n\"", r("", "\"$(cat <<EOF\n\"")),
        ("$(cat <<EOF\\n'", r("", "$(cat <<EOF\n'")),
        ("((x=$(cat <<EOF\\n", r("", "((x=$(cat <<EOF\n")),
        // review-fw-h I3
        (
            "$( + distinct delimiters",
            Box::new(move |n| format!("$({}", &distinct[..n])),
        ),
        (
            "$(cat <<A0 .. <<A199 + lines",
            Box::new(move |n| format!("$(cat{many}\n{}", rep("x\n", n))),
        ),
        // review-fw-h R1-I1 (and the test diff's REVIEW_R1_I1)
        ("$( + ${", r("$(", "${")),
        ("$[", r("", "$[")),
        ("\" + $[", r("\"", "$[")),
        ("$( + $[", r("$(", "$[")),
        ("${$(", r("", "${$(")),
        ("$( + ${\"", r("$(", "${\"")),
        ("${\"}", r("", "${\"}")),
        ("$( ${ ) ", r("", "$( ${ ) ")),
        ("(( ${ )x ", r("", "(( ${ )x ")),
        (
            "$[ nested",
            Box::new(|n| format!("{}{}", "$[".repeat(n / 3), "]".repeat(n / 3))),
        ),
        // review-fw-h round 1 "other new costs"
        ("$((", r("", "$((")),
        ("$(( ((", r("", "$(( ((")),
        // the report's cheapness cases (test diff `test_unterminated_heredocs_stay_cheap`)
        ("((", r("", "((")),
        ("cat <\\\\n", r("", "cat <\\\n")),
        (
            "cat <<EOF + a\\\\n",
            Box::new(|n| format!("cat <<EOF\n{}", rep("a\\\n", n))),
        ),
        (
            "$(cat <<EOF + x\\\\n",
            Box::new(|n| format!("$(cat <<EOF\n{})", rep("x\\\n", n))),
        ),
        // the test diff's budget test: the lenient fallback
        ("bash -c ", Box::new(|n| format!("{}'", rep("bash -c ", n)))),
    ]
}

/// Best of three analyses of `text`.
fn cost(text: &str) -> Duration {
    (0..3)
        .map(|_| {
            let start = Instant::now();
            analyze(text, HOME);
            start.elapsed()
        })
        .min()
        .unwrap()
}

#[test]
fn every_review_shape_is_quick_and_linear_at_100_kb() {
    let mut slow = Vec::new();
    let mut all = shapes();
    let scaled = all.len();
    for (name, command) in ordinary_100_kb() {
        // fixed 100 KB texts: checked against the catastrophe guard only
        all.push((name, Box::new(move |_| command.clone())));
    }
    let mut slowest = (Duration::ZERO, "");
    for (i, (name, build)) in all.iter().enumerate() {
        // a shape fails only if it is slow three times over (the CI runners and a loaded machine are noisy)
        let (small_text, large_text) = (build(25_000), build(100_000));
        let mut verdict = None;
        for _ in 0..3 {
            let small = cost(&small_text);
            let large = cost(&large_text);
            // 4x input costs less than 10x (floor 0.5 s); 8 s is only a catastrophe guard
            let bad = large >= Duration::from_secs(8)
                || (i < scaled && large >= (small * 10).max(Duration::from_millis(500)));
            verdict = Some((small, large, bad));
            if !bad {
                break;
            }
        }
        let (small, large, bad) = verdict.unwrap();
        if large > slowest.0 {
            slowest = (large, name);
        }
        if bad {
            slow.push(format!("{name:?}: 25 KB {small:?}, 100 KB {large:?}"));
        }
    }
    eprintln!("slowest shape: {:?} {:?}", slowest.1, slowest.0);
    assert!(slow.is_empty(), "slow shapes:\n{}", slow.join("\n"));
}
