//! Port of `tests/test_guard_cli.py` (GD2): `anthology guard hook [--shim]` as a process, with
//! exactly the environment each test gives it. Rows: `tests/data/test_guard_cli.jsonl`.
//!
//! Beyond the Python file: the `--shim` contract (N2-I1, GC-17), the fail-closed internal lines
//! (GC-14: a panic with `panic-on-marker`, the watchdog with `test-watchdog`, started before stdin
//! is read), and `--version` (one line, for BS1's probe).

use std::path::Path;
use std::time::Duration;

use anthology_core::Env;
use anthology_pycompat::json::{self, Map, PY_DEFAULT, Value};
use anthology_testkit::rows::{Row, rows};
use anthology_testkit::run::{Output, run_program_timeout};

const BIN: &str = env!("CARGO_BIN_EXE_anthology");
const HOME: &str = "/home/tester";
const FILE: &str = "test_guard_cli";
const TOKEN: &str = "anthology-guard:allow:v1\n";

fn env_with_home(home: Option<&str>) -> Env {
    let env = Env::default().with("PATH", "/usr/bin:/bin");
    match home {
        Some(h) => env.with("HOME", h),
        None => env,
    }
}

fn run_in(stdin: &[u8], home: Option<&str>, shim: bool, cwd: &Path) -> Output {
    let args: &[&str] = if shim {
        &["guard", "hook", "--shim"]
    } else {
        &["guard", "hook"]
    };
    run_program_timeout(
        Path::new(BIN),
        args,
        stdin,
        &env_with_home(home),
        cwd,
        Some(Duration::from_secs(60)),
    )
}

fn run_guard(stdin: &[u8]) -> Output {
    run_in(stdin, Some(HOME), false, Path::new("/"))
}

fn run_shim(stdin: &[u8]) -> Output {
    run_in(stdin, Some(HOME), true, Path::new("/"))
}

/// `hook_input(tool_name, tool_input)`: Python's `json.dumps` of the hook payload.
fn hook_input(tool: &str, input: Value) -> Vec<u8> {
    let mut m = Map::new();
    m.insert("hook_event_name".into(), Value::Str("PreToolUse".into()));
    m.insert("tool_name".into(), Value::Str(tool.into()));
    m.insert("tool_input".into(), input);
    json::dumps(&Value::Object(m), &PY_DEFAULT).into_bytes()
}

fn obj(entries: &[(&str, &str)]) -> Value {
    let mut m = Map::new();
    for (k, v) in entries {
        m.insert((*k).to_string(), Value::Str((*v).to_string()));
    }
    Value::Object(m)
}

fn message(rule: &str) -> &'static str {
    anthology_guard::Rule::ALL
        .iter()
        .find(|r| r.id() == rule)
        .map(|r| r.message())
        .expect("a rule id")
}

fn id(class: &str, method: &str) -> String {
    format!("test_guard_cli.{class}.{method}")
}

/// The six blocked payloads, in the Python table's order.
fn blocked_payloads() -> Vec<(&'static str, &'static str, Value)> {
    vec![
        ("G1", "Bash", obj(&[("command", "npx playwright install")])),
        (
            "G2",
            "Bash",
            obj(&[("command", "open -a \"Google Chrome\" https://x.dev")]),
        ),
        (
            "G3",
            "Read",
            obj(&[("file_path", "~/.config/google-chrome/Default/Cookies")]),
        ),
        (
            "G4",
            "Bash",
            obj(&[("command", "agent-browser open http://localhost:3000")]),
        ),
        (
            "G5",
            "mcp__playwright__browser_navigate",
            obj(&[("url", "https://x.dev")]),
        ),
        (
            "G6",
            "Write",
            obj(&[
                ("file_path", ".claude/memory/a.md"),
                ("content", &["gh", "p_", "abc123"].concat()),
            ]),
        ),
    ]
}

/// The malformed inputs, in the Python table's order.
fn malformed() -> Vec<Vec<u8>> {
    let mut v: Vec<Vec<u8>> = [
        "not json",
        "",
        "   ",
        "{",
        "[",
        "null",
        "[]",
        "[1, 2]",
        "42",
        "\"str\"",
        "true",
        "{}",
        "{\"tool_name\": 5}",
        "{\"tool_name\": null}",
        "{\"tool_name\": [\"Bash\"]}",
        "{\"tool_input\": {\"command\": \"npx playwright install\"}}",
        "{\"tool_name\": \"Bash\"}",
        "{\"tool_name\": \"Bash\", \"tool_input\": \"npx playwright install\"}",
        "{\"tool_name\": \"Bash\", \"tool_input\": null}",
        "{\"tool_name\": \"Bash\", \"tool_input\": []}",
        "{\"tool_name\": \"Bash\", \"tool_input\": {\"command\": 5}}",
        "{\"tool_name\": \"Bash\", \"tool_input\": {\"command\": null}}",
    ]
    .iter()
    .map(|s| s.as_bytes().to_vec())
    .collect();
    v.push("[".repeat(100_000).into_bytes());
    v.push(b"\xff\xfe\x00 not utf-8".to_vec());
    v.push(b"{\"tool_name\": \"Bash\", \"tool_input\": {\"command\": \"ls \xff\"}}".to_vec());
    v
}

mod guard_cli_test {
    use super::*;

    #[test]
    fn test_blocked_payloads_exit_2_with_the_message_on_stderr() {
        let rows: Vec<Row> = rows(
            FILE,
            &id(
                "GuardCliTest",
                "test_blocked_payloads_exit_2_with_the_message_on_stderr",
            ),
        );
        let cases = blocked_payloads();
        assert_eq!(rows.len(), cases.len());
        for (row, (rule, tool, input)) in rows.iter().zip(cases) {
            assert_eq!(row.str("rule"), rule);
            let line = format!(
                "Blocked by anthology-guardrails ({rule}): {}\n",
                message(rule)
            );
            for shim in [false, true] {
                let out = run_in(
                    &hook_input(tool, input.clone()),
                    Some(HOME),
                    shim,
                    Path::new("/"),
                );
                assert_eq!(out.code, 2, "{rule} shim={shim}");
                assert_eq!(out.stderr_text(), line, "{rule} shim={shim}");
                assert!(out.stdout.is_empty());
            }
        }
    }

    #[test]
    fn test_g1_message_wording() {
        let out = run_guard(&hook_input(
            "Bash",
            obj(&[("command", "npx playwright install")]),
        ));
        assert_eq!(out.code, 2);
        assert!(
            out.stderr_text()
                .contains("run it inside Docker (OrbStack); for browser work use BrowserOS Neo")
        );
    }

    #[test]
    fn test_allowed_payloads_exit_0_silently() {
        let rows: Vec<Row> = rows(
            FILE,
            &id("GuardCliTest", "test_allowed_payloads_exit_0_silently"),
        );
        assert_eq!(rows.len(), 7);
        for row in &rows {
            let payload = hook_input(row.str("tool"), row.param("tool_input").clone());
            let out = run_guard(&payload);
            assert_eq!(
                (out.code, out.stderr.as_slice(), out.stdout.as_slice()),
                (0, &b""[..], &b""[..]),
                "row {}",
                row.row
            );
            // the shim mode adds exactly the token
            let out = run_shim(&payload);
            assert_eq!(out.code, 0, "row {}", row.row);
            assert_eq!(out.stderr_text(), TOKEN, "row {}", row.row);
            assert!(out.stdout.is_empty());
        }
    }

    #[test]
    fn test_malformed_input_exits_0() {
        let rows: Vec<Row> = rows(FILE, &id("GuardCliTest", "test_malformed_input_exits_0"));
        let cases = malformed();
        assert_eq!(rows.len(), cases.len());
        for (row, stdin) in rows.iter().zip(&cases) {
            let out = run_guard(stdin);
            assert_eq!(out.code, 0, "row {}", row.row);
            assert!(out.stdout.is_empty(), "row {}", row.row);
            assert!(out.stderr.is_empty(), "row {}", row.row);
            let out = run_shim(stdin);
            assert_eq!(
                (out.code, out.stderr_text()),
                (0, TOKEN.to_string()),
                "row {}",
                row.row
            );
        }
    }

    #[test]
    fn test_invalid_utf8_inside_a_blocked_command_still_blocks() {
        let raw = b"{\"tool_name\": \"Bash\", \"tool_input\": {\"command\": \"echo \xff; npx playwright install\"}}";
        let out = run_guard(raw);
        assert_eq!(out.code, 2);
        assert!(out.stderr_text().contains("(G1)"));
    }

    #[test]
    fn test_self_test_sample_exits_2() {
        let out = run_guard(anthology_guard::data::SELF_TEST_SAMPLE_JSON.as_bytes());
        assert_eq!(out.code, 2);
        assert!(out.stderr_text().contains("(G1)"));
    }

    #[test]
    fn test_home_comes_from_the_environment() {
        let someone = ["/home/", "someone"].concat();
        let payload = hook_input("Read", obj(&[("file_path", "~/.config/chromium/x")]));
        assert_eq!(
            run_in(&payload, Some(&someone), false, Path::new("/")).code,
            2
        );
        // With no HOME at all the guard falls back to the passwd home and still works.
        assert_eq!(run_in(&payload, None, false, Path::new("/")).code, 2);
        let payload = hook_input("Bash", obj(&[("command", "cd ~ && ls .config/chromium")]));
        assert_eq!(
            run_in(&payload, Some(&someone), false, Path::new("/")).code,
            2
        );
    }

    #[test]
    fn test_runs_from_any_working_directory() {
        let dir = tempfile::tempdir().expect("temp dir");
        let out = run_in(
            &hook_input("Bash", obj(&[("command", "npx playwright install")])),
            Some(HOME),
            false,
            dir.path(),
        );
        assert_eq!(out.code, 2);
    }
}

// ------------------------------------------------------------------ beyond the Python file

#[test]
fn shim_blocks_are_byte_identical_to_plain_mode() {
    for payload in [
        hook_input("Bash", obj(&[("command", "google-chrome")])),
        hook_input(
            "Bash",
            obj(&[(
                "command",
                &format!("docker run img{}", " fish -cX".repeat(4000)),
            )]),
        ),
    ] {
        let plain = run_guard(&payload);
        let shim = run_shim(&payload);
        assert_eq!(plain.code, 2);
        assert_eq!(
            (plain.code, &plain.stderr, &plain.stdout),
            (shim.code, &shim.stderr, &shim.stdout)
        );
        assert!(!shim.stderr_text().contains("anthology-guard:allow"));
    }
}

#[test]
fn a_command_too_complex_to_check_is_blocked_naming_plugin() {
    let command = format!("docker run img{}", " fish -cX".repeat(4000));
    let out = run_guard(&hook_input("Bash", obj(&[("command", &command)])));
    assert_eq!(out.code, 2);
    let err = out.stderr_text();
    assert!(
        err.starts_with(
            "Blocked by anthology-guardrails (G0): this command is too complex to check safely"
        ),
        "{err}"
    );
    assert!(err.contains("/plugin"));
    assert_eq!(err.matches('\n').count(), 1);
}

#[test]
fn an_sdk_host_gets_a_too_complex_line_without_the_plugin_command() {
    let command = format!("docker run img{}", " fish -cX".repeat(4000));
    let payload = hook_input("Bash", obj(&[("command", &command)]));
    // (CLAUDE_CODE_ENTRYPOINT, CLAUDE_CODE_SESSION_ATTENDED, ANTHOLOGY_HOST_MODE, the line names /plugin)
    let rows = [
        (Some("sdk-cli"), None, None, false),
        (Some("sdk-ts"), Some("0"), Some("lean"), false),
        (Some("hostapp"), Some("0"), None, false),
        (None, Some("0"), None, false),
        (Some("sdk-cli"), Some("0"), Some("full"), true),
        (Some("hostapp"), Some("0"), Some("full"), true),
        (Some("cli"), Some("1"), None, true),
        (Some("hostapp"), None, None, true),
        (None, None, Some("lean"), true),
        (None, None, None, true),
    ];
    for (entrypoint, attended, mode, plugin) in rows {
        let mut env = env_with_home(Some(HOME));
        for (name, value) in [
            ("CLAUDE_CODE_ENTRYPOINT", entrypoint),
            ("CLAUDE_CODE_SESSION_ATTENDED", attended),
            ("ANTHOLOGY_HOST_MODE", mode),
        ] {
            if let Some(v) = value {
                env = env.with(name, v);
            }
        }
        for args in [&["guard", "hook"][..], &["guard", "hook", "--shim"]] {
            let out = run_program_timeout(
                Path::new(BIN),
                args,
                &payload,
                &env,
                Path::new("/"),
                Some(Duration::from_secs(60)),
            );
            let err = out.stderr_text();
            assert_eq!(out.code, 2, "{entrypoint:?} {attended:?} {mode:?}");
            assert!(
                err.starts_with(
                    "Blocked by anthology-guardrails (G0): this command is too complex to check \
                     safely; simplify it or split it into smaller commands"
                ),
                "{err}"
            );
            assert_eq!(
                err.contains("/plugin"),
                plugin,
                "{entrypoint:?} {attended:?} {mode:?}: {err}"
            );
            assert_eq!(err.matches('\n').count(), 1);
        }
    }
}

#[test]
fn usage_errors_exit_2() {
    for args in [
        &["guard"][..],
        &["guard", "x"],
        &["guard", "hook", "--nope"],
    ] {
        let out = run_program_timeout(
            Path::new(BIN),
            args,
            b"",
            &env_with_home(Some(HOME)),
            Path::new("/"),
            Some(Duration::from_secs(60)),
        );
        assert_eq!(out.code, 2, "{args:?}");
        assert!(out.stdout.is_empty());
    }
}

#[test]
fn version_is_exactly_one_line() {
    let out = run_program_timeout(
        Path::new(BIN),
        &["--version"],
        b"",
        &env_with_home(Some(HOME)),
        Path::new("/"),
        Some(Duration::from_secs(60)),
    );
    assert_eq!(out.code, 0);
    let text = out.stdout_text();
    assert!(
        text.starts_with("anthology ") && text.ends_with(")\n"),
        "{text:?}"
    );
    assert_eq!(text.matches('\n').count(), 1);
    assert!(out.stderr.is_empty());
}

/// m-5: a panic inside the check gives exactly the GC-14 panic line, and nothing else on stderr.
#[cfg(feature = "panic-on-marker")]
#[test]
fn a_panic_blocks_with_only_the_panic_line() {
    let payload = hook_input(
        "Bash",
        obj(&[("command", "ls anthology-guard-test-panic-marker")]),
    );
    for shim in [false, true] {
        let out = run_in(&payload, Some(HOME), shim, Path::new("/"));
        assert_eq!(out.code, 2);
        assert_eq!(out.stderr_text(), anthology_guard::PANIC_LINE);
        assert!(out.stdout.is_empty());
    }
}

/// m-R6: the watchdog starts before stdin is read, so a hook whose stdin never closes still ends
/// with the GC-14 limit line (the `test-watchdog` feature makes 8 s into 2 s).
#[cfg(feature = "test-watchdog")]
#[test]
fn the_watchdog_fires_while_stdin_stays_open() {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::Instant;
    let start = Instant::now();
    let mut child = Command::new(BIN)
        .args(["guard", "hook", "--shim"])
        .env_clear()
        .env("HOME", HOME)
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    let _keep_open = child.stdin.take();
    let status = child.wait().expect("wait");
    let mut err = String::new();
    child
        .stderr
        .take()
        .expect("stderr")
        .read_to_string(&mut err)
        .expect("read");
    let mut out = Vec::new();
    child
        .stdout
        .take()
        .expect("stdout")
        .read_to_end(&mut out)
        .expect("read");
    assert_eq!(status.code(), Some(2));
    assert_eq!(err, anthology_guard::LIMIT_LINE);
    assert!(out.is_empty());
    assert!(start.elapsed() < Duration::from_secs(30));
}
