//! The lexer (SH1): behaviour guard_rules.py's tests rely on, the "stays cheap" bounds at lexer level, the
//! backtrack limit, stack use and a no-panic property. Exact parity with Python is the difftest's job
//! (`shell.segments`); the expectations here were read from the Python oracle.

use std::time::{Duration, Instant};

use anthology_shell::{
    Lexed, MAX_DEPTH, Redirect, SHELLS, ShellMode, prog_name, script_starts, segments, shell_parse,
};
use proptest::prelude::*;

fn toks(lexed: &Lexed) -> Vec<Vec<&str>> {
    lexed
        .segments
        .iter()
        .map(|s| s.tokens.iter().map(String::as_str).collect())
        .collect()
}

/// Each segment's group as an ordinal by first appearance.
fn group_ordinals(lexed: &Lexed) -> Vec<usize> {
    let mut seen = Vec::new();
    lexed
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

fn texts_of(lexed: &Lexed, segment: usize) -> Vec<&str> {
    let id = lexed.segments[segment].group;
    let group = lexed
        .groups
        .iter()
        .find(|g| g.id == id)
        .expect("group listed");
    group.texts.iter().map(String::as_str).collect()
}

fn args(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| w.to_string()).collect()
}

fn redir(op: &str, target: &str) -> Redirect {
    Redirect {
        op: op.to_string(),
        target: target.to_string(),
    }
}

#[test]
fn operators_split_segments_and_pipes_share_a_group() {
    let lexed = segments("a | b && c; d\ne || f |& g", 0);
    assert_eq!(
        toks(&lexed),
        [["a"], ["b"], ["c"], ["d"], ["e"], ["f"], ["g"]]
    );
    assert_eq!(group_ordinals(&lexed), [0, 0, 1, 2, 3, 4, 4]);
    assert!(!lexed.lenient && !lexed.incomplete);
}

#[test]
fn heredoc_bodies_feed_their_pipeline() {
    let lexed = segments("cat <<EOF | sh\nnpx x\nEOF\nls", 0);
    assert_eq!(toks(&lexed), [vec!["cat"], vec!["sh"], vec!["ls"]]);
    assert_eq!(group_ordinals(&lexed), [0, 0, 1]);
    assert_eq!(texts_of(&lexed, 1), ["npx x\n"]);
}

#[test]
fn an_unquoted_heredoc_body_runs_its_substitutions() {
    let quoted = segments("cat <<'EOF'\n$(npx x)\nEOF", 0);
    assert_eq!(toks(&quoted), [["cat"]]);
    assert_eq!(texts_of(&quoted, 0), ["$(npx x)\n"]);
    let unquoted = segments("cat <<EOF\n$(npx x)\nEOF", 0);
    assert_eq!(toks(&unquoted), [vec!["cat"], vec!["npx", "x"]]);
}

#[test]
fn a_here_string_is_group_text() {
    let lexed = segments("bash <<< 'npx x'", 0);
    assert_eq!(toks(&lexed), [["bash"]]);
    assert_eq!(texts_of(&lexed, 0), ["npx x"]);
}

#[test]
fn only_unquoted_ascii_fd_numbers_are_not_words() {
    // fix-wave H: an fd is `isascii() and isdigit()` and unquoted (bash reads `²>g` and `"2">h` as words)
    let lexed = segments("a 2>f \u{b2}>g \"2\">h 2>&1 &>i", 0);
    assert_eq!(toks(&lexed), [["a", "\u{b2}", "2"]]);
    assert_eq!(
        lexed.segments[0].redirs,
        [
            redir(">", "f"),
            redir(">", "g"),
            redir(">", "h"),
            redir(">&", "1"),
            redir("&>", "i")
        ]
    );
}

#[test]
fn a_quoted_fd_word_is_a_word_and_the_heredoc_stays_unquoted() {
    // fix-wave H: `"2"` is a word, and only the delimiter word decides whether the body is quoted
    let lexed = segments("\"2\"<<EOF\n$(npx playwright install)\nEOF", 0);
    assert_eq!(
        toks(&lexed),
        [vec!["2"], vec!["npx", "playwright", "install"]]
    );
    let plain = segments("2<<EOF\n$(npx playwright install)\nEOF", 0);
    assert_eq!(toks(&plain), [["npx", "playwright", "install"]]);
}

#[test]
fn process_substitution_is_one_word_of_its_command() {
    // the body is a command of its own; the command keeps every word, the substitution as one word
    let lexed = segments("diff <(ls a) b", 0);
    assert_eq!(
        toks(&lexed),
        [vec!["ls", "a"], vec!["diff", "/dev/fd/63", "b"]]
    );
    let lexed = segments("npm --userconfig <(echo) i puppeteer", 0);
    assert_eq!(
        toks(&lexed),
        [
            vec!["echo"],
            vec!["npm", "--userconfig", "/dev/fd/63", "i", "puppeteer"]
        ]
    );
}

#[test]
fn comments_and_line_continuations() {
    assert_eq!(
        toks(&segments("ls # it's (\nnpx x", 0)),
        [vec!["ls"], vec!["npx", "x"]]
    );
    assert_eq!(
        toks(&segments("a\\\nb \"c\\\nd\"\r\ne", 0)),
        [vec!["ab", "cd"], vec!["e"]]
    );
}

#[test]
fn an_array_with_a_comment_does_not_hide_later_commands() {
    let lexed = segments(
        "deps=(\n  playwright  # it's the e2e runner\n)\nnpx playwright install",
        0,
    );
    assert_eq!(
        toks(&lexed),
        [
            vec!["echo", "playwright"],
            vec!["deps=(\n  playwright  # it's the e2e runner\n)"],
            vec!["npx", "playwright", "install"],
        ]
    );
}

#[test]
fn quotes_escapes_and_parameter_braces() {
    let lexed = segments("echo $'it\\'s' \"${x#\\\"}\" ${y}z", 0);
    assert_eq!(toks(&lexed), [["echo", "it's", "${x#\\\"}", "${y}z"]]);
    let backticks = segments("x=`a \\`b\\``", 0);
    // fix-wave H: the backtick text is unescaped and lexed again, so the inner backticks run too
    assert_eq!(
        toks(&backticks),
        [vec!["a", "`b`"], vec!["b"], vec!["a", ""], vec!["x="]]
    );
}

#[test]
fn nesting_stops_at_max_depth() {
    assert_eq!(MAX_DEPTH, 12);
    assert_eq!(
        toks(&segments("echo $(a $(b))", MAX_DEPTH - 1)),
        [vec!["a", ""], vec!["echo", ""]]
    );
    assert_eq!(
        toks(&segments("echo $(a $(b))", MAX_DEPTH)),
        [vec!["echo", ""]]
    );
}

#[test]
fn unbalanced_quotes_fall_back_to_the_lenient_split() {
    let lexed = segments("echo don't; bash -c -e 'npx playwright install'", 0);
    assert!(lexed.lenient);
    assert_eq!(
        toks(&lexed),
        [
            vec!["echo", "don", "t"],
            vec!["echo", "dont"],
            vec!["bash", "-c", "-e", "npx", "playwright", "install"],
            vec!["npx", "playwright", "install"],
        ]
    );
    assert_eq!(group_ordinals(&lexed), [0, 1, 2, 3]);
}

#[test]
fn shell_option_parsing() {
    let p = shell_parse("bash", &args(&["-euo", "pipefail", "-c", "x"]), 0);
    assert_eq!(
        (p.mode, p.start, p.scripts),
        (ShellMode::Command, 3, vec![(3, "x".to_string())])
    );
    let p = shell_parse("bash", &args(&["-s", "--", "x"]), 0);
    assert_eq!((p.mode, p.start), (ShellMode::Stdin, 2));
    let p = shell_parse("bash", &args(&["--rcfile", "x", "script.sh"]), 0);
    assert_eq!((p.mode, p.start), (ShellMode::File, 2));
    let p = shell_parse("bash", &args(&[]), 0);
    assert_eq!((p.mode, p.start), (ShellMode::Stdin, 0));
}

#[test]
fn fish_option_parsing_fails_closed() {
    let p = shell_parse("fish", &args(&["--comm=ls", "-icX", "--unknown", "f"]), 0);
    assert_eq!(p.mode, ShellMode::Command);
    assert_eq!(p.start, 3);
    assert_eq!(p.scripts, [(0, "ls".to_string()), (1, "X".to_string())]);
    // a `-c` after the script file is still found
    let p = shell_parse("fish", &args(&["--private", "f.fish", "-c", "ls"]), 0);
    assert_eq!((p.mode, p.start), (ShellMode::File, 1));
    assert_eq!(p.scripts, [(3, "ls".to_string())]);
    // past an unknown long option the next word may be its value: stdin, not a file
    let p = shell_parse("fish", &args(&["--unknown", "f.fish"]), 0);
    assert_eq!((p.mode, p.start), (ShellMode::Stdin, 1));
}

#[test]
fn script_starts_and_program_names() {
    let words = args(&[
        "docker", "run", "img", "bash", "-euo", "pipefail", "-c", "X", "npx", "-c", "Y",
    ]);
    assert_eq!(
        script_starts(&words),
        [(7, "X".to_string()), (10, "Y".to_string())]
    );
    // U+212A KELVIN SIGN lowercases to `k`, as in Python
    assert_eq!(
        script_starts(&args(&["\u{212a}sh", "-c", "ls"])),
        [(2, "ls".to_string())]
    );
    assert_eq!(prog_name("/usr/bin/BASH"), "bash");
    assert_eq!(prog_name("@Scope/Pkg"), "@scope/pkg");
    assert_eq!(prog_name("dir/"), "dir");
    assert_eq!(prog_name("/"), "/");
    assert_eq!(prog_name("\u{130}"), "i\u{307}");
    assert!(SHELLS.contains(&"fish"));
}

fn assert_quick(what: &str, bound: Duration, f: impl FnOnce()) {
    let start = Instant::now();
    f();
    let took = start.elapsed();
    assert!(took < bound, "{what}: {took:?} (bound {bound:?})");
}

#[test]
fn commented_array_stays_cheap() {
    let command = format!(
        "arr=(\n{})\nnpx playwright install",
        "  x # it's (\n".repeat(10000)
    );
    assert_quick("commented array", Duration::from_millis(500), || {
        let lexed = segments(&command, 0);
        assert_eq!(
            toks(&lexed).last().map(Vec::as_slice),
            Some(&["npx", "playwright", "install"][..])
        );
    });
}

#[test]
fn unterminated_heredocs_stay_cheap() {
    for (name, command) in [
        (
            "nested substitution",
            format!("{}{}", "\"$(".repeat(12), "cat <<X\n".repeat(2000)),
        ),
        ("lexer", "cat <<X\n".repeat(2000)),
        (
            "nested array",
            format!("{}{}", "a=(".repeat(12), "cat <<X\n".repeat(2000)),
        ),
    ] {
        assert_quick(name, Duration::from_secs(1), || {
            segments(&command, 0);
        });
    }
}

#[test]
fn many_heredoc_operators_stay_linear() {
    let command = "cat <<'aaaaaaaaaaaaaaaaaaaaaaaa' ".repeat(20000);
    assert_quick("heredoc operators", Duration::from_millis(500), || {
        segments(&command, 0);
    });
    let command = format!("$({}", "<<".repeat(50000));
    assert_quick(
        "heredoc operators in $(",
        Duration::from_millis(500),
        || {
            segments(&command, 0);
        },
    );
}

#[test]
fn the_lenient_fish_rescan_is_bounded_and_fails_closed() {
    // Python's `_script_starts` rescans every later word per `fish` word, and the lenient lexer copies a
    // segment per start: cubic. Rust stops at its work budget and sets `incomplete`.
    let small = format!("' {}", "fish -cX ".repeat(20));
    let lexed = segments(&small, 0);
    assert!(lexed.lenient && !lexed.incomplete);
    let big = format!("' {}", "fish -cX ".repeat(2000));
    assert_quick("fish rescan", Duration::from_secs(2), || {
        let lexed = segments(&big, 0);
        // over the budget: no segments, incomplete (Python: `_TooComplex`)
        assert!(lexed.incomplete);
        let tokens: usize = lexed.segments.iter().map(|s| s.tokens.len()).sum();
        assert!(tokens <= 4_000_000, "{tokens} tokens");
    });
    assert_quick("plain fish words", Duration::from_secs(2), || {
        let lexed = segments(&format!("' {}", "fish ".repeat(80_000)), 0);
        assert!(lexed.incomplete);
    });
    // long words are charged by their length: 190 scripts of 2,000 bytes each would copy gigabytes
    let long = format!("' {}", format!("fish -c{} ", "X".repeat(2000)).repeat(190));
    assert_quick("long fish words", Duration::from_secs(2), || {
        let lexed = segments(&long, 0);
        assert!(lexed.incomplete);
        let bytes: usize = lexed
            .segments
            .iter()
            .flat_map(|s| &s.tokens)
            .map(String::len)
            .sum();
        assert!(bytes <= 16_000_000, "{bytes} bytes");
    });
}

#[test]
fn a_repeated_unterminated_heredoc_is_searched_once() {
    let text = format!("$(cat {}\nls)", "<<A ".repeat(20_000));
    assert_quick("repeated delimiter", Duration::from_millis(500), || {
        segments(&text, 0);
    });
}

#[test]
fn worst_case_nesting_fits_a_small_stack() {
    let mut texts = vec![
        format!("{}ls{}", "$(".repeat(40), ")".repeat(40)),
        format!("{}ls{}", "\"$(".repeat(40), ")\"".repeat(40)),
        format!("{}ls", "`".repeat(40)),
        format!("{}ls", "a=(<(".repeat(40)),
    ];
    let mut heredoc = String::from("ls");
    for _ in 0..20 {
        heredoc = format!("cat <<EOF\n$({heredoc})\nEOF\n");
    }
    texts.push(heredoc);
    let handle = std::thread::Builder::new()
        .stack_size(512 * 1024)
        .spawn(move || {
            for text in &texts {
                segments(text, 0);
            }
        })
        .expect("spawn");
    assert!(handle.join().is_ok());
}

/// Each segment's group is listed, and every listed group is referenced, once, ascending.
fn assert_well_formed(lexed: &Lexed) {
    for seg in &lexed.segments {
        assert!(lexed.groups.iter().any(|g| g.id == seg.group));
    }
    for pair in lexed.groups.windows(2) {
        assert!(pair[0].id.0 < pair[1].id.0);
    }
    for group in &lexed.groups {
        assert!(lexed.segments.iter().any(|s| s.group == group.id));
    }
}

fn metachar_text() -> impl Strategy<Value = String> {
    let atoms = prop::sample::select(vec![
        "'",
        "\"",
        "\\",
        "`",
        "$",
        "(",
        ")",
        "{",
        "}",
        "<",
        ">",
        "|",
        "&",
        ";",
        "#",
        "\n",
        "\r",
        " ",
        "=",
        "a",
        "2",
        "EOF",
        "<<",
        "<<-",
        "<<<",
        "$(",
        "${",
        "$'",
        "a=(",
        "\u{e9}",
        "\u{1f600}",
        "\u{b2}",
        "\u{a0}",
        "\u{85}",
        "\0",
        "-c",
        "bash",
        "fish",
    ]);
    prop::collection::vec(atoms, 0..60).prop_map(|parts| parts.concat())
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 2000,
        rng_seed: proptest::test_runner::RngSeed::Fixed(0x5348_3121),
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn segments_never_panics_on_any_text(text in any::<String>(), depth in 0usize..16) {
        assert_well_formed(&segments(&text, depth));
    }

    #[test]
    fn segments_never_panics_on_metachar_text(text in metachar_text(), depth in 0usize..16) {
        assert_well_formed(&segments(&text, depth));
    }

    #[test]
    fn shell_parsers_never_panic(
        words in prop::collection::vec(
            prop::sample::select(vec![
                "-c", "-", "--", "-euo", "--comm", "--command=x", "-icX", "bash", "fish", "x", "",
                "\u{e9}", "-\u{e9}c", "--\u{e9}", "+O",
            ]),
            0..10,
        ),
        i in 0usize..12,
    ) {
        let words: Vec<String> = words.iter().map(|w| w.to_string()).collect();
        let i = i.min(words.len());
        for prog in ["bash", "fish"] {
            let parsed = shell_parse(prog, &words, i);
            prop_assert!(parsed.start <= words.len());
        }
        script_starts(&words);
    }
}
