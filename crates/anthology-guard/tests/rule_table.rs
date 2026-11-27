//! Port of `tests/test_guard_rules.py` (GD1): the rule table, driven by the extracted rows in
//! `tests/data/test_guard_rules.jsonl` (`{"rule": R, "case": [tool, input]}` per `subTest`),
//! plus the tests whose inputs Python builds in code (built the same way here; their rows only
//! name the case, and each test checks its cases against those names).
//!
//! Fixture secrets and home paths are escaped in the data file and assembled at run time here,
//! so the repository never holds one literally.
//!
//! The Python tests know the token layer by its legacy name, `rtk`. A row whose `row` number is
//! past its Python test's last one is an `atl` copy of a legacy `rtk` row (same rule, `atl` for
//! the legacy `rtk`): `every_legacy_rtk_row_has_an_atl_copy` keeps every legacy row paired.

#[path = "support/gd.rs"]
mod gd;

mod rule_table_test {
    use std::time::{Duration, Instant};

    use anthology_guard::data::{PROFILE_ROOTS, self_test_sample};
    use anthology_guard::{Rule, Verdict, check};
    use anthology_pycompat::json::{Map, Value};
    use anthology_testkit::rows::{FromPyJson, Row, rows};

    use super::gd::{HOME, bash, bash_case, bound, gh_token, obj, rule_of, verdict_id};

    const FILE: &str = "test_guard_rules";

    /// The old name of the token layer, which the Python tests and their rows still use.
    const LEGACY_NAME: &str = "rtk"; // legacy

    fn id(method: &str) -> String {
        format!("test_guard_rules.RuleTableTest.{method}")
    }

    /// Rows the Rust guard blocks where the Python test allowed them, by owner direction (D-8, Rust
    /// stricter): (method, row, rule). PE-1: an array element as the program word is only known at run
    /// time (`"${tools[0]}" open x`), so it is too complex.
    const STRICTER: &[(&str, usize, &str)] = &[
        ("test_allowed_round_1_neighbours", 40, "G0"),
        ("test_allowed_round_1_neighbours", 41, "G0"),
    ];

    /// `assert_table` over every row of `method`: each row's case gives its rule (or none).
    fn table(method: &str) {
        let rows: Vec<Row> = rows(FILE, &id(method));
        assert!(!rows.is_empty(), "{method}: no rows");
        let mut failures = Vec::new();
        for row in &rows {
            let case = row.param("case");
            let stricter = STRICTER
                .iter()
                .find(|(m, r, _)| *m == method && *r == row.row)
                .map(|(_, _, rule)| (*rule).to_string());
            let expected = stricter.or_else(|| row.param("rule").as_str().map(str::to_string));
            let got = rule_of(case);
            if got != expected {
                failures.push(format!(
                    "row {}: {:?} gave {got:?}, expected {expected:?}",
                    row.row, case
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "{method}: {} of {} rows failed:\n{}",
            failures.len(),
            rows.len(),
            failures.join("\n")
        );
    }

    /// The row names of a test whose cases are built here; they must match `names` in order.
    fn named_rows(method: &str, key: &str, names: &[&str]) {
        let rows: Vec<Row> = rows(FILE, &id(method));
        let got: Vec<&str> = rows.iter().map(|r| r.str(key)).collect();
        assert_eq!(got, names, "{method}: the rows name other cases");
    }

    /// A catastrophe guard: a bound below 8 s is raised to it, because a slow runner is not a regression.
    fn assert_quick(what: &str, bound: Duration, f: impl FnOnce()) {
        // the x86_64 macOS CI runner is about 4x slower than the others (9 to 11 s on a case that
        // takes 2 s elsewhere), so it gets 3x the bound; a real blow-up is far past even that
        let slow_runner = if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
            3
        } else {
            1
        };
        let bound = bound.max(Duration::from_secs(8)) * slow_runner;
        let start = Instant::now();
        f();
        let took = start.elapsed();
        assert!(took < bound, "{what}: {took:?} (bound {bound:?})");
    }

    // ------------------------------------------------------------------ blocked

    #[test]
    fn test_g1_browser_automation_installs() {
        table("test_g1_browser_automation_installs");
    }

    #[test]
    fn test_g2_host_chrome_launch() {
        table("test_g2_host_chrome_launch");
    }

    #[test]
    fn test_g3_profile_paths_in_shell() {
        table("test_g3_profile_paths_in_shell");
    }

    #[test]
    fn test_g3_profile_paths_in_file_tools() {
        table("test_g3_profile_paths_in_file_tools");
    }

    #[test]
    fn test_g4_agent_browser() {
        table("test_g4_agent_browser");
    }

    #[test]
    fn test_g5_browser_mcp_tools() {
        table("test_g5_browser_mcp_tools");
    }

    #[test]
    fn test_g6_secrets_into_memory_in_file_tools() {
        table("test_g6_secrets_into_memory_in_file_tools");
    }

    #[test]
    fn test_g6_secrets_into_memory_in_bash() {
        table("test_g6_secrets_into_memory_in_bash");
    }

    #[test]
    fn test_chains_wrappers_and_prefixes() {
        table("test_chains_wrappers_and_prefixes");
    }

    // ------------------------------------------------------------------ allowed

    #[test]
    fn test_allowed_commands() {
        table("test_allowed_commands");
    }

    #[test]
    fn test_allowed_g1_neighbours() {
        table("test_allowed_g1_neighbours");
    }

    #[test]
    fn test_allowed_commit_and_pr_heredoc_messages() {
        table("test_allowed_commit_and_pr_heredoc_messages");
    }

    #[test]
    fn test_allowed_wildcard_only_paths() {
        table("test_allowed_wildcard_only_paths");
    }

    #[test]
    fn test_allowed_free_text_fields_are_not_paths() {
        table("test_allowed_free_text_fields_are_not_paths");
    }

    #[test]
    fn test_allowed_round_1_neighbours() {
        table("test_allowed_round_1_neighbours");
    }

    #[test]
    fn test_allowed_docker_exemption() {
        table("test_allowed_docker_exemption");
    }

    #[test]
    fn test_docker_exempts_only_g1_and_g2() {
        table("test_docker_exempts_only_g1_and_g2");
    }

    #[test]
    fn test_allowed_g2_neighbours() {
        table("test_allowed_g2_neighbours");
    }

    #[test]
    fn test_allowed_g3_neighbours() {
        table("test_allowed_g3_neighbours");
    }

    #[test]
    fn test_allowed_g4_neighbours() {
        table("test_allowed_g4_neighbours");
    }

    #[test]
    fn test_allowed_g5_neighbours() {
        table("test_allowed_g5_neighbours");
    }

    #[test]
    fn test_allowed_g6_neighbours() {
        table("test_allowed_g6_neighbours");
    }

    #[test]
    fn test_allowed_prose_and_odd_but_ordinary_commands() {
        table("test_allowed_prose_and_odd_but_ordinary_commands");
    }

    #[test]
    fn test_g3_strings_in_content_fields_are_not_paths() {
        table("test_g3_strings_in_content_fields_are_not_paths");
    }

    // ---------------------------------------------------------------- ordering

    #[test]
    fn test_rule_order_g5_g6_then_g3_g4_g1_g2() {
        // Python keeps the expected rules beside the cases, outside the subTest keywords.
        let expected = ["G5", "G3", "G4", "G4", "G3", "G1", "G2", "G6", "G6", "G6"];
        let rows: Vec<Row> = rows(FILE, &id("test_rule_order_g5_g6_then_g3_g4_g1_g2"));
        assert_eq!(rows.len(), expected.len());
        for (row, want) in rows.iter().zip(expected) {
            assert_eq!(
                rule_of(row.param("case")).as_deref(),
                Some(want),
                "row {}: {:?}",
                row.row,
                row.param("case")
            );
        }
    }

    // ---------------------------------------------------------- shell analysis

    #[test]
    fn test_shell_analysis_applies_to_any_tool_with_a_command() {
        table("test_shell_analysis_applies_to_any_tool_with_a_command");
    }

    #[test]
    fn test_home_param_controls_expansion() {
        let read = obj(&[("file_path", Value::Str("~/.config/chromium/x".into()))]);
        let tool = Value::Str("Read".into());
        let other = ["/", "home/other"].concat();
        // Python passes "/home/other", "" and None (read as ""), and blocks each.
        for home in [other.as_str(), "", ""] {
            assert_eq!(
                check(&tool, &read, home),
                Verdict::Block(Rule::G3),
                "{home:?}"
            );
        }
        let (tool, input) = bash("cd ~ && ls .config/chromium");
        assert_eq!(check(&tool, &input, &other), Verdict::Block(Rule::G3));
    }

    // -------------------------------------------------------------- interface

    #[test]
    fn test_messages_cover_g1_to_g6_verbatim() {
        let messages: Vec<(&str, &str)> = Rule::ALL.iter().map(|r| (r.id(), r.message())).collect();
        assert_eq!(
            messages,
            [
                (
                    "G1",
                    "run it inside Docker (OrbStack); for browser work use BrowserOS Neo"
                ),
                ("G2", "never launch Chrome on the host; use BrowserOS Neo"),
                ("G3", "Chrome profiles are off-limits"),
                (
                    "G4",
                    "agent-browser is not used here; use the BrowserOS Neo MCP tools"
                ),
                ("G5", "use the BrowserOS Neo MCP tools"),
                (
                    "G6",
                    "memory is committed to the repo; never store secrets there"
                ),
            ]
        );
    }

    #[test]
    fn test_check_returns_rule_and_its_message() {
        let write = obj(&[
            ("file_path", Value::Str(".claude/memory/a.md".into())),
            ("content", Value::Str(gh_token())),
        ]);
        let cases: Vec<(&str, (Value, Value))> = vec![
            ("G1", bash("npx playwright install")),
            ("G2", bash("google-chrome")),
            ("G3", bash("ls ~/.config/chromium")),
            ("G4", bash("agent-browser open x")),
            (
                "G5",
                (
                    Value::Str("mcp__playwright__x".into()),
                    Value::Object(Map::new()),
                ),
            ),
            ("G6", (Value::Str("Write".into()), write)),
        ];
        let rows: Vec<Row> = rows(FILE, &id("test_check_returns_rule_and_its_message"));
        assert_eq!(rows.len(), cases.len());
        for (row, (rule, (tool, input))) in rows.iter().zip(&cases) {
            assert_eq!(row.str("rule"), *rule);
            let verdict = check(tool, input, HOME);
            let Verdict::Block(got) = verdict else {
                panic!("{rule}: {verdict:?}");
            };
            assert_eq!(got.id(), *rule);
            // the hook prints `got.message()`: the message belongs to the rule that blocked
            assert_eq!(
                Rule::ALL
                    .iter()
                    .find(|r| r.id() == *rule)
                    .map(|r| r.message()),
                Some(got.message())
            );
        }
    }

    #[test]
    fn test_self_test_sample_is_a_blocked_hook_input() {
        let sample = self_test_sample();
        let o = sample.as_object().unwrap();
        assert_eq!(
            o.get("hook_event_name").and_then(Value::as_str),
            Some("PreToolUse")
        );
        assert_eq!(o.get("tool_name").and_then(Value::as_str), Some("Bash"));
        let input = o.get("tool_input").unwrap();
        assert_eq!(
            input
                .as_object()
                .and_then(|i| i.get("command"))
                .and_then(Value::as_str),
            Some("npx playwright install chromium")
        );
        assert_eq!(o.iter().count(), 3);
        assert_eq!(
            check(o.get("tool_name").unwrap(), input, HOME),
            Verdict::Block(Rule::G1)
        );
    }

    #[test]
    fn test_profile_roots_are_the_four_home_relative_paths() {
        assert_eq!(
            PROFILE_ROOTS,
            [
                "Library/Application Support/Google",
                "Library/Application Support/Chromium",
                ".config/google-chrome",
                ".config/chromium",
            ]
        );
    }

    #[test]
    fn test_every_profile_root_is_blocked_under_home() {
        let mut paths = Vec::new();
        for root in PROFILE_ROOTS {
            paths.push(root.to_string());
            paths.push(format!("{root}/Default/Cookies"));
        }
        let names: Vec<&str> = paths.iter().map(String::as_str).collect();
        named_rows(
            "test_every_profile_root_is_blocked_under_home",
            "path",
            &names,
        );
        for path in &paths {
            let read = obj(&[("file_path", Value::Str(format!("{HOME}/{path}")))]);
            assert_eq!(
                check(&Value::Str("Read".into()), &read, HOME),
                Verdict::Block(Rule::G3),
                "Read {path}"
            );
            let (tool, input) = bash(&format!("cat ~/{}", path.replace(' ', "\\ ")));
            assert_eq!(
                check(&tool, &input, HOME),
                Verdict::Block(Rule::G3),
                "cat {path}"
            );
        }
    }

    // ----------------------------------------------------------------- robust

    /// `test_check_never_raises_on_odd_inputs`'s commands, built as Python builds them.
    fn odd_commands() -> Vec<String> {
        let huge = "a".repeat(300_000);
        let mut v: Vec<String> = [
            "echo 'unbalanced",
            "echo \"unbalanced",
            "echo `unbalanced",
            "echo $(unbalanced",
            "echo ${unbalanced",
            "echo $'unbalanced",
            "((((((",
            "))))))",
            "$(",
            "`",
            "'",
            "\"",
            "\\",
            "<<",
            "<<EOF",
            "<<EOF\nbody",
            "cat <<EOF\nit's\nnever closed",
            "<<<",
            ">",
            "&>",
            "| | |",
            "; ; ;",
            "&& ||",
            "\x00",
            "\n\n\n",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
        v.push(format!("{}{}", "$(".repeat(50), ")".repeat(50)));
        v.push("`".repeat(99));
        v.push(format!(
            "{}npx playwright install{}",
            "(".repeat(500),
            ")".repeat(500)
        ));
        v.push(format!(
            "{}npx playwright install{}",
            "$(".repeat(200),
            ")".repeat(200)
        ));
        v.push(format!("{}ls{}", "bash -c '".repeat(30), "'".repeat(30)));
        for s in [
            "cat <<EOF\nit's a heredoc with apostrophes\nEOF\necho don't",
            "echo don't; npx playwright install",
            "git commit -m \"it's\" && echo it's",
            // Python's "ls ~/\udcff": a lone surrogate, which JSON input turns into U+FFFD (D-6)
            "ls ~/\u{fffd}",
            "ls \u{202e}",
            "sudo",
            "env -S",
            "npx",
            "npx -p",
            "python -m",
            "timeout",
            "xargs -I",
            "rtk",        // legacy
            "rtk bypass", // legacy
            "docker",
            "docker run -c",
            "find -exec",
            "open -a",
            "open -b",
            "cd",
            "pushd",
            "tee",
            "dd of=",
            "sed -i",
            "FOO=",
        ] {
            v.push(s.to_string());
        }
        v.push(huge.clone());
        v.push(format!("{huge}\nnpx playwright install"));
        v.push("x ".repeat(100_000));
        v.push("\n".repeat(100_000));
        v.push(format!("echo {}", "'".repeat(100_001)));
        v.push(
            (0..3000)
                .map(|i| format!("cd d{i}"))
                .collect::<Vec<_>>()
                .join(" && "),
        );
        v.push("cat <<EOF x\n".repeat(2000));
        v.push("$(cat <<EOF\n".repeat(300));
        v.push(format!("{}ls{}", "echo $(".repeat(40), ")".repeat(40)));
        v
    }

    /// Python's odd `(tool, tool_input)` pairs. Values JSON cannot carry (`bytes`, a `None` or
    /// int key) become the nearest JSON value of another type; the point is the same: check
    /// never fails on them.
    fn odd_inputs() -> Vec<(Value, Value)> {
        let s = |x: &str| Value::Str(x.into());
        let i = |n: i64| Value::Int(anthology_pycompat::json::Int::Small(n));
        let a = Value::Array;
        let memory = || s(".claude/memory/a.md");
        let mut deep = a(vec![s("~/x")]);
        for _ in 0..21 {
            deep = a(vec![deep]);
        }
        vec![
            (s("Bash"), Value::Null),
            (s("Bash"), a(vec![])),
            (s("Bash"), s("ls")),
            (s("Bash"), i(5)),
            (s("Bash"), obj(&[])),
            (s("Bash"), obj(&[("command", Value::Null)])),
            (s("Bash"), obj(&[("command", i(5))])),
            (s("Bash"), obj(&[("command", a(vec![s("ls")]))])),
            (s("Bash"), obj(&[("command", obj(&[("a", s("b"))]))])),
            (s("Bash"), obj(&[("command", a(vec![i(108), i(115)]))])), // b"ls"
            (Value::Null, obj(&[("command", s("ls"))])),
            (i(5), obj(&[])),
            (a(vec![s("x")]), obj(&[])),
            (obj(&[]), obj(&[])),
            (a(vec![i(120)]), obj(&[])), // b"x"
            (s("Read"), obj(&[("file_path", Value::Null)])),
            (s("Read"), obj(&[("file_path", i(5))])),
            (
                s("Read"),
                obj(&[("file_path", a(vec![s("~/.config/chromium")]))]),
            ),
            (s("Read"), obj(&[("None", s("x"))])), // {None: "x"}
            (s("Read"), obj(&[("5", s("x"))])),    // {5: "x"}
            (
                s("Write"),
                obj(&[("file_path", Value::Null), ("content", Value::Null)]),
            ),
            (
                s("Write"),
                obj(&[("file_path", memory()), ("content", Value::Null)]),
            ),
            (
                s("Write"),
                obj(&[("file_path", memory()), ("content", i(5))]),
            ),
            (
                s("Write"),
                obj(&[
                    ("file_path", memory()),
                    ("content", a(vec![s(&gh_token())])),
                ]),
            ),
            (
                s("Write"),
                obj(&[("file_path", i(5)), ("content", s(&gh_token()))]),
            ),
            (
                s("MultiEdit"),
                obj(&[("file_path", memory()), ("edits", Value::Null)]),
            ),
            (
                s("MultiEdit"),
                obj(&[
                    ("file_path", memory()),
                    ("edits", a(vec![Value::Null, i(5), s("x"), obj(&[])])),
                ]),
            ),
            (
                s("MultiEdit"),
                obj(&[
                    ("file_path", memory()),
                    ("edits", a(vec![obj(&[("new_string", Value::Null)])])),
                ]),
            ),
            (s("Glob"), obj(&[("pattern", Value::Null), ("path", i(5))])),
            (
                s("Grep"),
                obj(&[("pattern", a(vec![s("a")])), ("path", obj(&[("b", i(1))]))]),
            ),
            (
                s("Glob"),
                obj(&[("pattern", s("*")), ("path", Value::Null)]),
            ),
            (
                s("Tool"),
                obj(&[(
                    "a",
                    obj(&[(
                        "b",
                        obj(&[(
                            "c",
                            obj(&[(
                                "d",
                                a(vec![
                                    i(1),
                                    i(2),
                                    a(vec![
                                        i(3),
                                        a(vec![i(4), obj(&[("e", s("~/.config/chromium"))])]),
                                    ]),
                                ]),
                            )]),
                        )]),
                    )]),
                )]),
            ),
            (s("Tool"), obj(&[("a", deep)])),
            (
                s("Tool"),
                obj(&[(&"k".repeat(1000), s(&"v".repeat(100_000)))]),
            ),
            (
                s("Tool"),
                obj(&[
                    ("command", s("ls")),
                    ("x", Value::Float(f64::NAN)),
                    ("y", Value::Bool(true)),
                    ("z", Value::Float(3.5)),
                ]),
            ),
        ]
    }

    #[test]
    fn test_check_never_raises_on_odd_inputs() {
        let rows: Vec<Row> = rows(FILE, &id("test_check_never_raises_on_odd_inputs"));
        let commands = odd_commands();
        let inputs = odd_inputs();
        let homes: Vec<Value> = {
            let s = |x: &str| Value::Str(x.into());
            vec![
                Value::Null,
                Value::Int(anthology_pycompat::json::Int::Small(5)),
                Value::Array(vec![]),
                obj(&[]),
                s(""),
                s("relative"),
                s("/"),
                s("//"),
                s(&format!("{HOME}/")),
                s("~"),
                s("$HOME"),
                Value::Array(vec![Value::Int(anthology_pycompat::json::Int::Small(47))]), // b"/x"
            ]
        };
        assert_eq!(rows.len(), commands.len() + inputs.len() + homes.len());
        let (cmd_rows, rest) = rows.split_at(commands.len());
        let (input_rows, home_rows) = rest.split_at(inputs.len());
        for (row, command) in cmd_rows.iter().zip(&commands) {
            // the row holds `command[:60]` (code points)
            let head: String = command.chars().take(60).collect();
            assert_eq!(row.str("command"), head, "row {}", row.row);
            let (tool, input) = bash(command);
            let _ = check(&tool, &input, HOME);
        }
        for (row, (tool, input)) in input_rows.iter().zip(&inputs) {
            assert!(row.params.get("tool").is_some(), "row {}", row.row);
            let _ = check(tool, input, HOME);
        }
        for (row, home) in home_rows.iter().zip(&homes) {
            assert!(row.params.get("home").is_some(), "row {}", row.row);
            // `check` takes the home as text; Python reads a non-str home as "".
            let home = home.as_str().unwrap_or("");
            let (tool, input) = bash("cd ~ && ls .config");
            let _ = check(&tool, &input, home);
            let read = obj(&[("file_path", Value::Str("~/.config/chromium".into()))]);
            let _ = check(&Value::Str("Read".into()), &read, home);
        }
    }

    #[test]
    fn test_array_comments_do_not_hide_later_commands() {
        table("test_array_comments_do_not_hide_later_commands");
    }

    #[test]
    fn test_commented_array_stays_cheap() {
        let command = format!(
            "arr=(\n{})\nnpx playwright install",
            "  x # it's (\n".repeat(10000)
        );
        assert_quick("commented array", bound(500), || {
            assert_eq!(rule_of(&bash_case(&command)).as_deref(), Some("G1"));
        });
    }

    #[test]
    fn test_unterminated_heredocs_stay_cheap() {
        let cases = [
            (
                "nested substitution",
                format!("{}{}", "\"$(".repeat(12), "cat <<X\n".repeat(2000)),
            ),
            ("lexer", "cat <<X\n".repeat(2000)),
            (
                "nested array",
                format!("{}{}", "a=(".repeat(12), "cat <<X\n".repeat(2000)),
            ),
        ];
        let names: Vec<&str> = cases.iter().map(|(n, _)| *n).collect();
        named_rows("test_unterminated_heredocs_stay_cheap", "case", &names);
        for (name, command) in &cases {
            assert_quick(name, bound(200), || {
                let (tool, input) = bash(command);
                let _ = check(&tool, &input, HOME);
            });
        }
    }

    #[test]
    fn test_check_survives_deep_nesting() {
        let mut nested = obj(&[]);
        for _ in 0..5000 {
            nested = obj(&[("a", Value::Array(vec![nested]))]);
        }
        assert_eq!(
            check(&Value::Str("Tool".into()), &nested, HOME),
            Verdict::Allow
        );
    }

    #[test]
    fn test_unbalanced_quotes_still_apply_the_rules() {
        table("test_unbalanced_quotes_still_apply_the_rules");
    }

    // ------------------------------------------- fix lane F2 (final review I1-I3)

    #[test]
    fn test_rtk_command_runners_are_unwrapped() {
        table("test_rtk_command_runners_are_unwrapped");
    }

    /// Every `case` row that names the legacy `rtk` has an `atl` row with the same test, rule and
    /// command (`atl` for the legacy `rtk`), so the guard looks through the new name as through the old.
    #[test]
    fn every_legacy_rtk_row_has_an_atl_copy() {
        let path = anthology_testkit::rows::rows_path(FILE);
        let text = std::fs::read_to_string(&path).expect("the rule table rows");
        let mut seen = std::collections::HashSet::new();
        let mut legacy = Vec::new();
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let row = Row::from_py_json(&anthology_pycompat::json::loads(line).expect("a row"))
                .expect("a row");
            let Some(Value::Array(case)) = row.params.get("case") else {
                continue;
            };
            let command = case
                .get(1)
                .and_then(Value::as_object)
                .and_then(|input| input.get("command"))
                .and_then(Value::as_str);
            let Some(command) = command else { continue };
            let rule = row.params.get("rule").and_then(Value::as_str);
            seen.insert((
                row.py.clone(),
                rule.map(str::to_string),
                command.to_string(),
            ));
            if command.contains(LEGACY_NAME) {
                legacy.push((row.py, rule.map(str::to_string), command.to_string()));
            }
        }
        assert!(legacy.len() >= 60, "{} legacy rows", legacy.len());
        let missing: Vec<String> = legacy
            .into_iter()
            .filter(|(py, rule, command)| {
                let copy = command.replace(LEGACY_NAME, "atl");
                !seen.contains(&(py.clone(), rule.clone(), copy))
            })
            .map(|(py, rule, command)| format!("{py} {rule:?} {command:?}"))
            .collect();
        assert!(
            missing.is_empty(),
            "no atl copy of:\n{}",
            missing.join("\n")
        );
    }

    #[test]
    fn test_shell_option_parser() {
        table("test_shell_option_parser");
    }

    #[test]
    fn test_g3_double_backslash_space_in_code_literals() {
        let bs = "\\\\";
        let dbl = format!("~/Library/Application{bs} Support/Google/Chrome/Default/Bookmarks");
        assert_eq!(dbl.matches('\\').count(), 2);
        table("test_g3_double_backslash_space_in_code_literals");
    }

    #[test]
    fn test_g3_profile_paths_in_inline_code() {
        table("test_g3_profile_paths_in_inline_code");
    }

    #[test]
    fn test_g3_code_keys_in_tools() {
        table("test_g3_code_keys_in_tools");
    }

    #[test]
    fn test_g3_percent_encoded_file_urls() {
        table("test_g3_percent_encoded_file_urls");
    }

    #[test]
    fn test_inline_code_scan_stays_cheap() {
        let line =
            |k: usize| format!("x = os.path.join('src', 'pkg/mod_{k}.py'); print('done', x)\n");
        let program: String = (0..4000).map(line).collect();
        assert!(program.len() > 200_000);
        let heredoc = format!(
            "python3 - <<'EOF'\n{}EOF",
            (0..10_000).map(line).collect::<String>()
        );
        let cases = [
            ("-c program", format!("python3 -c \"{program}\""), None),
            (
                "-c program + path",
                format!("python3 -c \"{program}open('~/.config/chromium/Default/Cookies')\""),
                Some("G3"),
            ),
            ("heredoc", heredoc.clone(), None),
            (
                "heredoc + path",
                format!(
                    "{}open('~/.config/chromium/x')\nEOF",
                    &heredoc[..heredoc.len() - 3]
                ),
                Some("G3"),
            ),
        ];
        let names: Vec<&str> = cases.iter().map(|(n, _, _)| *n).collect();
        named_rows("test_inline_code_scan_stays_cheap", "case", &names);
        for (name, command, expected) in &cases {
            assert_quick(name, bound(1000), || {
                assert_eq!(rule_of(&bash_case(command)).as_deref(), *expected, "{name}");
            });
        }
    }

    #[test]
    fn test_g3_quote_in_a_comment_does_not_hide_a_code_literal() {
        table("test_g3_quote_in_a_comment_does_not_hide_a_code_literal");
    }

    #[test]
    fn test_g3_save_skill_fn_is_code() {
        table("test_g3_save_skill_fn_is_code");
    }

    #[test]
    fn test_g3_browseros_neo_real_code_keys() {
        table("test_g3_browseros_neo_real_code_keys");
    }

    #[test]
    fn test_bad_glob_ranges_do_not_fail_open() {
        table("test_bad_glob_ranges_do_not_fail_open");
    }

    #[test]
    fn test_more_interpreters_and_fish() {
        table("test_more_interpreters_and_fish");
    }

    #[test]
    fn test_fish_is_parsed_with_fish_options() {
        table("test_fish_is_parsed_with_fish_options");
    }

    /// Three of these rows run in Python with `_FISH_LONG_VALUE` patched down to the script
    /// options, to show the table is not what keeps the form closed. anthology-shell has no
    /// patch seam for that table; the rows run against the real one, with the same expected
    /// rule (the form is closed either way).
    #[test]
    fn test_fish_fails_closed_past_unknown_options() {
        table("test_fish_fails_closed_past_unknown_options");
    }

    #[test]
    fn test_code_candidate_prefilter_stays_cheap() {
        let literals: String = (0..60_000)
            .map(|k| format!("v{k} = 'lit{k}/a/b/c'\n"))
            .collect();
        let words = (0..200_000)
            .map(|k| format!("w{k}/x/y/z/q/r/s"))
            .collect::<Vec<_>>()
            .join(" ");
        let url = ["file:%2F", "home%2Ftester%2F.config%2Fchromium"].concat();
        let cases = [
            (
                "60k unique literals",
                format!("python3 - <<'EOF'\n{literals}EOF"),
                None,
            ),
            (
                "60k unique literals + path",
                format!("python3 - <<'EOF'\n{literals}open('~/.config/chromium/x')\nEOF"),
                Some("G3"),
            ),
            (
                "200k path-like words",
                format!("python3 -c '{words}'"),
                None,
            ),
            (
                "200k path-like words + url",
                format!("python3 -c '{words} {url}'"),
                Some("G3"),
            ),
        ];
        let names: Vec<&str> = cases.iter().map(|(n, _, _)| *n).collect();
        named_rows("test_code_candidate_prefilter_stays_cheap", "case", &names);
        for (name, command, expected) in &cases {
            assert_quick(name, bound(1000), || {
                assert_eq!(rule_of(&bash_case(command)).as_deref(), *expected, "{name}");
            });
        }
    }

    #[test]
    fn test_escaped_space_normalising_scales_linearly() {
        fn cost(count: usize) -> Duration {
            let command = format!("python3 -c '{}'", "\\".repeat(count));
            let case = bash_case(&command);
            (0..3)
                .map(|_| {
                    let start = Instant::now();
                    assert_eq!(rule_of(&case), None);
                    start.elapsed()
                })
                .min()
                .unwrap_or_default()
        }
        let (small, large) = (cost(50_000), cost(200_000));
        assert!(
            large < (10 * small).max(Duration::from_millis(500)),
            "{small:?} then {large:?}"
        );
        // the run still normalises when a space ends it
        let bs = "\\".repeat(50_000);
        let command =
            format!("python3 -c 'open(\"~/Library/Application{bs} Support/Google/Chrome\")'");
        assert_eq!(rule_of(&bash_case(&command)).as_deref(), Some("G3"));
    }

    #[test]
    fn test_check_is_deterministic_and_pure() {
        let (tool, input) = bash("npx playwright install");
        let before = input.clone();
        let first = check(&tool, &input, HOME);
        let second = check(&tool, &input, HOME);
        assert_eq!(first, second);
        assert_eq!(input, before);
        assert_eq!(verdict_id(first), Some("G1".to_string()));
    }
}
