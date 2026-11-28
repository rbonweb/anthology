//! Port of tests/test_smoke.py (plus a few Rust-only tests, named as such): no model, no network,
//! Docker-safe.
//!
//! The smokes themselves need a real Claude Code and credentials; these tests cover the pieces
//! that do not: the name list, the isolated environment, SKIP without credentials, the helpers
//! and the summary. Where Python patched something with `mock.patch`, the test replaces the
//! matching `Deps` field (plan GC-19, row `dev::smoke`). The process environment is never
//! changed: every test passes its own `Env`.

use std::cell::RefCell;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anthology_core::Env;
use anthology_dev::smoke::cases;
use anthology_dev::smoke::runner::*;
use anthology_dev::smoke::{self, run_with};
use anthology_pycompat::json::{self, Value};

const NAMES: [&str; 12] = [
    "cli-syntax",
    "eval-ceiling",
    "context-cap",
    "missing-interpreter",
    "python-missing",
    "worktree-base",
    "model-check",
    "knowledge-read-writers",
    "auto-memory-dir",
    "writer-memory-commit",
    "atl-rewrite",
    "agent-memory-path",
];
/// The smokes EV1b adds after `guard-timeout`.
const EV1B_NAMES: [&str; 6] = [
    "binary-missing",
    "live-seed",
    "live-setup",
    "live-mem",
    "live-session-start",
    "live-guard",
];
const MODEL_SMOKES: [&str; 10] = [
    "eval-ceiling",
    "context-cap",
    "missing-interpreter",
    "worktree-base",
    "model-check",
    "knowledge-read-writers",
    "auto-memory-dir",
    "writer-memory-commit",
    "atl-rewrite",
    "agent-memory-path",
];

/// `tests/_repo.py` `EXPECTED_ROSTER`: (name, model, worktree, preloads, knowledge).
type RosterRow = (
    &'static str,
    &'static str,
    bool,
    &'static [&'static str],
    &'static [&'static str],
);
const UI_KNOWLEDGE: [&str; 10] = [
    "frontend-design",
    "design-taste-frontend",
    "ui-taste",
    "landing-page",
    "web-design-system",
    "design-handoff",
    "ds/design-systems",
    "ds/interaction-design",
    "ds/design-elevation",
    "website-clone",
];
const EXPECTED_ROSTER: [RosterRow; 11] = [
    (
        "researcher",
        "opus",
        false,
        &[],
        &[
            "competitive-brief",
            "synthesize-research",
            "research-synthesis",
            "ds/ux-research",
            "ds/journey-mapping",
        ],
    ),
    (
        "architect",
        "opus",
        false,
        &["writing-plans"],
        &["system-design", "architecture", "testing-strategy"],
    ),
    (
        "implementer",
        "sonnet",
        true,
        &["test-driven-development", "verification-before-completion"],
        &["karpathy-guidelines"],
    ),
    (
        "debugger",
        "sonnet",
        true,
        &["systematic-debugging"],
        &["karpathy-guidelines"],
    ),
    (
        "reviewer",
        "opus",
        false,
        &[],
        &[
            "code-review-checklist",
            "karpathy-guidelines",
            "tech-debt",
            "deploy-checklist",
        ],
    ),
    ("scout", "haiku", false, &[], &[]),
    (
        "docs-writer",
        "sonnet",
        true,
        &[],
        &["documentation", "ds/ux-writing"],
    ),
    ("ui-designer", "sonnet", true, &[], &UI_KNOWLEDGE),
    (
        "ui-reviewer",
        "opus",
        false,
        &[],
        &[
            "web-design-guidelines",
            "web-review",
            "ds/design-critique",
            "ds/accessibility-audit",
        ],
    ),
    (
        "product-planner",
        "opus",
        false,
        &[],
        &[
            "write-spec",
            "sprint-planning",
            "roadmap-update",
            "product-brainstorming",
            "ds/ux-strategy",
            "ds/design-ops",
        ],
    ),
    (
        "marketer",
        "sonnet",
        false,
        &[],
        &[
            "seo-audit",
            "campaign-plan",
            "content-creation",
            "email-sequence",
        ],
    ),
];

// ---- helpers ------------------------------------------------------------------------------

/// The checkout this test runs in (`smoke.ROOT`).
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

/// `Deps::default()` with the `root` seam pointing at `root`, and the staged plugins being
/// `<root>/plugins` itself (no copy; the tests that need the staged shims use the real `stage`).
fn deps_at(root: &Path) -> Deps {
    let root = root.to_path_buf();
    let plugins = root.join("plugins");
    Deps {
        root: Box::new(move || Ok(root.clone())),
        stage: Box::new(move |_ctx| {
            Ok(Staged {
                plugins: plugins.clone(),
                keep: None,
            })
        }),
        ..Deps::default()
    }
}

fn deps() -> Deps {
    deps_at(&repo_root())
}

/// `no_auth_environ()`: the process environment without the auth variables and `CLAUDE_BIN`.
fn no_auth_env() -> Env {
    let mut env = Env::from_process();
    for k in AUTH_VARS {
        env = env.without(k);
    }
    env.without("CLAUDE_BIN")
}

fn args(items: &[&str]) -> Vec<OsString> {
    items.iter().map(OsString::from).collect()
}

/// `run_main(argv)`: the exit code and what was printed to stdout.
fn run_main(argv: &[&str], env: Env, deps: Deps) -> (i32, String) {
    let out = run_with(&args(argv), env, deps);
    (
        out.code,
        String::from_utf8(out.stdout).expect("utf-8 output"),
    )
}

/// The `anthology` path the tests hand to smokes that only pass it on.
const FAKE_BIN: &str = "/nonexistent/anthology";

fn ctx_with(env: Env, deps: Deps) -> Ctx {
    Ctx::new("claude", "1.00", 10.0, env, deps)
}

fn load(text: &str) -> Value {
    json::loads(text).expect("valid JSON")
}

fn write_exec(path: &Path, text: &str) {
    fs::write(path, text).expect("write");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod");
}

fn to_strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

fn stream(events: &[Value]) -> String {
    events.iter().map(dumps).collect::<Vec<_>>().join("\n")
}

fn bash_call(command: &str) -> Value {
    load(&dumps(&jobj(vec![
        ("type", jstr("assistant")),
        (
            "message",
            jobj(vec![(
                "content",
                jarr(vec![jobj(vec![
                    ("type", jstr("tool_use")),
                    ("name", jstr("Bash")),
                    ("input", jobj(vec![("command", jstr(command))])),
                ])]),
            )]),
        ),
    ])))
}

fn tool_result(text: &str) -> Value {
    jobj(vec![
        ("type", jstr("user")),
        (
            "message",
            jobj(vec![(
                "content",
                jarr(vec![jobj(vec![
                    ("type", jstr("tool_result")),
                    ("content", jstr(text)),
                ])]),
            )]),
        ),
    ])
}

/// What a faked `prompt_run` was called with.
#[derive(Clone, Debug)]
struct Call {
    tmp: PathBuf,
    prompt: String,
    flags: Vec<String>,
    cwd: Option<PathBuf>,
    extra_env: Option<EnvMap>,
    variadic: Vec<String>,
}

type Calls = Rc<RefCell<Vec<Call>>>;
/// What a faked `claude_run` / `run` was asked: the arguments and the extra or full environment.
type ClaudeLog = Rc<RefCell<Vec<(Vec<String>, Option<EnvMap>)>>>;
type InstallLog = Rc<RefCell<Vec<(Vec<String>, EnvMap)>>>;

/// `fake_prompt_run(effects)`: records every call and answers with `effects(n, call)`.
fn fake_prompt_run(deps: &mut Deps, effects: impl Fn(usize, &Call) -> Proc + 'static) -> Calls {
    let calls: Calls = Rc::new(RefCell::new(Vec::new()));
    let seen = Rc::clone(&calls);
    deps.prompt_run = Box::new(move |_ctx, tmp, prompt, flags, opts| {
        let call = Call {
            tmp: tmp.to_path_buf(),
            prompt: prompt.to_string(),
            flags: flags.to_vec(),
            cwd: opts.cwd.clone(),
            extra_env: opts.extra_env.clone(),
            variadic: opts.variadic.clone(),
        };
        seen.borrow_mut().push(call.clone());
        let n = seen.borrow().len();
        Ok(effects(n, &call))
    });
    calls
}

fn nothing(_: usize, _: &Call) -> Proc {
    Proc::new(0, "", "")
}

/// `make_repo` as the Python tests patched it: just the folder `<tmp>/repo`.
fn fake_make_repo(deps: &mut Deps) {
    deps.make_repo = Box::new(|_ctx, tmp| {
        let repo = tmp.join("repo");
        fs::create_dir(&repo).map_err(|e| e.to_string())?;
        Ok(repo)
    });
}

fn flag_after<'a>(flags: &'a [String], name: &str) -> &'a str {
    let at = flags.iter().position(|f| f == name).expect("flag present");
    &flags[at + 1]
}

fn first_run(ctx: &Ctx, body: fn(&Ctx) -> Res<Outcome>) -> Outcome {
    body(ctx).expect("the smoke ends with an outcome")
}

// ---- ListAndEnvTests ------------------------------------------------------------------------

mod list_and_env_tests {
    use super::*;

    /// m-8: the twelve names, then `guard-timeout`, then the EV1b smokes.
    #[test]
    fn test_list_prints_exactly_the_twelve_names() {
        let (code, out) = run_main(&["--list"], no_auth_env(), deps());
        assert_eq!(code, 0);
        let mut want: Vec<&str> = NAMES.to_vec();
        want.push("guard-timeout");
        want.extend(EV1B_NAMES);
        assert_eq!(out.split_whitespace().collect::<Vec<_>>(), want);
    }

    #[test]
    fn test_unknown_name_is_a_usage_error() {
        let out = run_with(&args(&["no-such-smoke"]), no_auth_env(), deps());
        assert_eq!(out.code, 2);
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("unknown smoke(s): no-such-smoke"),
            "{:?}",
            out.stderr
        );
        assert!(out.stdout.is_empty());
    }

    #[test]
    fn test_env_passes_only_the_allowed_variables() {
        let fake = Env::default()
            .with("SECRET_X", "hunter2")
            .with("CLAUDE_CODE_SUBAGENT_MODEL", "opus")
            .with("ANTHROPIC_API_KEY", "sk-fake-key")
            .with("ANTHROPIC_AUTH_TOKEN", "")
            .with("ANTHROPIC_BASE_URL", "")
            .with("PATH", "/usr/bin:/bin");
        let d = tempfile::tempdir().expect("temp dir");
        let env = build_env(&fake, d.path(), None);
        assert_eq!(env["ANTHROPIC_API_KEY"], "sk-fake-key");
        assert!(!env.contains_key("SECRET_X"));
        assert!(!env.contains_key("CLAUDE_CODE_SUBAGENT_MODEL"));
        assert!(!env.contains_key("ANTHROPIC_AUTH_TOKEN")); // empty means unset
        assert_eq!(Path::new(&env["HOME"]), d.path().join("home"));
        assert_eq!(Path::new(&env["CLAUDE_CONFIG_DIR"]), d.path().join("cfg"));
        assert_eq!(env["DISABLE_AUTOUPDATER"], "1");
        assert_eq!(env["CLAUDE_CODE_SUBPROCESS_ENV_SCRUB"], "1"); // tools and hooks never see the key
        assert_eq!(env["NPM_CONFIG_OFFLINE"], "true");
        // `claude -p` reads as an embedded host: the cases that read the guide need the full start
        assert_eq!(env["ANTHOLOGY_HOST_MODE"], "full");
        assert_eq!(env["PATH"], "/usr/bin:/bin");
        let allowed: std::collections::BTreeSet<&str> = [
            "HOME",
            "CLAUDE_CONFIG_DIR",
            "PATH",
            "TMPDIR",
            "LANG",
            "TERM",
            "DISABLE_AUTOUPDATER",
            "ANTHROPIC_API_KEY",
            "CLAUDE_CODE_SUBPROCESS_ENV_SCRUB",
            "NPM_CONFIG_OFFLINE",
            "ANTHOLOGY_HOST_MODE",
        ]
        .into_iter()
        .collect();
        assert_eq!(
            env.keys()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>(),
            allowed
        );
        assert!(d.path().join("home").is_dir() && d.path().join("cfg").is_dir());
    }

    #[test]
    fn test_no_scrub_turns_the_scrub_off_and_the_default_stays_on() {
        assert_eq!(
            no_scrub(),
            EnvMap::from([(
                "CLAUDE_CODE_SUBPROCESS_ENV_SCRUB".to_string(),
                "0".to_string()
            )])
        );
        let d = tempfile::tempdir().expect("temp dir");
        let base = Env::default().with("PATH", "/usr/bin");
        assert_eq!(
            build_env(&base, d.path(), None)["CLAUDE_CODE_SUBPROCESS_ENV_SCRUB"],
            "1"
        );
        let env = build_env(&base, d.path(), Some(&no_scrub()));
        assert_eq!(env["CLAUDE_CODE_SUBPROCESS_ENV_SCRUB"], "0");
    }

    #[test]
    fn test_redact_hides_auth_values() {
        let env = Env::default().with("ANTHROPIC_API_KEY", "sk-secret-value");
        assert_eq!(redact(&env, "key=sk-secret-value!"), "key=***!");
    }
}

// ---- HelperTests ----------------------------------------------------------------------------

mod helper_tests {
    use super::*;
    use std::process::Command;

    fn write_hooks(dir: &Path, data: &str) -> PathBuf {
        let path = dir.join("hooks.json");
        fs::write(&path, data).expect("write hooks");
        path
    }

    /// A synthetic subagent transcript: one assistant message per (tool, input) pair.
    fn transcript(calls: &[(&str, Value)]) -> String {
        let mut lines: Vec<String> = calls
            .iter()
            .map(|(name, input)| {
                dumps(&jobj(vec![
                    ("type", jstr("assistant")),
                    (
                        "message",
                        jobj(vec![(
                            "content",
                            jarr(vec![
                                jobj(vec![("type", jstr("text")), ("text", jstr("working"))]),
                                jobj(vec![
                                    ("type", jstr("tool_use")),
                                    ("name", jstr(name)),
                                    ("input", input.clone()),
                                ]),
                            ]),
                        )]),
                    ),
                ]))
            })
            .collect();
        lines.push("not json".to_string());
        lines.join("\n")
    }

    fn file_input(path: &str) -> Value {
        jobj(vec![("file_path", jstr(path))])
    }

    fn pattern_input(pattern: &str) -> Value {
        jobj(vec![("pattern", jstr(pattern))])
    }

    #[test]
    fn test_cap_padding_is_exactly_n_chars_ending_in_the_marker() {
        for n in [10000usize, 10001, 9000, 100, 20] {
            let pad = cap_padding(n);
            assert_eq!(pad.chars().count(), n);
            assert!(pad.ends_with(&format!(" END-{n}")), "{n}");
        }
    }

    /// R4: the hook is POSIX sh now; it runs with a minimal environment and prints the same text.
    #[test]
    fn test_the_hook_script_prints_the_same_padding() {
        let d = tempfile::tempdir().expect("temp dir");
        let script = d.path().join("ss.sh");
        fs::write(&script, cap_hook_script()).expect("write script");
        let out = Command::new("/bin/sh")
            .arg(&script)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("CAP_N", "10001")
            .output()
            .expect("run the hook");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let doc = load(&String::from_utf8(out.stdout).expect("utf-8"));
        let payload = field(&doc, "hookSpecificOutput").expect("hookSpecificOutput");
        assert_eq!(
            field(payload, "hookEventName").and_then(Value::as_str),
            Some("SessionStart")
        );
        assert_eq!(
            field(payload, "additionalContext").and_then(Value::as_str),
            Some(cap_padding(10001).as_str())
        );
    }

    #[test]
    fn test_has_flag_is_not_a_prefix_match() {
        assert!(has_flag("  -s, --scope <scope>  Scope", "-s").unwrap());
        assert!(has_flag("--keep-data  Keep", "--keep-data").unwrap());
        assert!(!has_flag("--scope only", "-s").unwrap());
        assert!(!has_flag("--runs-per-case", "--runs").unwrap());
    }

    #[test]
    fn test_scanner_finds_sh_session_start_commands() {
        let hooks = r#"{"hooks": {
            "SessionStart": [
                {"hooks": [{"type": "command", "command": "sh", "args": ["${CLAUDE_PLUGIN_ROOT}/scripts/session-start.sh"]},
                           {"type": "command", "command": "python3", "args": ["${CLAUDE_PLUGIN_ROOT}/scripts/x.py"]}]},
                {"hooks": [{"type": "command", "command": "${CLAUDE_PLUGIN_ROOT}/scripts/other.sh --quick"}]}],
            "PreToolUse": [{"hooks": [{"type": "command", "command": "sh", "args": ["guard.sh"]}]}]}}"#;
        let d = tempfile::tempdir().expect("temp dir");
        let found = sh_session_start_commands(&write_hooks(d.path(), hooks)).unwrap();
        assert_eq!(
            found,
            vec![
                to_strings(&["sh", "${CLAUDE_PLUGIN_ROOT}/scripts/session-start.sh"]),
                to_strings(&["${CLAUDE_PLUGIN_ROOT}/scripts/other.sh", "--quick"]),
            ]
        );
    }

    #[test]
    fn test_scanner_tolerates_missing_and_odd_files() {
        let d = tempfile::tempdir().expect("temp dir");
        let scan = |p: &Path| sh_session_start_commands(p).unwrap();
        assert!(scan(&d.path().join("absent.json")).is_empty());
        assert!(scan(&write_hooks(d.path(), "not json")).is_empty());
        assert!(scan(&write_hooks(d.path(), r#"{"hooks": {"PreToolUse": []}}"#)).is_empty());
        assert!(scan(&write_hooks(d.path(), "[1, 2]")).is_empty());
        assert!(!has_session_start(&write_hooks(
            d.path(),
            r#"{"hooks": {}}"#
        )));
        assert!(has_session_start(&write_hooks(
            d.path(),
            r#"{"hooks": {"SessionStart": [{}]}}"#
        )));
    }

    #[test]
    fn test_stream_json_helpers_separate_asked_from_returned() {
        let lines = [
            r#"{"type": "system", "subtype": "hook_response", "output": "Executable not found: python9"}"#,
            r#"{"type": "assistant", "message": {"content": [{"type": "tool_use", "name": "Bash", "input": {"command": "echo RAN-OK"}}]}}"#,
            "not json at all",
            r#"{"type": "user", "message": {"content": [{"type": "tool_result", "content": "blocked by onFailure"}, {"type": "tool_result", "content": [{"type": "text", "text": "part one"}, {"type": "text", "text": "two"}]}]}}"#,
        ];
        let events = stream_events(&lines.join("\n"));
        assert_eq!(events.len(), 3);
        // Not the asked command.
        assert_eq!(
            tool_result_texts(&events),
            ["blocked by onFailure", "part one two"]
        );
        assert!(hook_failure_text(&events).contains("Executable not found"));
        assert_eq!(tool_uses(&events), ["Bash"]);
    }

    #[test]
    fn test_transcript_models_reads_both_layouts() {
        let d = tempfile::tempdir().expect("temp dir");
        let cfg = d.path();
        for rel in [
            "projects/p/sess/subagents/agent-1.jsonl",
            "projects/p/subagents/agent-2.jsonl",
        ] {
            let f = cfg.join(rel);
            fs::create_dir_all(f.parent().expect("parent")).expect("mkdir");
            fs::write(&f, "{\"message\": {\"model\": \"claude-haiku-9\"}}\nbad\n").expect("write");
        }
        let (models, files) = transcript_models(cfg).unwrap();
        assert_eq!(models, ["claude-haiku-9"; 2]);
        assert_eq!(files.len(), 2);
        assert!(
            transcript_models(Path::new("/nonexistent-dir"))
                .unwrap()
                .1
                .is_empty()
        );
    }

    #[test]
    fn test_startup_order_helper_accepts_reads_in_order_before_the_glob() {
        let text = transcript(&[
            (
                "Read",
                file_input("/x/plugins/anthology/knowledge/caveman/SKILL.md"),
            ),
            (
                "Read",
                file_input("/x/plugins/anthology/knowledge/documentation/SKILL.md"),
            ),
            (
                "Read",
                file_input(
                    "/x/plugins/anthology/knowledge/design-skills/skills/ux-writing/SKILL.md",
                ),
            ),
            ("Glob", pattern_input("*.md")),
        ]);
        let calls = transcript_calls(&text);
        assert_eq!(calls.len(), 4);
        let refs = to_strings(&["caveman", "documentation", "ds/ux-writing"]);
        assert!(startup_problems(&calls, &refs).unwrap().is_empty());
    }

    #[test]
    fn test_startup_order_helper_reports_missing_late_and_out_of_order_reads() {
        let text = transcript(&[
            // A discovery Glob is not the *.md anchor.
            ("Glob", pattern_input("**/knowledge/**")),
            ("Read", file_input("/x/knowledge/documentation/SKILL.md")),
            ("Read", file_input("/x/knowledge/caveman/SKILL.md")),
            ("Glob", pattern_input("*.md")),
            (
                "Read",
                file_input("/x/knowledge/design-skills/skills/ux-writing/SKILL.md"),
            ),
        ]);
        let refs = to_strings(&["caveman", "documentation", "ds/ux-writing", "tech-debt"]);
        let problems = startup_problems(&transcript_calls(&text), &refs).unwrap();
        assert_eq!(
            problems,
            [
                "documentation read out of order",
                "ds/ux-writing read after the Glob",
                "tech-debt never read"
            ]
        );
        let lone = [("Read".to_string(), "{}".to_string())];
        assert_eq!(
            startup_problems(&lone, &to_strings(&["caveman"])).unwrap(),
            ["no Glob of *.md"]
        );
    }

    #[test]
    fn test_startup_regex_does_not_confuse_a_design_skill_with_a_plain_one() {
        let matches =
            |text: &str, reference: &str| search_fancy(&startup_regex(reference), text).unwrap();
        assert!(matches(
            "knowledge/design-skills/skills/ux-writing/SKILL.md",
            "ds/ux-writing"
        ));
        assert!(!matches("knowledge/ux-writing/SKILL.md", "ds/ux-writing"));
        assert!(!matches("knowledge/tech-debt/SKILLxmd", "tech-debt"));
    }

    #[test]
    fn test_skill_heading_reads_the_first_h1_after_the_frontmatter() {
        assert_eq!(
            skill_heading(&repo_root(), "writing-plans").unwrap(),
            "Writing Plans"
        );
    }

    #[test]
    fn test_parse_agent_on_a_fixture() {
        let text = "---\nname: x\nmodel: opus\nisolation: worktree\nskills:\n  - anthology-superpowers:writing-plans\n\
                    \x20 - anthology-superpowers:test-driven-development\n---\n\n## Startup\nRead:\n\
                    - `${CLAUDE_PLUGIN_ROOT}/knowledge/caveman/SKILL.md`\n\
                    - `${CLAUDE_PLUGIN_ROOT}/knowledge/design-skills/skills/ux-writing/SKILL.md`\n\
                    Prose with `${CLAUDE_PLUGIN_ROOT}/knowledge/ignored/SKILL.md` inline.\n\n\
                    ## Role\n- `${CLAUDE_PLUGIN_ROOT}/knowledge/after-section/SKILL.md`\n";
        assert_eq!(
            parse_agent(text).unwrap(),
            AgentSpec {
                model: "opus".to_string(),
                worktree: true,
                preloads: to_strings(&["writing-plans", "test-driven-development"]),
                startup: to_strings(&["caveman", "ds/ux-writing"]),
            }
        );
        assert_eq!(
            parse_agent("no frontmatter\n").unwrap(),
            AgentSpec {
                model: "?".to_string(),
                worktree: false,
                preloads: Vec::new(),
                startup: Vec::new(),
            }
        );
    }

    #[test]
    fn test_parse_agent_on_a_real_agent_file() {
        let text = fs::read_to_string(repo_root().join("plugins/anthology/agents/implementer.md"))
            .expect("implementer.md");
        let agent = parse_agent(&text).unwrap();
        assert_eq!(agent.model, "sonnet");
        assert!(agent.worktree);
        assert_eq!(
            agent.preloads,
            ["test-driven-development", "verification-before-completion"]
        );
        assert_eq!(agent.startup[0], "caveman");
    }

    /// R9: the roster table is copied from tests/_repo.py (no PyYAML, no skip).
    #[test]
    fn test_the_parser_agrees_with_the_expected_roster() {
        let agents = load_agents(&repo_root().join("plugins/anthology/agents")).unwrap();
        let mut got: Vec<&str> = agents.iter().map(|(n, _)| n.as_str()).collect();
        got.sort_unstable();
        let mut want: Vec<&str> = EXPECTED_ROSTER.iter().map(|r| r.0).collect();
        want.sort_unstable();
        assert_eq!(got, want);
        assert_eq!(agents.len(), EXPECTED_AGENTS);
        for (name, model, worktree, preloads, knowledge) in EXPECTED_ROSTER {
            let spec = &agents.iter().find(|(n, _)| n == name).expect("agent").1;
            assert_eq!(spec.model, model, "{name}");
            assert_eq!(spec.worktree, worktree, "{name}");
            assert_eq!(spec.preloads, preloads, "{name}");
            let mut startup = vec!["caveman".to_string()];
            startup.extend(knowledge.iter().map(|k| (*k).to_string()));
            assert_eq!(spec.startup, startup, "{name}");
        }
    }

    #[test]
    fn test_cost_and_text_from_json_and_stream_json() {
        let one = r#"{"type": "result", "result": "hi", "total_cost_usd": 0.25}"#;
        let hi = (Some(0.25), Some("hi".to_string()));
        assert_eq!(result_cost_and_text(one), hi);
        assert_eq!(
            result_cost_and_text(&format!(r#"[{{"type": "system"}}, {one}]"#)),
            hi
        );
        assert_eq!(
            result_cost_and_text(&format!("noise\n{{\"type\": \"system\"}}\n{one}")),
            hi
        );
        assert_eq!(result_cost_and_text("not json"), (None, None));
        assert_eq!(result_cost_and_text(r#"{"type": "result"}"#), (None, None));
    }

    #[test]
    fn test_hook_failure_text_ignores_ids_and_the_not_found_patterns_are_specific() {
        let events = [load(
            r#"{"type": "system", "subtype": "hook_response", "uuid": "1277-abc", "session_id": "x127y", "output": "ok", "exit_code": 0}"#,
        )];
        assert!(!not_found(&hook_failure_text(&events)));
        let events = [load(
            r#"{"type": "system", "subtype": "hook_response", "exit_code": 127}"#,
        )];
        assert!(not_found(&hook_failure_text(&events)));
        assert!(not_found("python9: command not found"));
        assert!(blocked("Blocking because onFailure is \"block\""));
        assert!(!blocked("warning: unknown key onFailure"));
    }

    #[test]
    fn test_transcript_cwds_reads_the_cwd_field_only() {
        let text = [
            r#"{"cwd": "/w/.claude/worktrees/agent-1", "message": {}}"#,
            r#"{"message": {"content": "error: /w/.claude/worktrees/agent-2 missing"}}"#,
            "bad",
        ]
        .join("\n");
        assert_eq!(transcript_cwds(&text), ["/w/.claude/worktrees/agent-1"]);
    }

    #[test]
    fn test_read_allow_uses_the_absolute_path_form() {
        let allow = read_allow(&repo_root());
        let rule = field(field(&allow, "permissions").expect("permissions"), "allow")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(Value::as_str)
            .expect("rule")
            .to_string();
        assert!(
            rule.starts_with("Read(//") && rule.ends_with("/plugins/**)"),
            "{rule}"
        );
    }

    #[test]
    fn test_rtk_problems_name_each_failed_part() {
        let clean = "On branch main\n* No commits yet on main\nclean \u{2014} nothing to commit"
            .to_string();
        let g1 = "anthology-guardrails (G1): playwright";
        assert!(atl_problems(&[clean.clone(), "blocked".to_string()], g1).is_empty());
        // In a tool result.
        assert!(
            atl_problems(
                &[clean.clone(), "anthology-guardrails (G1)".to_string()],
                ""
            )
            .is_empty()
        );
        assert_eq!(
            atl_problems(&["Clean working tree".to_string()], "x (G1)"),
            [
                "no hook event or tool result shows 'anthology-guardrails (G1)' (the guard did not fire on the playwright command)"
            ]
        );
        let raw = "nothing to commit (create/copy files and use \"git add\" to track)".to_string();
        let problems = atl_problems(&[raw], "");
        assert_eq!(problems.len(), 3);
        assert!(problems[0].contains("condensed"));
        assert!(problems[1].contains("raw git"));
        assert!(problems[2].contains("(G1)"));
    }

    fn call(name: &str, command: &str) -> (String, String) {
        (
            name.to_string(),
            dumps(&jobj(vec![("command", jstr(command))])),
        )
    }

    fn bare(name: &str) -> (String, String) {
        (name.to_string(), "{}".to_string())
    }

    #[test]
    fn test_memory_save_problems_name_each_failed_check() {
        let note = "---\nname: x\nmetadata:\n  type: project\n---\nbody\n";
        let save = call("Bash", "anthology-mem save --name project-eval-probe");
        let check = memory_save_problems;
        assert!(
            check(
                &[bare("Agent")],
                std::slice::from_ref(&save),
                Some(note),
                &[]
            )
            .is_empty()
        );
        assert_eq!(
            check(
                &[],
                std::slice::from_ref(&save),
                Some(&note.replace("  type", "type")),
                &[]
            ),
            ["the note lacks a nested `metadata: type: project`"]
        );
        // The save must come from the subagent: the main session saving it is not enough.
        assert_eq!(
            check(
                std::slice::from_ref(&save),
                &[bare("Read")],
                Some(note),
                &[]
            ),
            ["anthology-mem save was not run by the subagent"]
        );
        // The heredoc and write-tool checks cover both sides.
        assert_eq!(
            check(
                &[bare("Write")],
                &[save.clone(), call("Bash", "cat <<EOF")],
                Some(note),
                &[]
            ),
            ["a Bash call used a heredoc", "used Write"]
        );
        let stray = ["/t/.claude/agent-memory/anthology:scout/n.md".to_string()];
        let problems = check(
            &[bare("Write")],
            &[
                call("Bash", "cat <<EOF > n.md"),
                bare("Edit"),
                bare("Write"),
            ],
            None,
            &stray,
        );
        assert_eq!(
            problems,
            [
                "anthology-mem save was not run by the subagent",
                "a Bash call used a heredoc",
                "used Edit, Write",
                "the note file was not written",
                "a path has the agent type's colon: /t/.claude/agent-memory/anthology:scout/n.md"
            ]
        );
    }

    #[test]
    fn test_calls_are_read_from_a_stream_json_run() {
        let line = dumps(&bash_call("anthology-mem save x"));
        assert_eq!(
            transcript_calls(&format!("{line}\nnot json")),
            [(
                "Bash".to_string(),
                r#"{"command": "anthology-mem save x"}"#.to_string()
            )]
        );
    }

    #[test]
    fn test_colon_memory_paths_finds_only_the_colon_folder() {
        let d = tempfile::tempdir().expect("temp dir");
        let ok = d.path().join(".claude/agent-memory/anthology-scout");
        let bad = d.path().join(".claude/agent-memory/anthology:scout");
        fs::create_dir_all(&ok).expect("mkdir");
        fs::create_dir_all(&bad).expect("mkdir");
        fs::write(bad.join("n.md"), "x").expect("write");
        let found = colon_memory_paths(d.path());
        assert_eq!(found.len(), 2); // the folder and the note in it
        assert!(
            found
                .iter()
                .all(|f| f.contains("agent-memory/anthology:scout"))
        );
    }
}

// ---- SmokeBodyTests -------------------------------------------------------------------------

/// The model-running smokes with the seams faked: assert what they pass to claude, never run a model.
mod smoke_body_tests {
    use super::*;

    #[test]
    fn test_eval_ceiling_runs_plugin_eval_unscrubbed() {
        let seen: ClaudeLog = Rc::default();
        let log = Rc::clone(&seen);
        let mut deps = deps();
        deps.claude_run = Box::new(move |_ctx, _tmp, args, opts| {
            log.borrow_mut()
                .push((args.to_vec(), opts.extra_env.clone()));
            Proc::new(2, "", "")
        });
        let ctx = ctx_with(no_auth_env(), deps);
        let (status, _) = first_run(&ctx, cases::eval_ceiling::run);
        assert_eq!(status, Status::Fail); // no result JSON came back
        let seen = seen.borrow();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0[..2], ["plugin", "eval"]);
        assert_eq!(seen[0].1, Some(no_scrub()));
    }

    /// `smoke_auto_memory_dir` with fake sessions; `session2(tmp, repo, mem)` writes what session 2 left behind.
    fn run_auto_memory_dir(session2: impl Fn(&Path, &Path, &Path) + 'static) -> (Outcome, Calls) {
        let mut deps = deps();
        let calls = fake_prompt_run(&mut deps, move |n, call| {
            let repo = call.cwd.clone().expect("cwd");
            let mem = repo.join(".claude").join("memory");
            fs::create_dir_all(&mem).expect("mkdir");
            match n {
                1 => {
                    let default = call.tmp.join("cfg/projects/p/memory");
                    fs::create_dir_all(&default).expect("mkdir");
                    fs::write(default.join("n.md"), "amber").expect("write");
                    let settings =
                        jobj(vec![("autoMemoryDirectory", jstr(&mem.to_string_lossy()))]);
                    fs::write(repo.join(".claude/settings.local.json"), dumps(&settings))
                        .expect("write");
                }
                2 => session2(&call.tmp, &repo, &mem),
                _ => fs::write(mem.join("c.md"), "cyan-circle").expect("write"),
            }
            Proc::new(0, "", "")
        });
        deps.approved_repo = Box::new(|_ctx, tmp| {
            let repo = tmp.join("repo");
            fs::create_dir(&repo).map_err(|e| e.to_string())?;
            Ok(repo)
        });
        let ctx = ctx_with(no_auth_env(), deps);
        (first_run(&ctx, cases::auto_memory_dir::run), calls)
    }

    fn personal_dir(tmp: &Path) -> PathBuf {
        tmp.join("cfg/anthology/personal-memory")
    }

    fn note_in_personal(tmp: &Path) {
        fs::create_dir_all(personal_dir(tmp)).expect("mkdir");
        fs::write(personal_dir(tmp).join("n.md"), "amber").expect("write");
    }

    #[test]
    fn test_auto_memory_dir_runs_all_three_sessions_unscrubbed() {
        // B3: the legacy note is copied to personal memory only.
        let ((status, reason), calls) =
            run_auto_memory_dir(|tmp, _repo, _mem| note_in_personal(tmp));
        assert_eq!(
            (status, calls.borrow().len()),
            (Status::Pass, 3),
            "{reason}"
        );
        for call in calls.borrow().iter() {
            assert_eq!(call.extra_env, Some(no_scrub()));
            assert_eq!(call.flags[..2], ["--permission-mode", "bypassPermissions"]);
        }
    }

    #[test]
    fn test_auto_memory_dir_fails_if_the_old_note_is_copied_into_the_repo() {
        // The pre-P1 behaviour, now a leak.
        let ((status, reason), _) = run_auto_memory_dir(|tmp, _repo, mem| {
            note_in_personal(tmp);
            fs::write(mem.join("n.md"), "amber").expect("write");
        });
        assert_eq!(status, Status::Fail);
        assert!(reason.contains("repo"), "{reason}");
    }

    #[test]
    fn test_auto_memory_dir_fails_if_the_old_note_reaches_no_personal_memory() {
        let ((status, reason), _) = run_auto_memory_dir(|_, _, _| {});
        assert_eq!(status, Status::Fail);
        assert!(reason.contains("personal memory"), "{reason}");
    }

    #[test]
    fn test_writer_memory_commit_runs_unscrubbed() {
        let mut deps = deps();
        let calls = fake_prompt_run(&mut deps, nothing);
        fake_make_repo(&mut deps);
        deps.git = Box::new(|_, _, _, _| Ok(String::new()));
        let ctx = ctx_with(no_auth_env(), deps);
        let (status, _) = first_run(&ctx, cases::writer_memory_commit::run);
        assert_eq!(status, Status::Fail); // no commit was made
        let calls = calls.borrow();
        assert_eq!(
            calls
                .iter()
                .map(|c| c.extra_env.clone())
                .collect::<Vec<_>>(),
            [Some(no_scrub())]
        );
        assert!(calls[0].flags.iter().any(|f| f == "bypassPermissions"));
    }

    /// Every model run of a smoke must carry exactly the unscrubbed environment.
    fn assert_all_unscrubbed(calls: &Calls, want_runs: usize) {
        let calls = calls.borrow();
        assert_eq!(calls.len(), want_runs, "model runs");
        for (i, c) in calls.iter().enumerate() {
            assert_eq!(c.extra_env, Some(no_scrub()), "model run {i}");
        }
    }

    /// On Linux, `CLAUDE_CODE_SUBPROCESS_ENV_SCRUB=1` makes Claude Code run its subprocesses in
    /// bubblewrap. It then cannot create an agent worktree ("Failed to resolve base branch ...")
    /// and a Bash call cannot create `.claude/` in the repo. The four smokes that need either must
    /// run with the scrub off, as `writer-memory-commit` does.
    #[test]
    fn test_worktree_base_runs_both_sessions_unscrubbed() {
        let mut deps = deps();
        let calls = fake_prompt_run(&mut deps, nothing);
        // Only `clone` has to leave something behind: the smoke writes into the clone.
        deps.git = Box::new(|_ctx, tmp, _cwd, args| {
            if args.first() == Some(&"clone") {
                fs::create_dir_all(tmp.join("repo")).map_err(|e| e.to_string())?;
            }
            Ok(String::new())
        });
        let ctx = ctx_with(no_auth_env(), deps);
        let (status, _) = first_run(&ctx, cases::worktree_base::run);
        assert_eq!(status, Status::Fail); // no agent ran, so no marker
        assert_all_unscrubbed(&calls, 2); // the baseRef=head run and the control
    }

    #[test]
    fn test_model_check_runs_every_agent_unscrubbed() {
        let mut deps = deps();
        let calls = fake_prompt_run(&mut deps, nothing);
        fake_make_repo(&mut deps);
        let ctx = ctx_with(no_auth_env(), deps);
        let (status, _) = first_run(&ctx, cases::model_check::run);
        assert_eq!(status, Status::Fail); // no transcripts
        assert_all_unscrubbed(&calls, EXPECTED_ROSTER.len());
    }

    #[test]
    fn test_knowledge_read_writers_runs_every_writer_unscrubbed() {
        let mut deps = deps();
        let calls = fake_prompt_run(&mut deps, nothing);
        fake_make_repo(&mut deps);
        let ctx = ctx_with(no_auth_env(), deps);
        let (status, _) = first_run(&ctx, cases::knowledge_read_writers::run);
        assert_eq!(status, Status::Fail); // no transcripts
        let writers = EXPECTED_ROSTER.iter().filter(|row| row.2).count();
        assert_all_unscrubbed(&calls, writers);
    }

    #[test]
    fn test_the_other_model_smokes_keep_the_scrub() {
        let mut deps = deps();
        let calls = fake_prompt_run(&mut deps, nothing);
        let ctx = ctx_with(no_auth_env(), deps);
        first_run(&ctx, cases::context_cap::run);
        let calls = calls.borrow();
        assert!(!calls.is_empty());
        assert!(calls.iter().all(|c| {
            !c.extra_env
                .as_ref()
                .is_some_and(|e| e.contains_key("CLAUDE_CODE_SUBPROCESS_ENV_SCRUB"))
        }));
    }

    /// Runs `atl-rewrite` with a faked installer (`run`) and a faked model run answering `out`.
    fn run_atl(out: &str, install_code: i32) -> (Outcome, Vec<(Vec<String>, EnvMap)>, Calls) {
        let installs: InstallLog = Rc::default();
        let log = Rc::clone(&installs);
        let mut deps = deps();
        deps.run = Box::new(move |argv, _cwd, env, _timeout, _stdin| {
            log.borrow_mut().push((argv.to_vec(), env.clone()));
            Proc::new(install_code, "", "boom: no network")
        });
        let out = out.to_string();
        let calls = fake_prompt_run(&mut deps, move |_, _| Proc::new(0, &out, ""));
        let ctx = ctx_with(no_auth_env(), deps).with_anthology(Some(FAKE_BIN.to_string()), true);
        let result = first_run(&ctx, cases::atl_rewrite::run);
        let installs = installs.borrow().clone();
        (result, installs, calls)
    }

    #[test]
    fn test_rtk_rewrite_installs_rtk_then_runs_with_the_scratch_config() {
        // legacy: port/ledger names this test
        let out = stream(&[
            bash_call("git -C atlrepo status"),
            tool_result("* No commits yet on main\nclean \u{2014} nothing to commit"),
            tool_result("blocked: anthology-guardrails (G1) ..."),
        ]);
        let ((status, reason), installs, calls) = run_atl(&out, 0);
        assert_eq!(status, Status::Pass, "{reason}");
        let (argv, env) = &installs[0];
        // EV1b: the Rust installer, `anthology atl-install`, with the staged plugin's atl.lock.
        assert_eq!(argv[0], FAKE_BIN);
        assert_eq!(argv[1..3], ["atl-install", "--data-dir"]);
        assert!(argv[3].ends_with("data"));
        assert_eq!(
            argv[4..],
            [
                "--lock".to_string(),
                repo_root()
                    .join("plugins/anthology/atl.lock")
                    .to_string_lossy()
                    .into_owned()
            ]
        );
        let cfg = env["CLAUDE_CONFIG_DIR"].clone();
        assert!(cfg.ends_with("home/.claude"), "{cfg}");
        assert_eq!(env["CLAUDE_CODE_SUBPROCESS_ENV_SCRUB"], "1"); // keep the default scrub
        let calls = calls.borrow();
        let call = &calls[0];
        assert_eq!(
            call.extra_env,
            Some(EnvMap::from([("CLAUDE_CONFIG_DIR".to_string(), cfg)]))
        );
        assert_eq!(
            call.cwd.as_ref().and_then(|c| c.file_name()),
            Some(std::ffi::OsStr::new("ws"))
        );
        assert_eq!(call.flags[..2], ["--model", "haiku"]);
        for flag in ["--include-hook-events", "--verbose"] {
            assert!(call.flags.iter().any(|f| f == flag), "{flag}");
        }
        assert_eq!(flag_after(&call.flags, "--max-turns"), "8");
        assert_eq!(flag_after(&call.flags, "--output-format"), "stream-json");
        assert_eq!(
            call.variadic,
            [
                "Bash(git init -q atlrepo)",
                "Bash(git -C atlrepo status)",
                "Bash(atl git -C atlrepo status)",
                "Bash(npx playwright test --list)",
                "Bash(atl playwright test --list)"
            ]
        );
    }

    #[test]
    fn test_rtk_rewrite_fails_loudly_when_the_install_fails_and_names_failed_parts() {
        let ((status, reason), _, calls) = run_atl("", 1);
        assert_eq!(status, Status::Fail);
        assert!(reason.starts_with("atl install failed: boom"), "{reason}");
        assert!(calls.borrow().is_empty());
        let out = stream(&[tool_result(
            "nothing to commit (create/copy files and use git add)",
        )]);
        let ((status, reason), _, _) = run_atl(&out, 0);
        assert_eq!(status, Status::Fail);
        for part in ["condensed", "raw git", "(G1)"] {
            assert!(reason.contains(part), "{reason}");
        }
    }

    #[test]
    fn test_agent_memory_path_checks_the_stream_and_the_note() {
        let note = "---\nname: project-eval-probe\nmetadata:\n  type: project\n---\nbody\n";
        // A fresh run of the smoke whose model run leaves `text` as the note and the given commands.
        let run = |text: &'static str, sub_command: &'static str, main_command: &'static str| {
            let mut deps = deps();
            fake_make_repo(&mut deps);
            let calls = fake_prompt_run(&mut deps, move |_, call| {
                let folder = call
                    .cwd
                    .as_ref()
                    .expect("cwd")
                    .join(".claude/agent-memory/anthology-scout");
                fs::create_dir_all(&folder).expect("mkdir");
                fs::write(folder.join("project-eval-probe.md"), text).expect("write");
                let sub = call.tmp.join("cfg/projects/p/sess/subagents/agent-1.jsonl");
                fs::create_dir_all(sub.parent().expect("parent")).expect("mkdir");
                fs::write(&sub, stream(&[bash_call(sub_command)])).expect("write");
                Proc::new(0, &stream(&[bash_call(main_command)]), "")
            });
            let ctx = ctx_with(no_auth_env(), deps);
            (first_run(&ctx, cases::agent_memory_path::run), calls)
        };
        let save = "anthology-mem save --type project";
        let ((status, reason), calls) = run(note, save, "echo main");
        assert_eq!(status, Status::Pass, "{reason}");
        let call = calls.borrow()[0].clone();
        // The scrub is off: on Linux it runs Bash in bubblewrap with the repo's `.claude` read-only.
        assert_eq!(call.extra_env, Some(no_scrub()));
        assert_eq!(call.flags[..2], ["--model", "sonnet"]);
        assert_eq!(flag_after(&call.flags, "--max-turns"), "12");
        assert!(call.flags.iter().any(|f| f == "stream-json"));
        assert_eq!(
            call.variadic,
            [
                "Agent",
                "Read",
                "Glob",
                "Grep",
                "Bash(anthology-mem save *)"
            ]
        );
        assert!(call.prompt.contains("anthology:scout"));
        let ((status, reason), _) = run("no metadata", "cat <<EOF > n.md", "echo main");
        assert_eq!(status, Status::Fail);
        for part in [
            "anthology-mem save was not run by the subagent",
            "heredoc",
            "nested",
        ] {
            assert!(reason.contains(part), "{reason}");
        }
        // Only the main session saved: the subagent did not.
        let ((status, reason), _) = run(note, "echo sub", save);
        assert_eq!(status, Status::Fail);
        assert!(
            reason.contains("anthology-mem save was not run by the subagent"),
            "{reason}"
        );
    }
}

// ---- MainWithStubTests ----------------------------------------------------------------------

/// `main()` driven with a stub `claude`: an `sh` script (macOS and Linux both have /bin/sh).
mod main_with_stub_tests {
    use super::*;

    struct Fixture {
        _dir: tempfile::TempDir,
        tmp: PathBuf,
        stub: PathBuf,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().expect("temp dir");
        let tmp = fs::canonicalize(dir.path()).expect("canonical");
        let stub = tmp.join("claude");
        write_exec(&stub, "#!/bin/sh\nexit 0\n"); // writes nothing
        Fixture {
            _dir: dir,
            tmp,
            stub,
        }
    }

    fn s(p: &Path) -> String {
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn test_model_smokes_skip_without_credentials() {
        let f = fixture();
        let out_path = f.tmp.join("out").join("smoke.json");
        let mut argv = vec![
            "--claude".to_string(),
            s(&f.stub),
            "--out".to_string(),
            s(&out_path),
        ];
        argv.extend(MODEL_SMOKES.iter().map(|n| (*n).to_string()));
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        let (code, out) = run_main(&argv, no_auth_env(), deps());
        assert_eq!(code, 0);
        let summary = load(&fs::read_to_string(&out_path).expect("summary"));
        let top = summary.as_object().expect("object");
        assert_eq!(top.iter().map(|(k, _)| k).collect::<Vec<_>>(), ["smokes"]);
        let smokes = field(&summary, "smokes")
            .and_then(Value::as_object)
            .expect("smokes");
        assert_eq!(
            smokes.iter().map(|(k, _)| k).collect::<Vec<_>>(),
            MODEL_SMOKES
        );
        for (name, result) in smokes.iter() {
            assert_eq!(
                result,
                &jobj(vec![("status", jstr("SKIP")), ("reason", jstr(NO_AUTH))]),
                "{name}"
            );
        }
        let lines: Vec<&str> = out.trim().lines().collect();
        assert_eq!(lines.len(), MODEL_SMOKES.len() + 1);
        assert!(
            lines[..lines.len() - 1]
                .iter()
                .all(|l| l.starts_with("SKIP "))
        );
        assert_eq!(lines[lines.len() - 1], "smoke spend: $0.00 of $10.00");
        // m-8: the new guard-timeout smoke needs credentials too.
        let (code, out) = run_main(
            &["--claude", &s(&f.stub), "guard-timeout"],
            no_auth_env(),
            deps(),
        );
        assert_eq!(code, 0);
        assert_eq!(
            out.lines().next(),
            Some(format!("SKIP guard-timeout: {NO_AUTH}").as_str())
        );
    }

    #[test]
    fn test_a_failing_smoke_exits_1_and_is_summarised() {
        let f = fixture();
        let out_path = f.tmp.join("smoke.json");
        // The stub knows no subcommand flags, so cli-syntax FAILs.
        let (code, out) = run_main(
            &[
                "--claude",
                &s(&f.stub),
                "--out",
                &s(&out_path),
                "cli-syntax",
            ],
            no_auth_env(),
            deps(),
        );
        assert_eq!(code, 1);
        let summary = load(&fs::read_to_string(&out_path).expect("summary"));
        let result =
            field(field(&summary, "smokes").expect("smokes"), "cli-syntax").expect("result");
        assert_eq!(
            field(result, "status").and_then(Value::as_str),
            Some("FAIL")
        );
        assert!(
            field(result, "reason")
                .and_then(Value::as_str)
                .expect("reason")
                .contains("lacks")
        );
        assert!(out.starts_with("FAIL cli-syntax: "));
    }

    #[test]
    fn test_a_missing_binary_fails_when_named_and_skips_only_by_default() {
        let f = fixture();
        let absent = s(&f.tmp.join("absent"));
        let (code, out) = run_main(&["--claude", &absent, "cli-syntax"], no_auth_env(), deps());
        assert_eq!(code, 1);
        assert!(out.starts_with("FAIL cli-syntax: claude binary not found"));
        let (code, _) = run_main(
            &["cli-syntax"],
            no_auth_env().with("CLAUDE_BIN", &absent),
            deps(),
        );
        assert_eq!(code, 1);
        let mut default_deps = deps();
        let fallback = absent.clone();
        default_deps.default_claude = Box::new(move |_env, _root| fallback.clone());
        let (code, out) = run_main(&["cli-syntax"], no_auth_env(), default_deps);
        assert_eq!(code, 0);
        assert!(out.starts_with("SKIP cli-syntax: claude binary not found"));
    }

    #[test]
    fn test_budgets_are_validated() {
        let cases: [&[&str]; 5] = [
            &["--total-budget-usd", "0"],
            &["--total-budget-usd", "abc"],
            &["--budget-usd", "abc"],
            &["--budget-usd", "-1"],
            &["--budget-usd", "nan"],
        ];
        for case in cases {
            let mut argv = case.to_vec();
            argv.push("cli-syntax");
            let out = run_with(&args(&argv), no_auth_env(), deps());
            assert_eq!(out.code, 2, "{case:?}");
        }
    }

    fn reporting_stub(f: &Fixture, cost: Option<f64>) -> String {
        let stub = f.tmp.join("reporter");
        let body = match cost {
            Some(c) => format!(r#"{{"type": "result", "result": "hi", "total_cost_usd": {c}}}"#),
            None => r#"{"type": "result"}"#.to_string(),
        };
        write_exec(&stub, &format!("#!/bin/sh\necho '{body}'\n"));
        s(&stub)
    }

    #[test]
    fn test_reported_costs_are_summed_and_stop_the_launches() {
        let f = fixture();
        let ctx = Ctx::new(
            &reporting_stub(&f, Some(0.4)),
            "1.00",
            1.0,
            no_auth_env(),
            deps(),
        );
        for _ in 0..3 {
            let p = ctx
                .prompt_run(&f.tmp, "x", &[], &PromptOpts::default())
                .expect("a run");
            // A failed spawn is `Proc(-1, "", "could not run …")`: show why, not just `""`.
            assert_eq!(p.out, "hi", "exit {}: {}", p.code, p.err);
        }
        assert!((ctx.spent() - 1.2).abs() < 1e-7, "{}", ctx.spent());
        assert_eq!(
            ctx.prompt_run(&f.tmp, "x", &[], &PromptOpts::default()),
            Err(Abort::Budget)
        );
        let (status, reason) = run_smoke("cli-syntax", &ctx, true);
        assert_eq!(status, Status::Fail);
        assert!(reason.contains("smoke budget exhausted"), "{reason}");
    }

    #[test]
    fn test_an_unreported_cost_counts_as_the_per_run_budget() {
        let f = fixture();
        let ctx = Ctx::new(
            &reporting_stub(&f, None),
            "0.75",
            10.0,
            no_auth_env(),
            deps(),
        );
        ctx.prompt_run(&f.tmp, "x", &[], &PromptOpts::default())
            .expect("a run");
        assert!((ctx.spent() - 0.75).abs() < 1e-7, "{}", ctx.spent());
    }

    #[test]
    fn test_a_smoke_that_hits_the_budget_mid_way_fails() {
        let f = fixture();
        let env = no_auth_env().with("ANTHROPIC_API_KEY", "sk-fake-key");
        let ctx = Ctx::new(&reporting_stub(&f, Some(2.0)), "1.00", 1.0, env, deps());
        // The first run costs 2.0, so the second cannot start.
        let (status, reason) = run_smoke("context-cap", &ctx, true);
        assert_eq!(status, Status::Fail);
        assert!(reason.contains("smoke budget exhausted"), "{reason}");
    }

    #[test]
    fn test_python_missing_passes_against_the_real_session_start_wrapper() {
        let f = fixture();
        let (code, out) = run_main(
            &["--claude", &s(&f.stub), "python-missing"],
            no_auth_env(),
            deps(),
        );
        if repo_root().join("plugins/anthology/scripts").is_dir() {
            assert!(
                out.starts_with("PASS python-missing: 1 wrapper(s) warn about the missing python3"),
                "{out}"
            );
        } else {
            // After CO2 deleted the Python plugin scripts.
            assert!(
                out.starts_with("SKIP python-missing: no Python plugin scripts (after CO2)"),
                "{out}"
            );
        }
        assert_eq!(code, 0);
    }

    #[test]
    fn test_python_missing_skips_once_the_python_plugin_scripts_are_gone() {
        let f = fixture();
        let tree = tempfile::tempdir().expect("temp dir");
        let root = fs::canonicalize(tree.path()).expect("canonical");
        // The post-cutover layout: the sh SessionStart hook runs the Rust shim.
        let hooks = root.join("plugins/anthology/hooks");
        fs::create_dir_all(&hooks).expect("mkdir");
        fs::write(
            hooks.join("hooks.json"),
            r#"{"hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": "sh", "args": ["${CLAUDE_PLUGIN_ROOT}/hooks/session-start.sh"]}]}]}}"#,
        )
        .expect("write");
        fs::write(hooks.join("session-start.sh"), "#!/bin/sh\nexit 0\n").expect("write");
        let (code, out) = run_main(
            &["--claude", &s(&f.stub), "python-missing"],
            no_auth_env(),
            deps_at(&root),
        );
        assert!(
            out.starts_with("SKIP python-missing: no Python plugin scripts (after CO2)"),
            "{out}"
        );
        assert_eq!(code, 0);
    }

    fn make_tree(tmp: &Path, script: Option<&str>) {
        let scripts = tmp.join("plugins/anthology/scripts");
        let hooks = tmp.join("plugins/anthology/hooks");
        fs::create_dir_all(&scripts).expect("mkdir");
        fs::create_dir_all(&hooks).expect("mkdir");
        fs::write(
            hooks.join("hooks.json"),
            r#"{"hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": "sh", "args": ["${CLAUDE_PLUGIN_ROOT}/scripts/session-start.sh"]}]}]}}"#,
        )
        .expect("write");
        if let Some(script) = script {
            fs::write(scripts.join("session-start.sh"), script).expect("write");
        }
    }

    fn run_python_missing_in(f: &Fixture, script: &str) -> (i32, String) {
        let tree = tempfile::tempdir().expect("temp dir");
        let root = fs::canonicalize(tree.path()).expect("canonical");
        make_tree(&root, Some(script));
        run_main(
            &["--claude", &s(&f.stub), "python-missing"],
            no_auth_env(),
            deps_at(&root),
        )
    }

    #[test]
    fn test_python_missing_skips_when_the_wrapper_is_absent() {
        let f = fixture();
        let tree = tempfile::tempdir().expect("temp dir");
        let root = fs::canonicalize(tree.path()).expect("canonical");
        let (code, out) = run_main(
            &["--claude", &s(&f.stub), "python-missing"],
            no_auth_env(),
            deps_at(&root),
        );
        assert!(out.starts_with("SKIP python-missing: "), "{out}");
        assert_eq!(code, 0);
    }

    #[test]
    fn test_python_missing_fails_when_the_wrapper_prints_nothing() {
        let f = fixture();
        let (code, out) = run_python_missing_in(&f, "#!/bin/sh\nexit 0\n");
        assert!(out.starts_with("FAIL python-missing: "), "{out}");
        assert_ne!(code, 0);
    }

    #[test]
    fn test_python_missing_fails_when_the_wrapper_exits_nonzero() {
        let f = fixture();
        let warn = r#"printf '%s\n' '{"systemMessage":"python3 is missing"}'"#;
        let (code, out) = run_python_missing_in(&f, &format!("#!/bin/sh\n{warn}\nexit 1\n"));
        let first = out.lines().next().unwrap_or_default();
        assert!(first.starts_with("FAIL python-missing: "), "{out}");
        assert!(first.contains("exit 1"), "{out}");
        assert_ne!(code, 0);
    }
}

// ---- Rust-only tests (not in the ledger) ------------------------------------------------------

/// The guard-timeout smoke (m-8) and its pure helper, against the wording Claude Code 2.1.311
/// really printed for a `sleep 30` hook with `timeout: 5` and `onFailure: "block"`.
mod guard_timeout_extra {
    use super::*;

    const BLOCKED_RESULT: &str = "PreToolUse:Bash hook error: [sleep 30]: timed out; blocking because onFailure is \"block\"";

    /// The stream-json of that real run, shortened to the events the smoke reads.
    fn real_stream() -> String {
        [
            r#"{"type":"system","subtype":"init","cwd":"/t","tools":["Bash"]}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"echo RAN-OK","description":"Print RAN-OK marker"}}]}}"#,
            r#"{"type":"system","subtype":"hook_started","hook_id":"a8","hook_name":"PreToolUse:Bash","hook_event":"PreToolUse"}"#,
            r#"{"type":"system","subtype":"hook_response","hook_id":"a8","hook_name":"PreToolUse:Bash","hook_event":"PreToolUse","output":"","stdout":"","stderr":"","exit_code":1,"outcome":"cancelled"}"#,
            &format!(
                r#"{{"type":"user","message":{{"content":[{{"type":"tool_result","content":{},"is_error":true,"tool_use_id":"toolu_1"}}]}}}}"#,
                dumps(&jstr(BLOCKED_RESULT))
            ),
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"The hook timed out, so echo RAN-OK never ran."}]}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"result":"The hook timed out, so echo RAN-OK never ran.","total_cost_usd":0.0025}"#,
        ]
        .join("\n")
    }

    fn problems_of(stream_text: &str) -> Vec<String> {
        let events = stream_events(stream_text);
        guard_timeout_problems(
            tool_uses(&events).iter().any(|t| t == "Bash"),
            &hook_failure_text(&events),
            &tool_result_texts(&events),
        )
    }

    #[test]
    fn the_real_wording_passes() {
        assert!(problems_of(&real_stream()).is_empty());
        // The model's own text mentions RAN-OK; only tool results count.
        assert!(real_stream().contains("never ran"));
    }

    #[test]
    fn a_command_that_ran_is_reported() {
        let ran = real_stream().replace(
            "PreToolUse:Bash hook error: [sleep 30]: timed out; blocking because onFailure is \\\"block\\\"",
            "RAN-OK",
        );
        let problems = problems_of(&ran);
        assert!(
            problems.iter().any(|p| p.contains("RAN-OK")),
            "{problems:?}"
        );
        assert!(
            problems.iter().any(|p| p.contains("blocking")),
            "{problems:?}"
        );
        assert!(
            problems.iter().any(|p| p.contains("timed out")),
            "{problems:?}"
        );
    }

    #[test]
    fn a_block_that_is_not_a_timeout_is_reported() {
        let other = real_stream().replace("timed out", "failed");
        assert_eq!(
            problems_of(&other),
            ["'timed out' not seen in the hook events or tool results"]
        );
    }

    #[test]
    fn a_timeout_without_the_block_is_reported() {
        let open =
            real_stream().replace("blocking because onFailure is \\\"block\\\"", "continuing");
        assert_eq!(
            problems_of(&open),
            ["'blocking because onFailure is \"block\"' not seen"]
        );
    }

    #[test]
    fn a_run_that_never_called_bash_is_reported() {
        let no_bash = real_stream().replace("\"name\":\"Bash\"", "\"name\":\"Read\"");
        assert_eq!(problems_of(&no_bash), ["Bash was never called"]);
    }

    #[test]
    fn the_two_phrases_must_come_from_the_same_text() {
        let problems = guard_timeout_problems(
            true,
            "",
            &[
                "hook timed out".to_string(),
                "Blocking because onFailure is \"block\"".to_string(),
            ],
        );
        assert_eq!(
            problems,
            ["'timed out' and the block are not in the same hook text or tool result"]
        );
    }

    #[test]
    fn an_empty_stream_reports_three_problems() {
        assert_eq!(problems_of("").len(), 3);
    }

    #[test]
    fn the_wording_is_matched_ignoring_case() {
        let problems = guard_timeout_problems(
            true,
            "",
            &["Hook TIMED OUT; Blocking because onFailure is \"block\"".to_string()],
        );
        assert!(problems.is_empty(), "{problems:?}");
    }

    /// The smoke writes the probe plugin with the one hook, and passes `claude` the flags of
    /// missing-interpreter; a fake model run then shows the verdict.
    fn run_guard_timeout(answer: String) -> (Outcome, Calls, Rc<RefCell<String>>) {
        let hooks_seen: Rc<RefCell<String>> = Rc::default();
        let log = Rc::clone(&hooks_seen);
        let mut deps = deps();
        let calls = fake_prompt_run(&mut deps, move |_, call| {
            let probe = flag_after(&call.flags, "--plugin-dir");
            *log.borrow_mut() =
                fs::read_to_string(Path::new(probe).join("hooks/hooks.json")).expect("hooks.json");
            Proc::new(0, &answer, "")
        });
        let ctx = ctx_with(no_auth_env(), deps);
        (
            first_run(&ctx, cases::guard_timeout::run),
            calls,
            hooks_seen,
        )
    }

    #[test]
    fn the_smoke_passes_on_the_real_wording_and_writes_the_probe_hook() {
        let ((status, reason), calls, hooks) = run_guard_timeout(real_stream());
        assert_eq!(status, Status::Pass, "{reason}");
        assert_eq!(
            *hooks.borrow(),
            r#"{"hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "sleep", "args": ["30"], "timeout": 5, "onFailure": "block"}]}]}}"#
        );
        let calls = calls.borrow();
        assert_eq!(calls.len(), 1);
        let call = &calls[0];
        assert_eq!(call.prompt, "Run this Bash command once: echo RAN-OK");
        assert_eq!(call.flags[..2], ["--model", "haiku"]);
        assert_eq!(flag_after(&call.flags, "--output-format"), "stream-json");
        assert_eq!(flag_after(&call.flags, "--max-turns"), "3");
        for flag in ["--verbose", "--include-hook-events"] {
            assert!(call.flags.iter().any(|f| f == flag), "{flag}");
        }
        assert_eq!(call.variadic, ["Bash(echo *)"]);
        assert_eq!(call.extra_env, None);
    }

    #[test]
    fn the_smoke_fails_in_one_line_when_the_command_ran() {
        let ran =
            r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"RAN-OK"}]}}"#;
        let ((status, reason), _, _) = run_guard_timeout(ran.to_string());
        assert_eq!(status, Status::Fail);
        assert!(
            !reason.contains('\n') && reason.contains("RAN-OK is in a tool result"),
            "{reason}"
        );
    }
}

/// Pieces of the framework the Python tests did not reach.
mod framework_extra {
    use super::*;

    #[test]
    fn the_summary_file_is_byte_identical_to_json_dumps_indent_2() {
        let dir = tempfile::tempdir().expect("temp dir");
        let out_path = dir.path().join("new/dir/smoke.json");
        let stub = dir.path().join("claude");
        write_exec(&stub, "#!/bin/sh\nexit 0\n");
        let (code, _) = run_main(
            &[
                "--claude",
                &stub.to_string_lossy(),
                "--out",
                &out_path.to_string_lossy(),
                "guard-timeout",
            ],
            no_auth_env(),
            deps(),
        );
        assert_eq!(code, 0);
        let want = "{\n  \"smokes\": {\n    \"guard-timeout\": {\n      \"status\": \"SKIP\",\n      \"reason\": \"no API credentials (set ANTHROPIC_API_KEY or ANTHROPIC_AUTH_TOKEN)\"\n    }\n  }\n}\n";
        assert_eq!(fs::read_to_string(&out_path).expect("summary"), want);
    }

    #[test]
    fn a_failing_smoke_summary_keeps_the_names_in_run_order() {
        let dir = tempfile::tempdir().expect("temp dir");
        let out_path = dir.path().join("smoke.json");
        let (code, out) = run_main(
            &[
                "--claude",
                "/definitely/not/here",
                "--out",
                &out_path.to_string_lossy(),
                "guard-timeout",
                "cli-syntax",
            ],
            no_auth_env(),
            deps(),
        );
        assert_eq!(code, 1);
        assert_eq!(out.lines().count(), 3);
        let text = fs::read_to_string(&out_path).expect("summary");
        assert!(
            text.find("guard-timeout").expect("first") < text.find("cli-syntax").expect("second")
        );
    }

    #[test]
    fn argparse_rules_are_kept() {
        let code = |argv: &[&str]| run_with(&args(argv), no_auth_env(), deps()).code;
        assert_eq!(code(&["--list"]), 0);
        assert_eq!(code(&["--li"]), 0); // a unique prefix
        assert_eq!(code(&["--out"]), 2); // a missing value
        assert_eq!(code(&["--out", "--list"]), 2); // an option is not a value
        assert_eq!(code(&["--f1-registration-probe"]), 2);
        assert_eq!(code(&["--list", "--budget-usd", "abc"]), 0); // --list comes first
        assert_eq!(code(&["--help"]), 0);
        // Positionals are one group: a name after an option is "unrecognized".
        assert_eq!(code(&["cli-syntax", "--list", "guard-timeout"]), 2);
    }

    #[test]
    fn a_hung_process_is_killed_at_the_timeout() {
        let env = EnvMap::from([("PATH".to_string(), "/usr/bin:/bin".to_string())]);
        let p = run_process(&to_strings(&["sleep", "30"]), Path::new("/"), &env, 1, None);
        assert_eq!(
            (p.code, p.out.as_str(), p.err.as_str()),
            (-1, "", "timed out after 1s")
        );
    }

    #[test]
    fn a_missing_program_is_reported_by_its_python_class() {
        let env = EnvMap::new();
        let p = run_process(
            &to_strings(&["/no/such/program"]),
            Path::new("/"),
            &env,
            5,
            None,
        );
        assert_eq!(
            (p.code, p.err.as_str()),
            (-1, "could not run /no/such/program: FileNotFoundError")
        );
    }

    /// A file that is still open for writing cannot be executed (`ETXTBSY`). In a threaded test
    /// binary another thread's fork inherits a just-closed write fd until its own exec, so
    /// running a freshly written stub can hit this for a moment. Holding the fd here makes the
    /// window deterministic. Linux only: macOS runs a file that is open for writing.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_stub_busy_for_a_moment_is_run_not_reported_as_empty_output() {
        let dir = tempfile::tempdir().expect("temp dir");
        let stub = dir.path().join("stub");
        write_exec(&stub, "#!/bin/sh\necho ok\n");
        let writer = fs::OpenOptions::new()
            .write(true)
            .open(&stub)
            .expect("open for writing");
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(150));
            drop(writer);
        });
        let argv = to_strings(&[&stub.to_string_lossy()]);
        let p = run_process(&argv, dir.path(), &EnvMap::new(), 10, None);
        release.join().expect("release thread");
        assert_eq!((p.code, p.out.as_str()), (0, "ok\n"), "{}", p.err);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_stub_that_stays_busy_is_an_error_not_empty_output() {
        let dir = tempfile::tempdir().expect("temp dir");
        let stub = dir.path().join("stub");
        write_exec(&stub, "#!/bin/sh\necho ok\n");
        let _writer = fs::OpenOptions::new()
            .write(true)
            .open(&stub)
            .expect("open for writing");
        let argv = to_strings(&[&stub.to_string_lossy()]);
        let p = run_process(&argv, dir.path(), &EnvMap::new(), 10, None);
        assert_eq!(p.code, -1);
        assert!(p.err.starts_with("could not run "), "{}", p.err);
    }

    #[test]
    fn stdin_and_both_streams_reach_the_caller() {
        let env = EnvMap::from([("PATH".to_string(), "/usr/bin:/bin".to_string())]);
        let argv = to_strings(&["sh", "-c", "cat; echo err >&2; exit 3"]);
        let p = run_process(&argv, Path::new("/"), &env, 10, Some("{}"));
        assert_eq!((p.code, p.out.as_str(), p.err.as_str()), (3, "{}", "err\n"));
    }

    #[test]
    fn the_hook_script_cuts_exactly_where_python_does() {
        for n in [10000usize, 9000, 100, 20] {
            let d = tempfile::tempdir().expect("temp dir");
            let script = d.path().join("ss.sh");
            fs::write(&script, cap_hook_script()).expect("write");
            let out = std::process::Command::new("/bin/sh")
                .arg(&script)
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env("CAP_N", n.to_string())
                .output()
                .expect("run");
            let doc = load(&String::from_utf8(out.stdout).expect("utf-8"));
            let text = field(
                field(&doc, "hookSpecificOutput").expect("payload"),
                "additionalContext",
            )
            .and_then(Value::as_str)
            .expect("text")
            .to_string();
            assert_eq!(text, cap_padding(n), "{n}");
        }
    }

    #[test]
    fn the_context_cap_hook_does_not_need_python() {
        let script = cap_hook_script();
        assert!(script.starts_with("#!/bin/sh\n") && !script.contains("python"));
    }

    #[test]
    fn the_root_seam_finds_this_checkout() {
        let root = Deps::default();
        let found = (root.root)().expect("run from inside the checkout");
        assert!(
            found
                .join("plugins/anthology/.claude-plugin/plugin.json")
                .is_file()
        );
        assert_eq!(smoke::names().len(), 19);
    }
}

/// Review fixes of EV1a (m-2 to m-5; m-1, a process-group kill, is NOT done: it would orphan `claude` on Ctrl-C): Rust-only tests of the framework, not in the ledger.
mod review_fixes {
    use super::*;

    #[test]
    fn help_stops_parsing_and_prints_the_options() {
        let (code, out) = run_main(&["-h", "--total-budget-usd", "abc"], no_auth_env(), deps());
        assert_eq!(code, 0);
        assert!(out.contains("options:") && out.contains("--list"), "{out}");
        assert!(
            out.contains("\n  --claude CLAUDE       claude binary"),
            "{out}"
        );
    }

    #[test]
    fn an_empty_out_path_writes_nothing_and_exits_0() {
        let dir = tempfile::tempdir().expect("temp dir");
        let stub = dir.path().join("claude");
        write_exec(&stub, "#!/bin/sh\nexit 0\n");
        let stub = stub.to_string_lossy().into_owned();
        let (code, out) = run_main(
            &["--claude", &stub, "--out=", "guard-timeout"],
            no_auth_env(),
            deps(),
        );
        assert_eq!(code, 0, "{out}");
    }

    #[test]
    fn the_temp_dir_comes_from_the_given_env_only() {
        let dir = tempfile::tempdir().expect("temp dir");
        let bare = Env::default().with("PATH", "/usr/bin");
        assert_eq!(build_env(&bare, dir.path(), None)["TMPDIR"], "/tmp");
        let tmp = bare.with("TMP", "/var/tmp");
        assert_eq!(build_env(&tmp, dir.path(), None)["TMPDIR"], "/var/tmp");
    }

    #[test]
    fn a_relative_claude_path_is_made_absolute_through_the_cwd_seam() {
        let mut d = deps();
        d.cwd = Box::new(|| PathBuf::from("/seam/dir"));
        let (code, out) = run_main(
            &["--claude", "./no-such-claude", "cli-syntax"],
            no_auth_env(),
            d,
        );
        assert_eq!(code, 1);
        assert!(
            out.contains("claude binary not found: /seam/dir/no-such-claude"),
            "{out}"
        );
    }
}
