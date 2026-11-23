//! Ports of tests/test_startup_budget.py (DV2): one `mod` per Python class, one test per method.
//!
//! `StartupBudgetTest` runs on `support::agents::make_repo` (tests/_agents.py). The CLI tests
//! call `main_with` with their own budget table, the `mock.patch.dict(sb.BUDGET)` seam.

mod support;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anthology_core::Env;
use anthology_core::roster::ROSTER;
use anthology_dev::startup_budget::{
    self as sb, BUDGET, Root, agent_names, agents_reading, split_agent, startup_bytes,
    startup_files, startup_paths, suggested_budget,
};
use support::agents::{make_repo, write};

struct Fixture {
    _tmp: tempfile::TempDir,
    path: PathBuf,
    root: Root,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().to_path_buf();
    make_repo(&path);
    let root = Root::new(&path, Path::new("/"));
    Fixture {
        _tmp: tmp,
        path,
        root,
    }
}

fn body_bytes(f: &Fixture, name: &str) -> u64 {
    let (_, body) = split_agent(
        &f.path
            .join("plugins/anthology/agents")
            .join(format!("{name}.md")),
    )
    .expect("agent");
    body.len() as u64
}

fn main(f: &Fixture, extra: &[&str], budget: &[(&str, u64)]) -> (i32, String) {
    let mut args = vec![OsString::from("--root"), f.path.clone().into_os_string()];
    args.extend(extra.iter().map(OsString::from));
    let out = sb::main_with(&args, &Env::from_process(), Path::new("/"), budget);
    assert!(
        out.stderr.is_empty(),
        "{:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    (out.code, String::from_utf8(out.stdout).expect("utf-8"))
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

mod startup_budget_test {
    use super::*;

    #[test]
    fn test_split_agent_reads_the_skills_list_without_yaml() {
        let f = fixture();
        let (skills, body) =
            split_agent(&f.path.join("plugins/anthology/agents/skilled.md")).expect("agent");
        assert_eq!(skills, ["anthology-superpowers:plan"]);
        assert!(body.starts_with("## Startup"));
        let (skills, _) =
            split_agent(&f.path.join("plugins/anthology/agents/reader.md")).expect("agent");
        assert!(skills.is_empty());
    }

    #[test]
    fn test_startup_files_and_paths() {
        let f = fixture();
        assert_eq!(
            startup_files(&f.root, "reader").expect("reader"),
            ["knowledge/alpha/SKILL.md", "knowledge/beta/SKILL.md"]
        );
        assert_eq!(
            startup_paths(&f.root, "skilled").expect("skilled"),
            [
                "plugins/anthology/knowledge/beta/SKILL.md",
                "plugins/anthology-superpowers/skills/plan/SKILL.md"
            ]
        );
        assert!(startup_paths(&f.root, "bare").expect("bare").is_empty());
    }

    #[test]
    fn test_startup_bytes_is_body_plus_startup_files_plus_preloads() {
        let f = fixture();
        assert_eq!(
            startup_bytes(&f.root, "reader").expect("reader"),
            body_bytes(&f, "reader") + 300
        );
        assert_eq!(
            startup_bytes(&f.root, "skilled").expect("skilled"),
            body_bytes(&f, "skilled") + 600
        );
        assert_eq!(
            startup_bytes(&f.root, "bare").expect("bare"),
            body_bytes(&f, "bare")
        );
    }

    #[test]
    fn test_agents_reading_a_target() {
        let f = fixture();
        let read = |t: &str| agents_reading(&f.root, t).expect("agents");
        assert_eq!(read("plugins/anthology/knowledge/alpha"), ["reader"]);
        assert_eq!(
            read("plugins/anthology/knowledge/beta"),
            ["reader", "skilled"]
        );
        assert_eq!(read("plugins/anthology-superpowers"), ["skilled"]);
        assert_eq!(read("plugins/anthology/knowledge"), ["reader", "skilled"]);
        assert!(read("plugins/anthology/knowledge/alph").is_empty()); // not a prefix match
        assert!(read("atl").is_empty());
        assert!(read("").is_empty());
    }

    #[test]
    fn test_agent_names_skip_names_that_are_not_plain_identifiers() {
        let f = fixture();
        write(&f.path, "plugins/anthology/agents/evil@octocat.md", "x");
        write(&f.path, "plugins/anthology/agents/a b.md", "x");
        assert_eq!(agent_names(&f.root), ["bare", "reader", "skilled"]);
    }

    #[test]
    fn test_cli_prints_size_against_budget_and_fails_when_over() {
        let f = fixture();
        let budgets = [("reader", 10_000), ("skilled", 10_000), ("bare", 1)];
        let (code, text) = main(&f, &[], &budgets);
        assert_eq!(code, 1);
        assert!(text.contains(&format!(
            "reader: {} bytes of 10000 budget (ok)",
            body_bytes(&f, "reader") + 300
        )));
        assert!(text.contains("bare: "));
        assert!(text.contains(&format!(
            "OVER; if intended set BUDGET to {}",
            suggested_budget(body_bytes(&f, "bare"))
        )));
    }

    #[test]
    fn test_cli_passes_and_selects_agents() {
        let f = fixture();
        let (code, text) = main(&f, &["reader"], &[("reader", 10_000)]);
        assert_eq!(code, 0);
        assert_eq!(text.matches("bytes of").count(), 1);
    }

    #[test]
    fn test_cli_rejects_unknown_agents_and_flags_a_missing_budget() {
        let f = fixture();
        let (code, text) = main(&f, &["nope"], &BUDGET);
        assert_eq!(code, 2);
        assert!(text.contains("unknown agent(s): nope"), "{text}");
        let (code, text) = main(&f, &["reader"], &[]);
        assert_eq!(code, 1);
        assert!(text.contains("no budget set"), "{text}");
    }

    #[test]
    fn test_suggested_budget_rounds_up() {
        assert_eq!(suggested_budget(100), 115);
        assert_eq!(suggested_budget(101), 117); // 116.15 -> 117
    }
}

/// The repository root (tests run with the crate as their working folder).
fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// An independent reading of an agent's frontmatter `skills` (the YAML side of the Python
/// test): the top-level `skills` key in block (`- item`) or flow (`[a, b]`) form, and the body
/// after the closing `---` line. It is written differently from `split_agent` on purpose.
fn frontmatter_skills_and_body(text: &str) -> (Vec<String>, String) {
    // A YAML plain or quoted scalar: a ` #` comment dropped, then one level of quotes (`''` is a
    // quote inside single quotes; double-quoted escapes are refused rather than guessed).
    fn scalar(raw: &str) -> String {
        let mut v = raw.trim();
        let Some(q) = v.chars().next().filter(|c| matches!(c, '"' | '\'')) else {
            if let Some(at) = v.find(" #") {
                v = v[..at].trim_end();
            }
            return v.to_string();
        };
        let close = v[1..].rfind(q).map(|i| i + 1).expect("a closing quote");
        let rest = v[close + 1..].trim();
        assert!(
            rest.is_empty() || rest.starts_with('#'),
            "after the quotes: {raw:?}"
        );
        let inner = &v[1..close];
        if q == '"' {
            assert!(!inner.contains('\\'), "a double-quoted escape: {raw:?}");
            inner.to_string()
        } else {
            inner.replace("''", "'")
        }
    }
    let mut lines = text.split_inclusive('\n');
    assert_eq!(lines.next(), Some("---\n"), "frontmatter opens");
    let mut skills = Vec::new();
    let mut in_skills = false;
    let mut consumed = 4;
    for line in lines.by_ref() {
        consumed += line.len();
        let l = line.trim_end_matches('\n');
        if l == "---" {
            break;
        }
        if let Some(value) = l.strip_prefix("skills:") {
            let value = value.trim();
            if let Some(flow) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) {
                skills.extend(flow.split(',').map(scalar).filter(|s| !s.is_empty()));
                in_skills = false;
            } else {
                in_skills = value.is_empty();
            }
        } else if in_skills && l.trim_start().starts_with("- ") {
            skills.push(scalar(&l.trim_start()[2..]));
        } else if !l.starts_with(' ') {
            in_skills = false;
        }
    }
    let body = text[consumed..].trim_start_matches('\n').to_string();
    (skills, body)
}

mod real_repo_test {
    use super::*;

    #[test]
    fn test_skills_match_the_yaml_frontmatter() {
        for name in ROSTER {
            let path = repo().join(format!("plugins/anthology/agents/{name}.md"));
            let text = std::fs::read_to_string(&path).expect("agent file");
            let (want_skills, want_body) = frontmatter_skills_and_body(&text);
            let (skills, body) = split_agent(&path).expect("agent");
            assert_eq!(skills, want_skills, "{name}");
            assert_eq!(body, want_body, "{name}");
        }
    }

    #[test]
    fn test_budget_covers_the_roster_and_the_repo_is_within_it() {
        let mut budgeted: Vec<&str> = BUDGET.iter().map(|(k, _)| *k).collect();
        budgeted.sort_unstable();
        let mut roster: Vec<&str> = ROSTER.to_vec();
        roster.sort_unstable();
        assert_eq!(budgeted, roster);
        let root = Root::new(&repo(), Path::new("/"));
        assert_eq!(agent_names(&root), strings(&roster));
        let args = [OsString::from("--root"), repo().into_os_string()];
        let out = sb::main_with(&args, &Env::from_process(), Path::new("/"), &BUDGET);
        assert_eq!(out.code, 0, "{}", String::from_utf8_lossy(&out.stdout));
    }
}

/// Not ledger targets: R8 and R10 edges.
mod lead_extras {
    use super::*;

    #[test]
    fn the_budget_table_is_gc13_verbatim() {
        assert_eq!(
            BUDGET,
            [
                ("researcher", 89267),
                ("architect", 23612),
                ("implementer", 26561),
                ("debugger", 22167),
                ("reviewer", 16672),
                ("scout", 5944),
                ("docs-writer", 31376),
                ("ui-designer", 128097),
                ("ui-reviewer", 65986),
                ("product-planner", 102779),
                ("marketer", 54384),
            ]
        );
    }

    #[test]
    fn crlf_bodies_count_after_universal_newlines_and_errors_are_uncaught() {
        let f = fixture();
        write(
            &f.path,
            "plugins/anthology/agents/crlf.md",
            "---\r\nname: crlf\r\n---\r\n\r\n## Role\r\nx\r\n",
        );
        assert_eq!(startup_bytes(&f.root, "crlf").expect("crlf"), 10);
        write(
            &f.path,
            "plugins/anthology/agents/nofm.md",
            "no frontmatter\n",
        );
        let out = sb::main_with(
            &[
                OsString::from("--root"),
                f.path.clone().into_os_string(),
                "nofm".into(),
            ],
            &Env::from_process(),
            Path::new("/"),
            &BUDGET,
        );
        assert_eq!(out.code, 1);
        assert!(out.stdout.is_empty());
        let err = String::from_utf8(out.stderr).expect("utf-8");
        assert!(
            err.starts_with("anthology-dev startup-budget: ValueError: ")
                && err.ends_with("plugins/anthology/agents/nofm.md: missing frontmatter\n"),
            "{err}"
        );
        write(
            &f.path,
            "plugins/anthology/agents/gone.md",
            "---\nname: gone\n---\n## Startup\n- `${CLAUDE_PLUGIN_ROOT}/knowledge/gone/SKILL.md`\n",
        );
        let err = startup_bytes(&f.root, "gone").expect_err("missing file");
        assert_eq!(err.ty, "FileNotFoundError");
        assert!(err.msg.starts_with("[Errno 2] "), "{}", err.msg);
    }

    /// Usage errors captured from CPython 3.12.16 (the cases of the deleted argparse stand-in):
    /// `(arguments, message)`; the stderr is the usage line, then `prog: error: <message>`.
    #[test]
    fn argparse_usage_errors_match_cpython() {
        let cases: &[(&[&str], &str)] = &[
            (&["a", "--root", "x", "b"], "unrecognized arguments: b"),
            (&["--rootx"], "unrecognized arguments: --rootx"),
            (&["-x"], "unrecognized arguments: -x"),
            (&["a", "-x"], "unrecognized arguments: -x"),
            (&["-r=x"], "unrecognized arguments: -r=x"),
            (
                &["--root", "--", "a"],
                "argument --root: expected one argument",
            ),
            (&["--root", "--x"], "argument --root: expected one argument"),
            (
                &["--ro", "--", "a"],
                "argument --root: expected one argument",
            ),
            (
                &["--=x"],
                "ambiguous option: --=x could match --help, --root",
            ),
            (
                &["-=x"],
                "ambiguous option: -=x could match -h, --help, --root",
            ),
            (
                &["-h-"],
                "argument -h/--help: ignored explicit argument '-'",
            ),
            (
                &["-hh="],
                "argument -h/--help: ignored explicit argument ''",
            ),
            (
                &["--help=x"],
                "argument -h/--help: ignored explicit argument 'x'",
            ),
        ];
        for (args, message) in cases {
            let args: Vec<OsString> = args.iter().map(OsString::from).collect();
            let out = sb::main_with(&args, &Env::from_process(), Path::new("/"), &BUDGET);
            let err = String::from_utf8(out.stderr).expect("utf-8");
            assert_eq!(out.code, 2, "{args:?}");
            assert!(
                err.starts_with("usage: startup_budget.py [-h] [--root ROOT] [agents ...]\n")
                    && err.ends_with(&format!("startup_budget.py: error: {message}\n")),
                "{args:?}: {err}"
            );
        }
        // `-h` (and a stuck-on value) prints the help and exits 0.
        for help in [&["-h", "x"][..], &["--he"], &["-hX"], &["-hh"]] {
            let args: Vec<OsString> = help.iter().map(OsString::from).collect();
            let out = sb::main_with(&args, &Env::from_process(), Path::new("/"), &BUDGET);
            assert_eq!(out.code, 0, "{help:?}");
            assert!(
                out.stdout.starts_with(b"usage: startup_budget.py"),
                "{help:?}"
            );
        }
    }

    #[test]
    fn usage_errors_exit_2_with_argparse_text() {
        let out = sb::main_with(
            &[OsString::from("--root")],
            &Env::from_process(),
            Path::new("/"),
            &BUDGET,
        );
        assert_eq!(out.code, 2);
        assert!(
            String::from_utf8_lossy(&out.stderr)
                .ends_with("startup_budget.py: error: argument --root: expected one argument\n")
        );
    }
}
