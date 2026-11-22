//! SH-FW review, part C (lexer): finding I-2. A newline right after a `|` or `|&` (with any heredoc
//! bodies taken there, and comments or blank lines between) does not end the pipeline, so the pipe's
//! literal stdin text still reaches the shell, interpreter or xargs on the next line. bash 5.2 and 3.2 run
//! every "runs" row below; the Python reference misses them (Rust stricter, applied to the oracle too).
//!
//! `g1` is guard_rules.py's `_g1` cut down to playwright, the only program these payloads reach.

use anthology_shell::{Analysis, Entry, analyze, segments};

const HOME: &str = "/home/tester";

const PLAYWRIGHT_SUBCOMMANDS: &[&str] = &["install", "install-deps", "test"];

fn first_arg(args: &[String]) -> Option<&str> {
    args.iter()
        .map(String::as_str)
        .find(|a| !a.starts_with('-'))
}

/// guard_rules.py `_g1` for playwright.
fn g1(e: &Entry) -> bool {
    !e.exempt
        && e.prog == "playwright"
        && first_arg(&e.args).is_some_and(|s| PLAYWRIGHT_SUBCOMMANDS.contains(&s))
}

/// An entry or an alternate reading of `a` is a G1 install.
fn installs(a: &Analysis) -> bool {
    a.entries.iter().chain(&a.alternates).any(g1)
}

fn summary(a: &Analysis) -> String {
    let show = |e: &Entry| format!("{} {:?}", e.prog, e.args);
    format!(
        "entries {:?}, alternates {:?}, incomplete {}",
        a.entries.iter().map(show).collect::<Vec<_>>(),
        a.alternates.iter().map(show).collect::<Vec<_>>(),
        a.incomplete
    )
}

/// Each command runs a browser install in bash: the analysis must show it (complete, not by a budget stop).
#[track_caller]
fn assert_runs(commands: &[&str]) {
    let missed: Vec<String> = commands
        .iter()
        .filter_map(|command| {
            let a = analyze(command, HOME);
            (a.incomplete || !installs(&a)).then(|| format!("{command:?}: {}", summary(&a)))
        })
        .collect();
    assert!(missed.is_empty(), "missed installs:\n{}", missed.join("\n"));
}

/// No command is a browser install (negative controls): no entry and no alternate reading is G1.
#[track_caller]
fn assert_clear(commands: &[&str]) {
    let wrong: Vec<String> = commands
        .iter()
        .filter_map(|command| {
            let a = analyze(command, HOME);
            (a.incomplete || installs(&a)).then(|| format!("{command:?}: {}", summary(&a)))
        })
        .collect();
    assert!(wrong.is_empty(), "false installs:\n{}", wrong.join("\n"));
}

/// The group of each segment, by first appearance.
fn group_shape(text: &str) -> Vec<usize> {
    let mut seen = Vec::new();
    segments(text, 0)
        .segments
        .iter()
        .map(|s| match seen.iter().position(|g| *g == s.group) {
            Some(k) => k,
            None => {
                seen.push(s.group);
                seen.len() - 1
            }
        })
        .collect()
}

#[test]
fn a_newline_after_a_pipe_continues_the_pipeline() {
    assert_runs(&[
        // the review's three payloads
        "echo npx playwright install |\nsh",
        "cat <<'EOF' |\nnpx playwright install\nEOF\nsh",
        "echo playwright install |\nxargs npx",
        // `|&`, blank lines, comments, indentation, backslash-newline
        "echo npx playwright install |&\nsh",
        "echo npx playwright install |\n\n\nsh",
        "echo npx playwright install | # feed it\n  # still feeding\n\tsh",
        "echo npx playwright install |\\\n\nsh",
        // printf, here-string, an unquoted heredoc, two heredocs on the line
        "printf 'npx playwright install\\n' |\nbash",
        "cat <<< 'npx playwright install' |\nsh",
        "cat <<EOF |\nnpx playwright install\nEOF\nsh -s",
        "cat <<A <<B |\nx\nA\nnpx playwright install\nB\nsh",
        // after `&&`, and inside a substitution
        "true && echo npx playwright install |\nsh",
        "x=$(echo npx playwright install |\nsh)",
        // a subshell right after the `|` reads the pipe too
        "echo npx playwright install | ( sh )",
        "echo npx playwright install |\n(\n  sh\n)",
    ]);
}

#[test]
fn an_interpreter_after_the_newline_gets_the_text_as_code() {
    for command in [
        "echo 'import os' |\npython3",
        "cat <<'EOF' |\nimport os\nEOF\n\npython3 -",
    ] {
        let a = analyze(command, HOME);
        let python = a.entries.iter().find(|e| e.prog == "python3");
        assert!(
            python.is_some_and(|e| e.code.iter().any(|c| c.contains("import os"))),
            "{command:?}: {:?}",
            a.entries
        );
    }
}

#[test]
fn the_pipeline_after_the_newline_is_one_group() {
    assert_eq!(group_shape("echo a |\nsh"), vec![0, 0]);
    assert_eq!(group_shape("echo a |&\n\n# c\nsh"), vec![0, 0]);
    assert_eq!(group_shape("cat <<'EOF' |\nbody\nEOF\nsh"), vec![0, 0]);
    // the newline still ends a pipeline that has a command after its last `|`
    assert_eq!(group_shape("echo a | cat\nsh"), vec![0, 0, 1]);
    assert_eq!(group_shape("echo a ||\nsh"), vec![0, 1]);
    assert_eq!(group_shape("echo a |\nsh\nls"), vec![0, 0, 1]);
}

#[test]
fn everyday_multiline_pipelines_stay_clear() {
    assert_clear(&[
        "echo npx playwright install\nsh",
        "echo npx playwright install ||\nsh",
        "echo npx playwright install | cat\nsh",
        "echo npx playwright install | tee log\nbash ./build.sh",
        "git log --oneline |\n  head -20",
        "ps aux |\ngrep node |\nwc -l",
        "cat <<'EOF' |\nnpx playwright install\nEOF\nwc -l",
        "printf 'a\\nb\\n' |\nsort |\nuniq -c",
        "echo playwright install |\nxargs echo",
    ]);
}

#[test]
fn everyday_multiline_pipelines_are_complete_and_split_into_commands() {
    for command in ["ps aux |\ngrep node |\nwc -l", "git status |\n\n  head"] {
        let a = analyze(command, HOME);
        assert!(!a.incomplete, "{command:?}: {}", summary(&a));
        assert_eq!(a.entries.len(), command.matches('|').count() + 1);
    }
}
