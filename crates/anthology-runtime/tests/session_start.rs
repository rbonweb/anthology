//! Port of `tests/test_session_start.py` (RT3), plus `test_guardrails_loc`'s
//! `SelfTestFollowsTest` rows, and Rust tests for what the port adds (GC-7 time rules, F-4,
//! the `<ah>/bin` prune).
//!
//! Modules are the Python classes; every ported test keeps the Python method name. Python's
//! `mock.patch` sites go through `SessionDeps` (GC-19): a step patched with a `side_effect` is
//! `SessionDeps::fail`, `atl_install` is a fake `AtlWiring`, `memory_sync.run` is
//! `memory_run`. The layout is `anthology_testkit::session::SessionCase` (its module docs say
//! why it is a small copy of the plugin with a stand-in guard). The budget tests sit in modules
//! named `budget` so `cargo test -p anthology-runtime budget` selects them.
//!
//! The memory step is the real `anthology_memory::sync::run` (MS2). Cases that need a repo
//! (MemoryPrivacyTest, and the repo-cover and hint cases) skip when git is missing, as Python's
//! `needs_git` does. The real-guard cases are in `crates/anthology-cli/tests/session_start.rs`.
//!
//! Ledger exceptions (port/ledger/test_session_start.tsv): the python3 and `session-start.sh`
//! source tests are `dropped:superseded:F-3`, hooks.json is `dropped:superseded:F-2`, the
//! python3-on-PATH and sample-loading self-test cases are `dropped:superseded:F-7`.
#![forbid(unsafe_code)]

use std::cell::RefCell;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anthology_core::git::RealGit;
use anthology_core::lock::Lock;
use anthology_core::{Env, state};
use anthology_pycompat::json::{self, Map, Value};
use anthology_pycompat::pyerr::PyExc;
use anthology_runtime::atl_install::{self, AtlDeps, Reconciled};
use anthology_runtime::bootstrap::GuardBinaryStatus;
use anthology_runtime::session_start::{
    self, ATL_MIN_REMAINING, ATL_TIMEOUT, AtlWiring, DEADLINE, MOVED, NAG, OPS_CHANGED, RunResult,
    RunSpec, SessionDeps, SessionLimits,
};
use anthology_settings::{self as settings, Op, SettingsDeps, TAG_PLUGIN_READ};
use anthology_testkit::mem::{Note, needs_git, write};
use anthology_testkit::session::{
    BLOCKS, EXIT_0, SessionCase, copy_bin, guard_plugin, guide_path, real_plugin, write_mode,
};

const DATA_LINE: &str = "anthology: Claude Code did not pass CLAUDE_PLUGIN_DATA to the hook; atl and the memory search index have no data folder (compatibility check)";
const MODEL_LINE: &str = "anthology: CLAUDE_CODE_SUBAGENT_MODEL is set, so every agent runs on that model and the roster's model tiers do not apply; run /anthology:setup to see where it is set";
const AGENTS: [&str; 11] = [
    "researcher",
    "architect",
    "implementer",
    "debugger",
    "reviewer",
    "scout",
    "docs-writer",
    "ui-designer",
    "ui-reviewer",
    "product-planner",
    "marketer",
];

// --- the fixture --------------------------------------------------------------------------

/// `FakeAtl`: atl_install's interface; records the calls.
struct FakeAtl {
    calls: RefCell<Vec<(PathBuf, Lock, f64)>>,
    lock_paths: RefCell<Vec<PathBuf>>,
    result: RefCell<Reconciled>,
    exc: RefCell<Option<PyExc>>,
    lock_exc: RefCell<Option<PyExc>>,
}

fn fake_lock() -> Lock {
    Lock {
        version: "9.9.9".into(),
        tag: "atl--v9.9.9".into(),
        repo: "example/x".into(),
        sha256: Default::default(),
    }
}

impl FakeAtl {
    fn new() -> FakeAtl {
        FakeAtl {
            calls: RefCell::new(Vec::new()),
            lock_paths: RefCell::new(Vec::new()),
            result: RefCell::new(Reconciled {
                atl_path: None,
                warning: None,
                installed: false,
            }),
            exc: RefCell::new(None),
            lock_exc: RefCell::new(None),
        }
    }
}

impl AtlWiring for FakeAtl {
    fn load_lock(&self, path: &Path) -> Result<Lock, PyExc> {
        self.lock_paths.borrow_mut().push(path.to_path_buf());
        match self.lock_exc.borrow().clone() {
            Some(e) => Err(e),
            None => Ok(fake_lock()),
        }
    }

    fn reconcile(
        &self,
        _env: &Env,
        data_dir: &Path,
        lock: &Lock,
        timeout: f64,
    ) -> Result<Reconciled, PyExc> {
        self.calls
            .borrow_mut()
            .push((data_dir.to_path_buf(), lock.clone(), timeout));
        match self.exc.borrow().clone() {
            Some(e) => Err(e),
            None => Ok(self.result.borrow().clone()),
        }
    }
}

fn no_ensure(_: &Path, _: u64, _: &Env) -> GuardBinaryStatus {
    panic!("the guard binary must not be ensured without ANTHOLOGY_BOOTSTRAP_DOWNLOADED=0");
}

fn exc(class: &'static str, msg: &str) -> PyExc {
    PyExc::Other(class, msg.into())
}

fn jstr(s: &str) -> String {
    json::dumps(&Value::Str(s.into()), &json::PY_DEFAULT)
}

fn p(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

struct Fx {
    c: SessionCase,
    atl: FakeAtl,
}

impl Fx {
    fn new() -> Fx {
        Fx {
            c: SessionCase::new(),
            atl: FakeAtl::new(),
        }
    }

    fn tmp(&self) -> &Path {
        &self.c.mem.tmp
    }

    fn config(&self) -> &Path {
        &self.c.mem.config
    }

    fn plugin(&self) -> &Path {
        &self.c.plugin
    }

    fn env(&self) -> Env {
        self.c.env.clone()
    }

    /// The test's production-like seams: the fake atl and no binary bootstrap.
    fn deps(&self) -> SessionDeps<'_> {
        SessionDeps {
            atl: Some(&self.atl),
            ensure_guard_binary: &no_ensure,
            ..SessionDeps::default()
        }
    }

    fn state_write(&self, kv: Vec<(&str, Value)>) {
        state::write(
            &self.c.env,
            kv.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
        )
        .expect("state.write");
    }

    fn state(&self) -> Map {
        state::read(&self.c.env)
    }

    fn current_ops_version(&self) -> String {
        let ops = settings::setup_ops(self.plugin(), &self.c.env).expect("setup_ops");
        settings::ops_version(&ops, &Default::default())
    }

    /// `mark_setup(plugin_root)`.
    fn mark_setup(&self, plugin_root: Option<&Path>) {
        self.state_write(vec![
            ("setup_done", Value::Str("0.1.0".into())),
            (
                "settings_ops_version",
                Value::Str(self.current_ops_version()),
            ),
        ]);
        let rule = settings::plugin_read_rule(plugin_root.unwrap_or(self.plugin()));
        write(
            &self.config().join("settings.json"),
            &format!(r#"{{"permissions": {{"allow": [{}]}}}}"#, jstr(&rule)),
            None,
        );
    }

    fn hook_text(&self, cwd: Option<&Path>, hook_input: &[(&str, &str)]) -> String {
        let mut data = Map::new();
        data.insert("hook_event_name".into(), Value::Str("SessionStart".into()));
        data.insert("source".into(), Value::Str("startup".into()));
        data.insert("cwd".into(), Value::Str(p(cwd.unwrap_or(self.tmp()))));
        for (k, v) in hook_input {
            data.insert((*k).into(), Value::Str((*v).into()));
        }
        json::dumps(&Value::Object(data), &json::PY_DEFAULT)
    }

    /// `go(cwd, env_extra, hook_input)` with `deps`.
    fn go_d(
        &self,
        deps: &SessionDeps<'_>,
        cwd: Option<&Path>,
        env: &Env,
        hook_input: &[(&str, &str)],
    ) -> Value {
        let text = session_start::run(env, &self.hook_text(cwd, hook_input), deps);
        json::loads(&text).unwrap_or_else(|e| panic!("not JSON: {text:?}: {e:?}"))
    }

    fn go(&self) -> Value {
        self.go_d(&self.deps(), None, &self.env(), &[])
    }

    fn go_root(&self, root: &Path) -> Value {
        self.go_d(
            &self.deps(),
            None,
            &self.env().with_os("CLAUDE_PLUGIN_ROOT", root),
            &[],
        )
    }

    fn go_cwd(&self, cwd: &Path) -> Value {
        self.go_d(&self.deps(), Some(cwd), &self.env(), &[])
    }
}

fn context(out: &Value) -> String {
    out.as_object()
        .and_then(|o| o.get("hookSpecificOutput"))
        .and_then(Value::as_object)
        .and_then(|h| h.get("additionalContext"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("no additionalContext: {out:?}"))
        .to_string()
}

fn msg(out: &Value) -> Option<String> {
    out.as_object()
        .and_then(|o| o.get("systemMessage"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn sys(out: &Value) -> String {
    msg(out).unwrap_or_else(|| panic!("no systemMessage: {out:?}"))
}

fn lines(out: &Value) -> Vec<String> {
    msg(out)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

fn has_msg(out: &Value) -> bool {
    msg(out).is_some()
}

fn guide() -> String {
    fs::read_to_string(guide_path()).expect("the guide")
}

fn files(dir: &Path, skip: &[&Path]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let path = e.path();
            if skip.iter().any(|s| path.starts_with(s)) {
                continue;
            }
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file() {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

// --- OutputTest ---------------------------------------------------------------------------

mod output_test {
    use super::*;

    #[test]
    fn test_output_shape_and_guide() {
        let f = Fx::new();
        f.mark_setup(None);
        let out = f.go();
        let hook = out
            .as_object()
            .and_then(|o| o.get("hookSpecificOutput"))
            .and_then(Value::as_object)
            .expect("hookSpecificOutput");
        assert_eq!(
            hook.get("hookEventName").and_then(Value::as_str),
            Some("SessionStart")
        );
        assert!(context(&out).starts_with(guide().trim_end_matches('\n')));
        assert!(!has_msg(&out), "{out:?}");
    }

    #[test]
    fn test_every_session_source_gets_the_guide() {
        let f = Fx::new();
        f.mark_setup(None);
        for source in ["startup", "resume", "clear", "compact"] {
            let out = f.go_d(&f.deps(), None, &f.env(), &[("source", source)]);
            assert!(
                context(&out).contains("# Anthology routing guide"),
                "{source}"
            );
        }
    }

    #[test]
    fn test_a_subagent_gets_no_routing_guide() {
        let f = Fx::new();
        f.mark_setup(None);
        let out = f.go_d(
            &f.deps(),
            None,
            &f.env(),
            &[
                ("source", "compact"),
                ("agent_id", "a0b1c2d3e4f5a6b7c"),
                ("agent_type", "anthology:implementer"),
            ],
        );
        let ctx = context(&out);
        assert!(!ctx.contains("# Anthology routing guide"), "{ctx}");
        assert!(ctx.starts_with("## Personal memory"), "{ctx}");
        assert!(!has_msg(&out), "{out:?}");
    }

    #[test]
    fn test_only_a_non_empty_agent_id_marks_a_subagent() {
        let f = Fx::new();
        f.mark_setup(None);
        for extra in [
            r#""agent_type": "reviewer""#,
            r#""agent_id": """#,
            r#""agent_id": 7"#,
            r#""agent_id": null"#,
        ] {
            let stdin = format!(
                r#"{{"hook_event_name": "SessionStart", "source": "compact", "cwd": {}, {extra}}}"#,
                json::dumps(&Value::Str(p(f.tmp())), &json::PY_DEFAULT)
            );
            let text = session_start::run(&f.env(), &stdin, &f.deps());
            let out = json::loads(&text).expect("JSON");
            assert!(
                context(&out).contains("# Anthology routing guide"),
                "{extra}"
            );
        }
    }

    /// A hook input the way Claude Code sends it when a subagent compacts.
    const SUBAGENT_COMPACT: [(&str, &str); 3] = [
        ("source", "compact"),
        ("agent_id", "a0b1c2d3e4f5a6b7c"),
        ("agent_type", "anthology:implementer"),
    ];

    /// Everything the lead would be told at session start, and a subagent must not be: setup
    /// never ran, no data folder, and guardrails installed for another project only.
    fn lead_warnings_setup(f: &Fx) -> (PathBuf, Env) {
        let (other, mine) = (f.tmp().join("other-proj"), f.tmp().join("my-proj"));
        fs::create_dir(&other).unwrap();
        fs::create_dir(&mine).unwrap();
        write(
            &f.config().join("plugins/installed_plugins.json"),
            &format!(
                r#"{{"version": 2, "plugins": {{"anthology@anthology": [{{"scope": "user", "installPath": {}}}],
                "anthology-guardrails@anthology": [{{"scope": "project", "projectPath": {}, "installPath": {}}}]}}}}"#,
                jstr(&p(f.plugin())),
                jstr(&p(&other)),
                jstr(&p(&guard_plugin(&f.tmp().join("installs/g"), BLOCKS)))
            ),
            None,
        );
        (mine, f.env().without("CLAUDE_PLUGIN_DATA"))
    }

    #[test]
    fn test_a_subagent_on_compact_gets_none_of_the_lead_warnings() {
        let f = Fx::new();
        let (mine, env) = lead_warnings_setup(&f);
        // The same session as the lead gets the warnings (the nag is for a start), so the
        // subagent's silence is real.
        let lead = f.go_d(&f.deps(), Some(&mine), &env, &[("source", "startup")]);
        let lead_lines = lines(&lead);
        assert!(lead_lines.contains(&NAG.to_string()), "{lead_lines:?}");
        assert!(lead_lines.iter().any(|l| l.contains("not installed")));
        assert!(context(&lead).contains("# Anthology routing guide"));

        let out = f.go_d(&f.deps(), Some(&mine), &env, &SUBAGENT_COMPACT);
        assert!(!has_msg(&out), "{out:?}");
        assert!(!context(&out).contains("# Anthology routing guide"));
        assert!(!context(&out).contains("EXTREMELY"), "{}", context(&out));
    }

    #[test]
    fn test_a_subagent_runs_no_session_checks_and_writes_no_state() {
        let f = Fx::new();
        f.mark_setup(None);
        let seen: RefCell<Vec<Vec<PathBuf>>> = RefCell::new(Vec::new());
        let spy = |spec: &RunSpec<'_>| {
            seen.borrow_mut().push(spec.argv.clone());
            session_start::run_with_timeout(spec)
        };
        let mut d = f.deps();
        d.run = &spy;
        let before = f.state();
        let out = f.go_d(&d, None, &f.env(), &SUBAGENT_COMPACT);
        assert!(!has_msg(&out), "{out:?}");
        assert!(seen.borrow().is_empty(), "no guard self-test in a subagent");
        assert!(
            f.atl.calls.borrow().is_empty(),
            "no atl check in a subagent"
        );
        assert_eq!(f.state(), before, "a subagent never writes state.json");
    }

    #[test]
    fn a_subagent_compact_input_with_every_field_claude_code_sends_takes_the_subagent_path() {
        // The whole object Claude Code builds for a SessionStart that fires when a subagent
        // compacts (base fields, then source, agent_type and model), not just the keys the
        // other tests pick: the subagent path must not depend on the rest being absent.
        let f = Fx::new();
        f.mark_setup(None);
        let seen: RefCell<Vec<Vec<PathBuf>>> = RefCell::new(Vec::new());
        let spy = |spec: &RunSpec<'_>| {
            seen.borrow_mut().push(spec.argv.clone());
            session_start::run_with_timeout(spec)
        };
        let mut d = f.deps();
        d.run = &spy;
        let stdin = format!(
            r#"{{"session_id": "5b0c0a2e-3f6d-4d0e-9a53-0d7c1f7e9a11", "transcript_path": "/var/projects/p/5b0c0a2e.jsonl", "cwd": {}, "prompt_id": "c1f0", "permission_mode": "default", "agent_id": "a0b1c2d3e4f5a6b7c", "agent_type": "anthology:implementer", "effort": "high", "hook_event_name": "SessionStart", "source": "compact", "model": "claude-sonnet-5-5", "session_title": "t"}}"#,
            jstr(&p(f.tmp()))
        );
        let before = f.state();
        let text = session_start::run(&f.env(), &stdin, &d);
        let out = json::loads(&text).expect("JSON");
        assert!(!has_msg(&out), "{out:?}");
        assert!(!context(&out).contains("# Anthology routing guide"));
        assert!(seen.borrow().is_empty(), "no guard self-test in a subagent");
        assert!(f.atl.calls.borrow().is_empty());
        assert_eq!(f.state(), before);
    }

    #[test]
    fn test_a_subagent_does_not_use_up_the_agent_memory_warning() {
        // The note is once per repo: if a subagent printed it, the lead would never see it.
        let f = Fx::new();
        f.mark_setup(None);
        let repo = f.tmp().join("w");
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::create_dir_all(repo.join(".claude/agent-memory/implementer")).unwrap();
        let out = f.go_d(&f.deps(), Some(&repo), &f.env(), &SUBAGENT_COMPACT);
        assert!(!has_msg(&out), "{out:?}");
        assert!(f.state().get("agent_memory_warned").is_none());
        let lead = f.go_cwd(&repo);
        assert!(sys(&lead).contains("agent-memory"), "{lead:?}");
    }

    #[test]
    fn test_setup_not_run_shows_the_one_line_nag() {
        let f = Fx::new();
        let out = f.go();
        assert!(lines(&out).contains(&NAG.to_string()), "{out:?}");
        assert!(context(&out).contains("# Anthology routing guide"));
    }

    #[test]
    fn test_stale_u1_rule_warns() {
        let f = Fx::new();
        f.state_write(vec![("setup_done", Value::Str("0.1.0".into()))]);
        write(
            &f.config().join("settings.json"),
            r#"{"permissions": {"allow": ["Read(//old/place/plugins/**)"]}}"#,
            None,
        );
        assert!(lines(&f.go()).contains(&MOVED.to_string()));
    }

    #[test]
    fn test_missing_settings_file_after_setup_warns() {
        let f = Fx::new();
        f.state_write(vec![("setup_done", Value::Str("0.1.0".into()))]);
        assert!(lines(&f.go()).contains(&MOVED.to_string()));
    }

    #[test]
    fn test_unreadable_settings_after_setup_has_its_own_wording() {
        let f = Fx::new();
        f.state_write(vec![("setup_done", Value::Str("0.1.0".into()))]);
        write(&f.config().join("settings.json"), "{nope", None);
        let out = f.go();
        assert!(!lines(&out).contains(&MOVED.to_string()));
        assert!(sys(&out).contains("could not read settings.json"));
    }

    #[test]
    fn test_settings_with_a_bom_and_the_rule_are_quiet() {
        let f = Fx::new();
        f.state_write(vec![
            ("setup_done", Value::Str("0.1.0".into())),
            ("settings_ops_version", Value::Str(f.current_ops_version())),
        ]);
        let rule = settings::plugin_read_rule(f.plugin());
        write(
            &f.config().join("settings.json"),
            &format!(
                "\u{feff}{{\"permissions\": {{\"allow\": [{}]}}}}",
                jstr(&rule)
            ),
            None,
        );
        let out = f.go();
        assert!(!has_msg(&out), "{out:?}");
    }

    #[test]
    fn test_allow_that_is_a_string_is_not_a_match() {
        let f = Fx::new();
        f.state_write(vec![("setup_done", Value::Str("0.1.0".into()))]);
        let rule = settings::plugin_read_rule(f.plugin());
        write(
            &f.config().join("settings.json"),
            &format!(
                r#"{{"permissions": {{"allow": {}}}}}"#,
                jstr(&format!("x{rule}y"))
            ),
            None,
        );
        assert!(lines(&f.go()).contains(&MOVED.to_string()));
    }

    #[test]
    fn test_current_u1_rule_is_quiet() {
        let f = Fx::new();
        f.mark_setup(None);
        let out = f.go();
        assert!(!has_msg(&out), "{out:?}");
    }

    #[test]
    fn test_the_guide_is_never_truncated_even_if_it_alone_is_over_budget() {
        let f = Fx::new();
        f.mark_setup(None);
        let root = f.c.fake_plugins(BLOCKS);
        fs::write(
            root.join("templates/routing-guide.md"),
            format!("# Anthology routing guide\n{}", "x".repeat(9500)),
        )
        .unwrap();
        let out = f.go_root(&root);
        assert!(context(&out).chars().count() >= 9500);
    }

    #[test]
    fn test_memory_messages_reach_the_system_message() {
        let f = Fx::new();
        f.mark_setup(None);
        let mem = |_: &Path, _: &Env| Ok((String::new(), vec!["memory said hi".to_string()]));
        let mut d = f.deps();
        d.memory_run = &mem;
        let out = f.go_d(&d, None, &f.env(), &[]);
        assert!(lines(&out).contains(&"memory said hi".to_string()));
    }

    #[test]
    fn test_no_claude_md_is_written_and_no_guide_text_lands_on_disk() {
        let f = Fx::new();
        f.mark_setup(None);
        let repo = f.tmp().join("work");
        fs::create_dir(&repo).unwrap();
        f.go_cwd(&repo);
        // The plugin copies stand for Python's PLUGIN, which is outside the temporary folder.
        let skip = [f.tmp().join("plugins"), f.tmp().join("fake-plugins")];
        let skip: Vec<&Path> = skip.iter().map(PathBuf::as_path).collect();
        let all = files(f.tmp(), &skip);
        assert!(
            !all.iter()
                .any(|p| p.file_name() == Some(OsStr::new("CLAUDE.md")))
        );
        for path in all {
            if path.components().any(|c| c.as_os_str() == ".git") {
                continue;
            }
            let text = String::from_utf8_lossy(&fs::read(&path).unwrap()).into_owned();
            assert!(
                !text.contains("Anthology routing guide"),
                "{}",
                path.display()
            );
        }
    }

    #[test]
    fn test_session_start_only_writes_state() {
        let f = Fx::new();
        f.mark_setup(None);
        let before = files(f.tmp(), &[]);
        f.go();
        let after = files(f.tmp(), &[]);
        let new: Vec<_> = after
            .iter()
            .filter(|p| !before.contains(p))
            .filter(|p| p.file_name() != Some(OsStr::new("state.json")))
            .collect();
        assert!(new.is_empty(), "{new:?}");
    }

    #[test]
    fn test_missing_guide_file_is_a_warning_not_a_crash() {
        let f = Fx::new();
        f.mark_setup(None);
        let root = f.c.fake_plugins(BLOCKS);
        fs::remove_file(root.join("templates/routing-guide.md")).unwrap();
        assert!(sys(&f.go_root(&root)).contains("routing guide"));
    }

    #[test]
    fn test_stdin_that_is_not_json_still_works() {
        let f = Fx::new();
        f.mark_setup(None);
        let text = session_start::run(&f.env(), "not json", &f.deps());
        let out = json::loads(&text).unwrap();
        assert!(context(&out).contains("# Anthology routing guide"));
    }

    #[test]
    fn test_total_context_stays_within_the_budget_with_a_300_note_personal_index() {
        let f = Fx::new();
        f.mark_setup(None);
        let personal = f.c.mem.anth.join("personal-memory");
        for i in 0..300 {
            let name = format!("project-topic-{i:03}");
            let note = Note::new(&name)
                .type_("project")
                .desc(&format!("A fairly descriptive note number {i}"))
                .text();
            write(&personal.join(format!("{name}.md")), &note, None);
        }
        let out = f.go();
        let ctx = context(&out);
        assert!(ctx.chars().count() <= session_start::CONTEXT_BUDGET);
        assert!(ctx.contains("# Anthology routing guide"));
        assert!(ctx.contains("## Personal memory"));
        // `\d+ more .*anthology-mem search` on one line
        let more = ctx.lines().any(|l| {
            l.match_indices(" more ").any(|(i, _)| {
                l[..i].ends_with(|c: char| c.is_ascii_digit())
                    && l[i..].contains("anthology-mem search")
            })
        });
        assert!(more, "{ctx}");
        // The pointers name no file (the on-disk index is capped too); the heading names the folder.
        assert!(!ctx.contains("full index: "));
        assert!(ctx.contains(&p(&personal)));
    }

    #[test]
    fn test_repo_cover_only_when_memory_sync_repaired() {
        if !needs_git() {
            return;
        }
        let f = Fx::new();
        f.mark_setup(None);
        let repo =
            f.c.mem
                .make_repo("app", Some("https://example.invalid/acme/app.git"));
        write(
            &repo.join(".claude/memory/project-a.md"),
            &Note::new("project-a").type_("project").desc("First").text(),
            None,
        );
        anthology_memory::approve::approve(&repo, &f.env(), &RealGit).expect("approve");
        let first = context(&f.go_cwd(&repo));
        assert!(first.contains("## Repo memory"));
        assert!(first.contains("project-a"));
        let second = context(&f.go_cwd(&repo));
        assert!(!second.contains("## Repo memory"));
    }

    #[test]
    fn test_the_memory_folder_notice_is_quiet_on_compact() {
        if !needs_git() {
            return;
        }
        let f = Fx::new();
        f.mark_setup(None);
        let repo =
            f.c.mem
                .make_repo("app", Some("https://example.invalid/acme/app.git"));
        anthology_memory::approve::approve(&repo, &f.env(), &RealGit).expect("approve");
        let dir_set = anthology_memory::sync::DIR_SET;
        let out = f.go_d(&f.deps(), Some(&repo), &f.env(), &[("source", "compact")]);
        assert!(context(&out).contains("## Repo memory"), "{out:?}");
        assert!(!msg(&out).unwrap_or_default().contains(dir_set), "{out:?}");
        fs::remove_file(repo.join(".claude/settings.local.json")).expect("remove");
        let out = f.go_d(&f.deps(), Some(&repo), &f.env(), &[("source", "startup")]);
        assert!(lines(&out).iter().any(|l| l == dir_set), "{out:?}");
    }

    #[test]
    fn test_unapproved_repo_with_memory_gets_the_hint() {
        if !needs_git() {
            return;
        }
        let f = Fx::new();
        f.mark_setup(None);
        let repo =
            f.c.mem
                .make_repo("other", Some("https://example.invalid/acme/other.git"));
        write(
            &repo.join(".claude/memory/project-a.md"),
            &Note::new("project-a").type_("project").desc("First").text(),
            None,
        );
        let out = f.go_cwd(&repo);
        assert!(lines(&out).contains(&anthology_memory::sync::HINT.to_string()));
    }

    /// The 9,000 and 2,000 budgets (`cargo test -p anthology-runtime budget`).
    mod budget {
        use super::*;

        #[test]
        fn test_oversized_memory_text_is_truncated_at_a_line_with_a_pointer() {
            let f = Fx::new();
            f.mark_setup(None);
            let lines_text: String = (0..400)
                .map(|i| format!("- note line {i:04} with some padding text here\n"))
                .collect();
            let mem_text = format!("## Personal memory (x)\n\n{lines_text}");
            let mem = move |_: &Path, _: &Env| Ok((mem_text.clone(), Vec::new()));
            let mut d = f.deps();
            d.memory_run = &mem;
            let out = f.go_d(&d, None, &f.env(), &[]);
            let ctx = context(&out);
            assert!(ctx.chars().count() <= 9000);
            assert!(ctx.starts_with(guide().trim_end_matches('\n')));
            let all: Vec<&str> = ctx.lines().collect();
            let last = all.last().unwrap();
            assert!(last.contains("anthology-mem search"), "{last}");
            assert!(!last.contains("MEMORY.md"), "{last}");
            let body = &all[..all.len() - 1];
            let end = body.last().unwrap();
            assert!(end.starts_with("- note line "), "{end}");
            assert!(end.ends_with("padding text here"), "{end}"); // cut on a line boundary
        }

        #[test]
        fn test_system_message_is_capped() {
            let f = Fx::new();
            f.mark_setup(None);
            let messages: Vec<String> = (0..100)
                .map(|i| format!("m{i:03} {}", "y".repeat(80)))
                .collect();
            let mem = move |_: &Path, _: &Env| Ok((String::new(), messages.clone()));
            let mut d = f.deps();
            d.memory_run = &mem;
            let out = f.go_d(&d, None, &f.env(), &[]);
            let m = sys(&out);
            assert!(m.chars().count() <= 2000);
            assert!(m.ends_with('…'));
        }

        /// The 300-note budget case of `test_total_context_stays_within_the_budget…`, with the
        /// memory text given directly (the real index waits for MS2).
        #[test]
        fn context_budget_holds_for_a_300_line_memory_index() {
            let f = Fx::new();
            f.mark_setup(None);
            let personal = f.c.mem.anth.join("personal-memory");
            let index: String = (0..300)
                .map(|i| {
                    format!(
                        "- [project-topic-{i:03}](project-topic-{i:03}.md) — A fairly descriptive note number {i}\n"
                    )
                })
                .collect();
            let text = format!("## Personal memory ({})\n\n{index}", personal.display());
            let mem = move |_: &Path, _: &Env| Ok((text.clone(), Vec::new()));
            let mut d = f.deps();
            d.memory_run = &mem;
            let ctx = context(&f.go_d(&d, None, &f.env(), &[]));
            assert!(ctx.chars().count() <= 9000);
            assert!(ctx.contains("# Anthology routing guide"));
            assert!(ctx.contains("## Personal memory"));
            assert!(ctx.contains("use `anthology-mem search <words>`"));
            assert!(!ctx.contains("full indexes"));
        }

        #[test]
        fn budget_compose_counts_code_points_and_never_cuts_the_guide() {
            let g = "é".repeat(100);
            let memory = "ü\n".repeat(6000);
            let out = session_start::compose(&g, &memory);
            assert!(out.chars().count() <= 9000);
            assert!(out.starts_with(&g));
            assert!(out.ends_with(
                "(memory index truncated to fit the session budget; use `anthology-mem search <words>`)"
            ));
            assert!(!out.contains("full indexes"));
            // a guide alone over the budget keeps every character, and memory is dropped
            let big = "x".repeat(9500);
            assert_eq!(session_start::compose(&big, "m\n"), big);
            // no memory: the guide without its trailing newlines
            assert_eq!(session_start::compose("g\n\n", "\n\n"), "g");
        }
    }
}

// --- OpsVersionTest -----------------------------------------------------------------------

fn setup_ops_plus_new_tool(root: &Path, env: &Env) -> Result<Vec<Op>, String> {
    let mut ops = settings::setup_ops(root, env)?;
    ops.push(Op::add(&["permissions", "allow"], "Bash(new-tool *)"));
    Ok(ops)
}

fn setup_ops_not_found(_: &Path, _: &Env) -> Result<Vec<Op>, String> {
    Err("FileNotFoundError: anthology-guardrails plugin not found".into())
}

fn setup_ops_import_error(_: &Path, _: &Env) -> Result<Vec<Op>, String> {
    Err("ImportError: x".into())
}

fn setup_ops_attribute_error(_: &Path, _: &Env) -> Result<Vec<Op>, String> {
    Err("AttributeError: x".into())
}

fn setup_ops_syntax_error(_: &Path, _: &Env) -> Result<Vec<Op>, String> {
    Err("SyntaxError: x".into())
}

mod ops_version_test {
    use super::*;

    #[test]
    fn test_same_version_is_quiet() {
        let f = Fx::new();
        f.mark_setup(None);
        assert!(!has_msg(&f.go()));
    }

    #[test]
    fn test_changed_version_is_one_note_and_settings_are_untouched() {
        let f = Fx::new();
        f.mark_setup(None);
        f.state_write(vec![(
            "settings_ops_version",
            Value::Str("1-0000000000000000".into()),
        )]);
        let before = fs::read(f.config().join("settings.json")).unwrap();
        let out = f.go();
        assert_eq!(lines(&out), vec![OPS_CHANGED.to_string()]);
        assert_eq!(fs::read(f.config().join("settings.json")).unwrap(), before);
        assert_eq!(
            f.state()
                .get("settings_ops_version")
                .and_then(Value::as_str),
            Some("1-0000000000000000")
        );
    }

    #[test]
    fn test_a_new_op_in_a_release_is_noticed() {
        let f = Fx::new();
        f.mark_setup(None);
        let mut d = f.deps();
        d.settings = SettingsDeps {
            setup_ops: setup_ops_plus_new_tool,
        };
        let out = f.go_d(&d, None, &f.env(), &[]);
        assert!(lines(&out).contains(&OPS_CHANGED.to_string()), "{out:?}");
    }

    #[test]
    fn test_setup_apply_records_the_version_session_start_expects() {
        use anthology_runtime::setup::{self, SetupDeps};
        fn present(_: &Path, _: u64, _: &Env) -> GuardBinaryStatus {
            GuardBinaryStatus::Present
        }
        let f = Fx::new();
        write(&f.config().join("settings.json"), "{}\n", None);
        let deps = SetupDeps {
            atl: Some(&f.atl),
            ensure_guard_binary: &present,
            ..SetupDeps::default()
        };
        let env = f.env();
        let out = setup::preview(f.plugin(), Some(f.tmp()), &env, &deps);
        let digest = out
            .as_object()
            .and_then(|o| o.get("settings"))
            .and_then(Value::as_object)
            .and_then(|s| s.get("sha256"))
            .and_then(Value::as_str)
            .unwrap()
            .to_string();
        let res = setup::apply(f.plugin(), &digest, Some(f.tmp()), true, &env, &[], &deps);
        assert_eq!(
            res.as_object().and_then(|o| o.get("ok")),
            Some(&Value::Bool(true)),
            "{res:?}"
        );
        assert_eq!(
            f.state()
                .get("settings_ops_version")
                .and_then(Value::as_str),
            Some(f.current_ops_version().as_str())
        );
        assert!(!lines(&f.go()).contains(&OPS_CHANGED.to_string()));
    }

    #[test]
    fn test_setup_from_before_the_version_was_recorded_gets_the_note() {
        let f = Fx::new();
        f.mark_setup(None);
        f.state_write(vec![("settings_ops_version", Value::Null)]);
        assert!(lines(&f.go()).contains(&OPS_CHANGED.to_string()));
    }

    #[test]
    fn test_no_state_is_the_existing_run_setup_path() {
        let f = Fx::new();
        let l = lines(&f.go());
        assert!(l.contains(&NAG.to_string()));
        assert!(!l.contains(&OPS_CHANGED.to_string()));
    }

    #[test]
    fn test_unusable_plugin_root_skips_the_check_quietly() {
        let f = Fx::new();
        let root = f.c.fake_plugins(BLOCKS); // no .claude-plugin/plugin.json: setup_ops refuses it
        f.state_write(vec![("setup_done", Value::Str("0.1.0".into()))]);
        write(
            &f.config().join("settings.json"),
            &format!(
                r#"{{"permissions": {{"allow": [{}]}}}}"#,
                jstr(&settings::plugin_read_rule(&root))
            ),
            None,
        );
        let out = f.go_root(&root);
        assert!(!has_msg(&out), "{out:?}");
    }

    #[test]
    fn test_missing_or_broken_guardrails_add_no_line_of_their_own() {
        let f = Fx::new();
        f.mark_setup(None);
        f.state_write(vec![(
            "settings_ops_version",
            Value::Str("1-0000000000000000".into()),
        )]);
        for ops in [
            setup_ops_not_found as fn(&Path, &Env) -> Result<Vec<Op>, String>,
            setup_ops_import_error,
            setup_ops_attribute_error,
            setup_ops_syntax_error,
        ] {
            let mut d = f.deps();
            d.settings = SettingsDeps { setup_ops: ops };
            let out = f.go_d(&d, None, &f.env(), &[]);
            assert!(!has_msg(&out), "{out:?}");
        }
    }

    #[test]
    fn test_changed_version_with_every_op_already_present_is_quiet() {
        let f = Fx::new();
        f.mark_setup(None);
        let path = f.config().join("settings.json");
        let text = fs::read_to_string(&path).unwrap();
        let ops = settings::setup_ops(f.plugin(), &f.env()).unwrap();
        fs::write(&path, settings::apply(&text, &ops).unwrap().0).unwrap();
        f.state_write(vec![(
            "settings_ops_version",
            Value::Str("1-0000000000000000".into()),
        )]);
        assert!(!has_msg(&f.go()));
    }

    #[test]
    fn test_a_moved_plugin_alone_is_only_the_moved_line() {
        let f = Fx::new();
        f.mark_setup(None);
        let path = f.config().join("settings.json");
        let ops: Vec<Op> = settings::setup_ops(f.plugin(), &f.env())
            .unwrap()
            .into_iter()
            .filter(|op| op.tag() != Some(TAG_PLUGIN_READ))
            .collect();
        fs::write(&path, settings::apply("{}", &ops).unwrap().0).unwrap();
        f.state_write(vec![(
            "settings_ops_version",
            Value::Str("1-0000000000000000".into()),
        )]);
        assert_eq!(lines(&f.go()), vec![MOVED.to_string()]);
    }
}

// --- CompatibilityLinesTest ---------------------------------------------------------------

mod compatibility_lines_test {
    use super::*;

    struct Cx {
        f: Fx,
        repo: PathBuf,
    }

    fn setup() -> Cx {
        let f = Fx::new();
        f.mark_setup(None);
        let repo = f.tmp().join("work");
        fs::create_dir_all(repo.join(".git")).unwrap();
        Cx { f, repo }
    }

    impl Cx {
        fn lines_with(&self, d: &SessionDeps<'_>, env: &Env) -> Vec<String> {
            lines(&self.f.go_d(d, Some(&self.repo), env, &[]))
        }

        fn lines(&self) -> Vec<String> {
            self.lines_with(&self.f.deps(), &self.f.env())
        }

        fn copy_plugin(&self, drop: &[&str], unexec: &[&str]) -> PathBuf {
            let root = self.f.tmp().join("plugcopy");
            fs::create_dir_all(root.join("templates")).unwrap();
            fs::copy(guide_path(), root.join("templates/routing-guide.md")).unwrap();
            copy_bin(&root.join("bin"));
            for name in drop {
                fs::remove_file(root.join("bin").join(name)).unwrap();
            }
            for name in unexec {
                fs::set_permissions(
                    root.join("bin").join(name),
                    fs::Permissions::from_mode(0o644),
                )
                .unwrap();
            }
            root
        }
    }

    fn quiet_self_test(_: &Env, _: &Path, _: &Path) -> Result<Option<String>, PyExc> {
        Ok(None)
    }

    #[test]
    fn test_a_healthy_session_has_no_compat_lines() {
        assert_eq!(setup().lines(), Vec::<String>::new());
    }

    #[test]
    fn test_missing_plugin_data_env() {
        let c = setup();
        let env = c.f.env().without("CLAUDE_PLUGIN_DATA");
        assert!(
            c.lines_with(&c.f.deps(), &env)
                .contains(&DATA_LINE.to_string())
        );
    }

    #[test]
    fn test_present_plugin_data_env_is_quiet() {
        assert!(!setup().lines().contains(&DATA_LINE.to_string()));
    }

    #[test]
    fn test_wrappers_missing_or_not_executable_are_named() {
        let c = setup();
        let root = c.copy_plugin(&["atl"], &["anthology-mem"]);
        let mut d = c.f.deps();
        d.self_test = Some(&quiet_self_test);
        let l = c.lines_with(&d, &c.f.env().with_os("CLAUDE_PLUGIN_ROOT", &root));
        let hit: Vec<_> = l.iter().filter(|l| l.contains("bin/")).collect();
        assert_eq!(hit.len(), 1, "{l:?}");
        assert!(hit[0].contains("atl"));
        assert!(hit[0].contains("anthology-mem"));
        assert!(!hit[0].contains("atl-hook"));
    }

    #[test]
    fn test_wrappers_present_is_quiet() {
        let c = setup();
        let root = c.copy_plugin(&[], &[]);
        c.f.mark_setup(Some(&root));
        let mut d = c.f.deps();
        d.self_test = Some(&quiet_self_test);
        assert_eq!(
            c.lines_with(&d, &c.f.env().with_os("CLAUDE_PLUGIN_ROOT", &root)),
            Vec::<String>::new()
        );
    }

    #[test]
    fn test_a_user_agent_folder_next_to_anthology_folders_is_quiet() {
        let c = setup();
        let mem = c.repo.join(".claude/agent-memory");
        fs::create_dir_all(mem.join("anthology-implementer")).unwrap();
        fs::create_dir(mem.join("reviewer")).unwrap();
        assert_eq!(c.lines(), Vec::<String>::new());
        assert!(c.f.state().get("agent_memory_warned").is_none());
    }

    #[test]
    fn test_a_bare_agent_folder_alone_is_a_soft_warning_shown_once() {
        let c = setup();
        fs::create_dir_all(c.repo.join(".claude/agent-memory/reviewer")).unwrap();
        let l = c.lines();
        let hit: Vec<_> = l.iter().filter(|l| l.contains("agent-memory")).collect();
        assert_eq!(hit.len(), 1, "{l:?}");
        assert!(
            hit[0].contains(
                "`.claude/agent-memory/reviewer/` exists but no `anthology-reviewer/` does"
            )
        );
        assert!(hit[0].contains("agent of your own named reviewer"));
        assert_eq!(c.lines(), Vec::<String>::new());
        assert_eq!(c.lines(), Vec::<String>::new());
        let warned = c.f.state();
        let warned = warned
            .get("agent_memory_warned")
            .and_then(Value::as_object)
            .unwrap();
        assert_eq!(warned.iter().count(), 1);
    }

    #[test]
    fn test_subagent_model_in_hook_env() {
        let c = setup();
        let l = c.lines_with(
            &c.f.deps(),
            &c.f.env().with("CLAUDE_CODE_SUBAGENT_MODEL", "SECRETMODEL"),
        );
        assert!(l.contains(&MODEL_LINE.to_string()));
        assert!(!l.join("\n").contains("SECRETMODEL"));
    }

    #[test]
    fn test_subagent_model_in_settings_files() {
        let c = setup();
        let user = c.f.config().join("settings.json");
        write(
            &user,
            r#"{"env": {"CLAUDE_CODE_SUBAGENT_MODEL": "m"}}"#,
            None,
        );
        assert!(c.lines().contains(&MODEL_LINE.to_string()));
        write(
            &user,
            &format!(
                r#"{{"permissions": {{"allow": [{}]}}}}"#,
                jstr(&settings::plugin_read_rule(c.f.plugin()))
            ),
            None,
        );
        assert!(!c.lines().contains(&MODEL_LINE.to_string()));
        write(
            &c.repo.join(".claude/settings.local.json"),
            r#"{"env": {"CLAUDE_CODE_SUBAGENT_MODEL": "m"}}"#,
            None,
        );
        assert!(c.lines().contains(&MODEL_LINE.to_string()));
    }

    #[test]
    fn test_each_compat_step_is_isolated() {
        let c = setup();
        for (name, label) in [
            ("_data_dir_warning", "the plugin data folder check"),
            ("_wrappers_warning", "the bin wrapper check"),
            ("_agent_memory_warning", "the agent-memory path check"),
            ("_subagent_model_warning", "the subagent model check"),
            // only a compact or resume runs this step
            ("_stale_plugin_warning", "the loaded plugin version check"),
        ] {
            let fail = move |n: &str| (n == name).then(|| exc("RuntimeError", "boom"));
            let mut d = c.f.deps();
            d.fail = &fail;
            let out =
                c.f.go_d(&d, Some(&c.repo), &c.f.env(), &[("source", "compact")]);
            assert!(
                context(&out).contains("# Anthology routing guide"),
                "{name}"
            );
            assert_eq!(
                lines(&out),
                vec![format!("anthology: {label} failed (RuntimeError)")],
                "{name}"
            );
        }
    }

    #[test]
    fn test_a_broken_compat_import_only_loses_these_checks() {
        let c = setup();
        let mut d = c.f.deps();
        d.compat = None;
        let out = c.f.go_d(&d, Some(&c.repo), &c.f.env(), &[]);
        assert!(context(&out).contains("# Anthology routing guide"));
        let l = lines(&out);
        assert_eq!(
            l.iter().filter(|l| l.contains("failed")).count(),
            3,
            "{l:?}"
        );
    }

    #[test]
    fn test_session_start_never_asks_docker() {
        let c = setup();
        let seen: RefCell<Vec<Vec<PathBuf>>> = RefCell::new(Vec::new());
        let spy = |spec: &RunSpec<'_>| {
            seen.borrow_mut().push(spec.argv.clone());
            session_start::run_with_timeout(spec)
        };
        let mut d = c.f.deps();
        d.run = &spy;
        c.f.go_d(&d, Some(&c.repo), &c.f.env(), &[]);
        assert!(!seen.borrow().is_empty(), "the self-test ran");
        for argv in seen.borrow().iter() {
            assert_ne!(argv[0].file_name(), Some(OsStr::new("docker")));
        }
    }
}

// --- MemoryPrivacyTest --------------------------------------------------------------------

/// `MemoryPrivacyTest`: B3 and C1 through the whole hook. Python stubbed memory_sync's git
/// calls (`fake_repo`); the real sync needs a real repo, so these cases need git.
mod memory_privacy_test {
    use super::*;

    /// `re.sub(r"[^A-Za-z0-9]", "-", str(path))`: the old per-path memory folder name.
    fn encode(path: &Path) -> String {
        path.to_string_lossy()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect()
    }

    /// `setUp`: a session with setup run, and an approved repo with a remote.
    fn set_up() -> Option<(Fx, PathBuf)> {
        if !needs_git() {
            return None;
        }
        let f = Fx::new();
        f.mark_setup(None);
        let root =
            f.c.mem
                .make_repo("app", Some("https://example.invalid/acme/app.git"));
        anthology_memory::approve::approve(&root, &f.env(), &RealGit).expect("approve");
        Some((f, root))
    }

    #[test]
    fn test_legacy_notes_never_land_in_the_repo() {
        let Some((f, root)) = set_up() else { return };
        let old = f
            .config()
            .join("projects")
            .join(encode(&root))
            .join("memory");
        // built from two halves, as the Python test is, so no host name appears in the source
        let body = ["Builds run on build-", "box.lan.\n"].concat();
        write(
            &old.join("reference-build-host.md"),
            &Note::new("reference-build-host")
                .type_("reference")
                .desc("The build host")
                .body(&body)
                .text(),
            None,
        );
        write(
            &old.join("project-plan.md"),
            &Note::new("project-plan")
                .type_("project")
                .desc("The plan")
                .text(),
            None,
        );
        let out = f.go_cwd(&root);
        let mut names: Vec<String> = fs::read_dir(root.join(".claude/memory"))
            .expect("repo memory")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, Vec::<String>::new());
        let personal = f.c.mem.anth.join("personal-memory");
        assert!(personal.join("reference-build-host.md").is_file());
        assert!(personal.join("project-plan.md").is_file());
        assert!(f.c.mem.anth.join("migrated.json").is_file());
        let m = sys(&out);
        assert!(m.contains("into personal memory"), "{m}");
        assert!(m.contains("/anthology:setup"), "{m}");
    }

    #[test]
    fn test_user_notes_leave_the_repo_and_personal_memory_is_always_shown() {
        let Some((f, root)) = set_up() else { return };
        let personal = f.c.mem.anth.join("personal-memory");
        let out = f.go_cwd(&root);
        let ctx = context(&out);
        assert!(ctx.contains(&format!("## Personal memory ({})", p(&personal))));
        assert!(ctx.contains("Save notes about the user"));
        assert!(!personal.exists());
        write(
            &root.join(".claude/memory/user-role.md"),
            &Note::new("user-role").type_("user").desc("Role").text(),
            None,
        );
        let out = f.go_cwd(&root);
        assert!(!root.join(".claude/memory/user-role.md").exists());
        assert!(personal.join("user-role.md").is_file());
        assert!(sys(&out).contains("moved 1 user note(s) out of the repo"));
        let ctx = context(&out);
        assert!(ctx.contains("[user-role](user-role.md)"));
        assert!(ctx.chars().count() <= session_start::CONTEXT_BUDGET);
    }
}

// --- FailureTest --------------------------------------------------------------------------

mod failure_test {
    use super::*;

    fn boom_root(_: &Env) -> Result<PathBuf, PyExc> {
        Err(exc("RuntimeError", "boom"))
    }

    #[test]
    fn test_injected_exception_gives_a_short_system_message() {
        let f = Fx::new();
        let mut d = f.deps();
        d.plugin_root = &boom_root;
        let out = json::loads(&session_start::run(&f.env(), "{}", &d)).unwrap();
        let m = sys(&out);
        assert!(m.contains("anthology"));
        assert!(m.chars().count() <= 300);
        assert!(!m.contains("did not run"));
        assert_eq!(
            m,
            "anthology: session start did not complete (RuntimeError); the routing guide may be missing"
        );
    }

    #[test]
    fn test_every_step_is_isolated_and_the_guide_is_always_delivered() {
        let f = Fx::new();
        f.mark_setup(None);
        for (name, e) in [
            ("_guard_selftest", exc("RuntimeError", "boom")),
            ("_setup_warning", exc("RecursionError", "")),
            ("_reconcile_atl", exc("OSError", "x")),
            ("_read_guide", PyExc::UnicodeDecodeError("bad".into())),
            ("_parse_stdin", PyExc::ValueError("x".into())),
        ] {
            let fail = move |n: &str| (n == name).then(|| e.clone());
            let mut d = f.deps();
            d.fail = &fail;
            let out = f.go_d(&d, None, &f.env(), &[]);
            if name != "_read_guide" {
                assert!(
                    context(&out).contains("# Anthology routing guide"),
                    "{name}"
                );
            }
            let m = sys(&out);
            assert!(m.contains("anthology:"), "{name}");
            // the steps that did not fail are not blamed
            assert!(!m.contains("memory sync failed"), "{name}");
        }
    }

    #[test]
    fn test_a_failing_guard_does_not_claim_the_other_steps_failed() {
        let f = Fx::new();
        f.mark_setup(None);
        let fail = |n: &str| (n == "_guard_selftest").then(|| exc("RuntimeError", "boom"));
        let mut d = f.deps();
        d.fail = &fail;
        let out = f.go_d(&d, None, &f.env(), &[]);
        assert_eq!(
            lines(&out),
            vec!["anthology: the guardrails self-test failed (RuntimeError)".to_string()]
        );
    }

    #[test]
    fn test_pathologically_nested_stdin_and_settings_still_deliver_the_guide() {
        let f = Fx::new();
        f.mark_setup(None);
        write(
            &f.config().join("settings.json"),
            &"[".repeat(200_000),
            None,
        );
        let out = json::loads(&session_start::run(
            &f.env(),
            &"[".repeat(200_000),
            &f.deps(),
        ))
        .unwrap();
        assert!(context(&out).contains("# Anthology routing guide"));
    }

    #[test]
    fn test_non_utf8_guide_is_a_warning_not_a_crash() {
        let f = Fx::new();
        f.mark_setup(None);
        let root = f.c.fake_plugins(BLOCKS);
        fs::write(root.join("templates/routing-guide.md"), b"\xff\xfe\x00bad").unwrap();
        f.mark_setup(Some(&root));
        assert!(sys(&f.go_root(&root)).contains("routing guide"));
    }

    #[test]
    fn test_memory_failure_never_fails_the_session() {
        let f = Fx::new();
        f.mark_setup(None);
        let mem = |_: &Path, _: &Env| Err(exc("OSError", "disk"));
        let mut d = f.deps();
        d.memory_run = &mem;
        let out = f.go_d(&d, None, &f.env(), &[]);
        assert!(context(&out).contains("# Anthology routing guide"));
        assert!(sys(&out).contains("memory"));
        assert!(lines(&out).contains(&"anthology memory: disk".to_string()));
    }
}

// --- PlatformTest -------------------------------------------------------------------------

mod platform_test {
    use super::*;

    fn windows() -> (String, String) {
        ("Windows".into(), "AMD64".into())
    }

    #[test]
    fn test_windows_gets_the_refusal_and_nothing_else() {
        let f = Fx::new();
        let mem_called = RefCell::new(false);
        let guard_called = RefCell::new(false);
        let mem = |_: &Path, _: &Env| {
            *mem_called.borrow_mut() = true;
            Ok((String::new(), Vec::new()))
        };
        let guard = |_: &Env, _: &Path, _: &Path| {
            *guard_called.borrow_mut() = true;
            Ok(None)
        };
        let mut d = f.deps();
        d.uname = windows;
        d.memory_run = &mem;
        d.self_test = Some(&guard);
        let out = session_start::run(&f.env(), "{}", &d);
        assert_eq!(
            out,
            r#"{"systemMessage": "Anthology supports macOS and Linux only; it is inactive on Windows."}"#
        );
        assert!(!*mem_called.borrow());
        assert!(!*guard_called.borrow());
        assert!(f.atl.calls.borrow().is_empty());
        assert!(!state::path(&f.env()).exists());
    }
}

// --- StateTest ----------------------------------------------------------------------------

mod state_test {
    use super::*;

    #[test]
    fn test_data_dir_is_recorded_only_when_it_changes() {
        let f = Fx::new();
        f.mark_setup(None);
        f.go();
        assert_eq!(
            f.state().get("data_dir").and_then(Value::as_str),
            Some(p(&f.c.mem.data).as_str())
        );
        let writes: RefCell<Vec<Vec<(String, Value)>>> = RefCell::new(Vec::new());
        let spy = |env: &Env, kv: Vec<(String, Value)>| {
            writes.borrow_mut().push(kv.clone());
            state::write(env, kv)
        };
        let mut d = f.deps();
        d.state_write = &spy;
        f.go_d(&d, None, &f.env(), &[]);
        assert!(
            !writes
                .borrow()
                .iter()
                .any(|kv| kv.iter().any(|(k, _)| k == "data_dir"))
        );
        let data2 = f.tmp().join("data2");
        f.go_d(
            &d,
            None,
            &f.env().with_os("CLAUDE_PLUGIN_DATA", &data2),
            &[],
        );
        assert!(writes.borrow().iter().any(|kv| {
            kv.iter()
                .any(|(k, v)| k == "data_dir" && v.as_str() == Some(p(&data2).as_str()))
        }));
    }

    #[test]
    fn test_no_data_dir_env_means_no_state_write() {
        let f = Fx::new();
        f.mark_setup(None);
        let env = f.env().without("CLAUDE_PLUGIN_DATA");
        let before = f.state();
        session_start::run(&env, "{}", &f.deps());
        assert_eq!(f.state(), before);
    }
}

// --- AtlTest ------------------------------------------------------------------------------

mod rtk_test {
    use super::*;

    #[test]
    fn test_not_reconciled_before_setup_ran() {
        let f = Fx::new();
        f.go();
        assert!(f.atl.calls.borrow().is_empty());
    }

    #[test]
    fn test_reconcile_runs_after_setup_with_the_data_dir_and_lock() {
        let f = Fx::new();
        f.mark_setup(None);
        f.go();
        let calls = f.atl.calls.borrow();
        assert_eq!(calls.len(), 1);
        let (data_dir, lock, timeout) = &calls[0];
        assert_eq!(data_dir, &f.c.mem.data);
        assert_eq!(
            *f.atl.lock_paths.borrow(),
            vec![f.plugin().join("atl.lock")]
        );
        assert_eq!(lock, &fake_lock()); // the parsed lock, not a path
        assert_eq!(*timeout, ATL_TIMEOUT);
    }

    #[test]
    fn test_worst_case_reconcile_fits_the_hook_timeout() {
        // atl_install downloads for at most 4 x timeout plus one socket timeout; it only starts
        // when ATL_MIN_REMAINING is left of the DEADLINE, which is itself under the 60 s hook
        // timeout.
        const { assert!(5.0 * ATL_TIMEOUT <= ATL_MIN_REMAINING) };
        const { assert!(DEADLINE < 60.0) };
    }

    #[test]
    fn test_data_dir_falls_back_to_state() {
        let f = Fx::new();
        f.mark_setup(None);
        let remembered = f.tmp().join("remembered");
        f.state_write(vec![("data_dir", Value::Str(p(&remembered)))]);
        let env = f.env().without("CLAUDE_PLUGIN_DATA");
        session_start::run(&env, "{}", &f.deps());
        assert_eq!(f.atl.calls.borrow()[0].0, remembered);
    }

    #[test]
    fn test_returned_warning_is_shown() {
        let f = Fx::new();
        f.mark_setup(None);
        f.atl.result.borrow_mut().warning = Some("atl download failed: offline".into());
        assert!(lines(&f.go()).contains(&"atl download failed: offline".to_string()));
    }

    #[test]
    fn test_a_clean_result_adds_no_warning() {
        let f = Fx::new();
        f.mark_setup(None);
        *f.atl.result.borrow_mut() = Reconciled {
            atl_path: Some(f.c.mem.data.join("atl/x/atl")),
            warning: None,
            installed: true,
        };
        assert!(!has_msg(&f.go()));
    }

    fn never_exists(_: &Path) -> bool {
        false
    }

    #[test]
    fn test_a_missing_lock_is_the_not_wired_note() {
        let f = Fx::new();
        f.mark_setup(None);
        *f.atl.lock_exc.borrow_mut() = Some(exc("RuntimeError", "cannot read"));
        let mut d = f.deps();
        d.path_exists = never_exists;
        let data = Value::Str(p(&f.c.mem.data));
        let warning = session_start::reconcile_atl(&f.env(), f.plugin(), Some(&data), &d).unwrap();
        assert_eq!(
            warning.as_deref(),
            Some("anthology: atl wiring not present yet; skipped")
        );
        assert!(f.atl.calls.borrow().is_empty());
    }

    #[test]
    fn test_an_invalid_lock_is_a_warning_and_nothing_is_reconciled() {
        let f = Fx::new();
        f.mark_setup(None);
        *f.atl.lock_exc.borrow_mut() = Some(exc("RuntimeError", "atl.lock is not valid JSON"));
        let out = f.go();
        assert!(sys(&out).contains("atl.lock is not valid"));
        assert!(f.atl.calls.borrow().is_empty());
        assert!(context(&out).contains("# Anthology routing guide"));
    }

    #[test]
    fn test_exception_becomes_a_warning() {
        let f = Fx::new();
        f.mark_setup(None);
        *f.atl.exc.borrow_mut() = Some(exc("RuntimeError", "kaboom"));
        let out = f.go();
        assert!(sys(&out).contains("atl"));
        assert!(sys(&out).contains("kaboom"));
        assert!(context(&out).contains("# Anthology routing guide"));
    }

    #[test]
    fn test_absent_wiring_is_one_short_note() {
        let f = Fx::new();
        f.mark_setup(None);
        let mut d = f.deps();
        d.atl = None;
        let out = f.go_d(&d, None, &f.env(), &[]);
        assert!(
            lines(&out).contains(&"anthology: atl wiring not present yet; skipped".to_string())
        );
    }

    #[test]
    fn a_data_dir_that_is_not_a_string_is_the_path_error() {
        let f = Fx::new();
        let data = Value::Int(json::Int::Small(5));
        let warning =
            session_start::reconcile_atl(&f.env(), f.plugin(), Some(&data), &f.deps()).unwrap();
        assert_eq!(
            warning.as_deref(),
            Some(
                "anthology: atl setup failed: argument should be a str or an os.PathLike object \
                 where __fspath__ returns a str, not 'int'"
            )
        );
        let falsy = Value::Str(String::new());
        assert_eq!(
            session_start::reconcile_atl(&f.env(), f.plugin(), Some(&falsy), &f.deps()).unwrap(),
            None
        );
    }
}

// --- RealAtlTest --------------------------------------------------------------------------

mod real_rtk_test {
    use super::*;
    use anthology_runtime::fetch::Fetch;
    use anthology_testkit::mem::MemCase;
    use anthology_testkit::release::{self, ATL_BODY, TARGET, VERSION};
    use anthology_testkit::rt_b::{AtlFakeFetch, FakeAnswer, atl_url_for};
    use std::io::Write;

    struct Fake(AtlFakeFetch);

    impl Fetch for Fake {
        fn fetch(&self, url: &str, dest: &mut dyn Write, timeout: f64) -> Result<(), PyExc> {
            match self.0.call(url, timeout) {
                FakeAnswer::Write(b) => {
                    dest.write_all(b)?;
                    Ok(())
                }
                FakeAnswer::Raise(t, m) => Err(PyExc::Other(t, m)),
            }
        }
    }

    fn linux() -> (String, String) {
        (release::LINUX.0.into(), release::LINUX.1.into())
    }

    /// The real `atl_install` behind session start's adapter, with a fake download.
    struct Real<'a>(&'a Fake);

    impl AtlWiring for Real<'_> {
        fn load_lock(&self, path: &Path) -> Result<Lock, PyExc> {
            atl_install::load_lock(path)
        }

        fn reconcile(
            &self,
            env: &Env,
            data_dir: &Path,
            lock: &Lock,
            timeout: f64,
        ) -> Result<Reconciled, PyExc> {
            let deps = AtlDeps {
                uname: linux,
                ..AtlDeps::new(self.0)
            };
            Ok(atl_install::reconcile(env, data_dir, lock, timeout, &deps))
        }
    }

    fn deps<'a>(real: &'a Real<'a>) -> SessionDeps<'a> {
        SessionDeps {
            atl: Some(real),
            ..SessionDeps::default()
        }
    }

    #[test]
    fn test_installs_records_the_path_and_is_quiet_the_second_time() {
        let m = MemCase::new();
        let blob = release::tarball(ATL_BODY);
        let root = m.tmp.join("plugin");
        let lock = release::write_lock(&root.join("atl.lock"), &blob, VERSION);
        let fetch = Fake(AtlFakeFetch::serving(&atl_url_for(&lock, TARGET), &blob));
        let real = Real(&fetch);
        let d = deps(&real);
        let data = Value::Str(p(&m.data));
        let want = m.data.join("atl").join(VERSION).join("atl");
        assert_eq!(
            session_start::reconcile_atl(&m.env, &root, Some(&data), &d).unwrap(),
            None
        );
        assert_eq!(
            state::read(&m.env).get("atl_path").and_then(Value::as_str),
            Some(p(&want).as_str())
        );
        assert!(fs::metadata(&want).unwrap().permissions().mode() & 0o111 != 0);
        assert_eq!(
            session_start::reconcile_atl(&m.env, &root, Some(&data), &d).unwrap(),
            None
        );
        let calls = fetch.0.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, ATL_TIMEOUT);
    }

    #[test]
    fn session_start_leaves_rtk_path_and_legacy_folder() {
        // The old install stays exactly as it was until setup removes it: its state key, its
        // downloaded versions (an ancient one included) and its history folder.
        let m = MemCase::new();
        let blob = release::tarball(ATL_BODY);
        let root = m.tmp.join("plugin");
        let lock = release::write_lock(&root.join("atl.lock"), &blob, VERSION);
        let fetch = Fake(AtlFakeFetch::serving(&atl_url_for(&lock, TARGET), &blob));
        let real = Real(&fetch);
        let data = Value::Str(p(&m.data));
        let old_bin = m.data.join("rtk/1.2.1-anthology.1/rtk"); // legacy binary
        fs::create_dir_all(old_bin.parent().unwrap()).unwrap();
        fs::write(&old_bin, "old").unwrap();
        let old_state = Value::Str(p(&old_bin));
        state::write(&m.env, vec![("rtk_path".into(), old_state.clone())]).unwrap(); // legacy key
        let history = state::legacy_rtk_data_dir(&m.env).join("history.db");
        fs::create_dir_all(history.parent().unwrap()).unwrap();
        fs::write(&history, "old history").unwrap();
        assert_eq!(
            session_start::reconcile_atl(&m.env, &root, Some(&data), &deps(&real)).unwrap(),
            None
        );
        let st = state::read(&m.env);
        let want = m.data.join("atl").join(VERSION).join("atl");
        assert_eq!(
            st.get("atl_path").and_then(Value::as_str),
            Some(p(&want).as_str())
        );
        assert_eq!(st.get("rtk_path"), Some(&old_state)); // legacy key kept
        assert_eq!(fs::read_to_string(&old_bin).unwrap(), "old");
        assert_eq!(fs::read_to_string(&history).unwrap(), "old history");
    }

    #[test]
    fn test_a_failed_download_is_a_warning_and_state_is_untouched() {
        let m = MemCase::new();
        let blob = release::tarball(ATL_BODY);
        let root = m.tmp.join("plugin");
        let lock = release::write_lock(&root.join("atl.lock"), &blob, VERSION);
        let fetch = Fake(AtlFakeFetch::serving(
            &atl_url_for(&lock, TARGET),
            b"not the pinned tarball",
        ));
        let real = Real(&fetch);
        let data = Value::Str(p(&m.data));
        let warning = session_start::reconcile_atl(&m.env, &root, Some(&data), &deps(&real))
            .unwrap()
            .unwrap();
        assert!(
            warning.contains("atl is not installed or up to date"),
            "{warning}"
        );
        assert!(state::read(&m.env).get("atl_path").is_none());
    }

    #[test]
    fn test_a_missing_lock_is_the_not_wired_note() {
        let m = MemCase::new();
        let root = m.tmp.join("no-lock");
        fs::create_dir(&root).unwrap();
        let fetch = Fake(AtlFakeFetch::default());
        let real = Real(&fetch);
        let data = Value::Str(p(&m.data));
        assert_eq!(
            session_start::reconcile_atl(&m.env, &root, Some(&data), &deps(&real))
                .unwrap()
                .as_deref(),
            Some("anthology: atl wiring not present yet; skipped")
        );
    }
}

// --- GuardSelfTestTest --------------------------------------------------------------------

mod guard_self_test_test {
    use super::*;

    #[test]
    fn test_guard_that_does_not_block_warns() {
        let f = Fx::new();
        f.mark_setup(None);
        let root = f.c.fake_plugins(EXIT_0);
        f.mark_setup(Some(&root));
        assert!(
            sys(&f.go_root(&root))
                .to_lowercase()
                .contains("did not block")
        );
    }

    #[test]
    fn test_guard_that_crashes_warns() {
        let f = Fx::new();
        let root = f.c.fake_plugins("#!/bin/sh\nexit 1\n");
        f.mark_setup(Some(&root));
        assert!(
            sys(&f.go_root(&root))
                .to_lowercase()
                .contains("did not block")
        );
    }

    #[test]
    fn test_guard_that_blocks_is_quiet() {
        let f = Fx::new();
        let root = f.c.fake_plugins(BLOCKS);
        f.mark_setup(Some(&root));
        let out = f.go_root(&root);
        assert!(!has_msg(&out), "{out:?}");
    }

    #[test]
    fn test_slow_guard_times_out_into_a_warning() {
        let f = Fx::new();
        let root = f.c.fake_plugins("#!/bin/sh\nsleep 30\n");
        f.mark_setup(Some(&root));
        let mut d = f.deps();
        d.limits = SessionLimits {
            guard_timeout: Duration::from_secs(1),
        };
        let started = Instant::now();
        let out = f.go_d(&d, None, &f.env().with_os("CLAUDE_PLUGIN_ROOT", &root), &[]);
        let m = sys(&out).to_lowercase();
        assert!(
            m.contains("timed out") || m.contains("did not finish"),
            "{m}"
        );
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn test_guardrails_not_found_warns() {
        let f = Fx::new();
        let root = f.c.fake_plugins(BLOCKS);
        fs::remove_dir_all(root.parent().unwrap().join("anthology-guardrails")).unwrap();
        f.mark_setup(Some(&root));
        let m = sys(&f.go_root(&root)).to_lowercase();
        assert!(
            m.contains("guardrails") && (m.contains("not found") || m.contains("missing")),
            "{m}"
        );
    }

    #[test]
    fn test_guardrails_found_through_installed_plugins_json() {
        let f = Fx::new();
        let root = f.tmp().join("cache-style/anthology/0.1.0");
        fs::create_dir_all(root.join("templates")).unwrap();
        fs::copy(guide_path(), root.join("templates/routing-guide.md")).unwrap();
        copy_bin(&root.join("bin"));
        let guard = guard_plugin(&f.tmp().join("installs/guard-new"), BLOCKS);
        let guard_old = guard_plugin(&f.tmp().join("installs/guard-old"), EXIT_0);
        write(
            &f.config().join("plugins/installed_plugins.json"),
            &format!(
                r#"{{"version": 2, "plugins": {{"anthology-guardrails@anthology": [
                {{"scope": "user", "installPath": {}, "lastUpdated": "2026-01-01T00:00:00Z"}},
                {{"scope": "user", "installPath": {}, "lastUpdated": "2026-06-01T00:00:00Z"}},
                {{"scope": "user", "installPath": {}, "lastUpdated": "2027-01-01T00:00:00Z"}}]}}}}"#,
                jstr(&p(&guard_old)),
                jstr(&p(&guard)),
                jstr(&p(&f.tmp().join("gone")))
            ),
            None,
        );
        f.mark_setup(Some(&root));
        let out = f.go_root(&root);
        assert!(!has_msg(&out), "{out:?}");
    }
}

// --- GuardLocationTest --------------------------------------------------------------------

mod guard_location_test {
    use super::*;

    fn installed(f: &Fx, plugins: &str) {
        write(
            &f.config().join("plugins/installed_plugins.json"),
            &format!(r#"{{"version": 2, "plugins": {plugins}}}"#),
            None,
        );
    }

    fn guard_install(f: &Fx, name: &str) -> PathBuf {
        guard_plugin(&f.tmp().join("installs").join(name), BLOCKS)
    }

    /// anthology as a marketplace install runs it: from `<config>/plugins/cache` (no sibling
    /// fallback). A real install ships its `bin/` wrappers (four since F-9).
    fn cached_root(f: &Fx) -> PathBuf {
        let root = f.config().join("plugins/cache/anthology/anthology/0.1.0");
        fs::create_dir_all(root.join("templates")).unwrap();
        fs::copy(guide_path(), root.join("templates/routing-guide.md")).unwrap();
        for name in ["anthology", "anthology-mem", "atl", "atl-hook"] {
            write_mode(&root.join("bin").join(name), "#!/bin/sh\nexit 0\n", 0o755);
        }
        root
    }

    #[test]
    fn test_anthology_installed_without_guardrails_warns_even_if_a_sibling_folder_exists() {
        let f = Fx::new();
        f.mark_setup(None);
        installed(
            &f,
            &format!(
                r#"{{"anthology@anthology": [{{"scope": "user", "installPath": {}}}]}}"#,
                jstr(&p(f.plugin()))
            ),
        );
        let m = sys(&f.go()).to_lowercase();
        assert!(m.contains("anthology-guardrails is not installed"), "{m}");
    }

    fn settings_with(f: &Fx, extra: &str) -> String {
        format!(
            r#"{{"permissions": {{"allow": [{}]}}, "enabledPlugins": {extra}}}"#,
            jstr(&settings::plugin_read_rule(f.plugin()))
        )
    }

    #[test]
    fn test_disabled_in_user_settings_warns() {
        let f = Fx::new();
        f.mark_setup(None);
        write(
            &f.config().join("settings.json"),
            &settings_with(&f, r#"{"anthology-guardrails@anthology": false}"#),
            None,
        );
        assert!(
            sys(&f.go())
                .to_lowercase()
                .contains("guardrails is disabled")
        );
    }

    #[test]
    fn test_disabled_in_project_and_local_settings_warns() {
        let f = Fx::new();
        f.mark_setup(None);
        for name in ["settings.json", "settings.local.json"] {
            let proj = f.tmp().join(format!("proj-{name}"));
            write(
                &proj.join(".claude").join(name),
                r#"{"enabledPlugins": {"anthology-guardrails@x": false}}"#,
                None,
            );
            assert!(
                sys(&f.go_cwd(&proj))
                    .to_lowercase()
                    .contains("guardrails is disabled"),
                "{name}"
            );
        }
    }

    #[test]
    fn test_enabled_true_is_fine() {
        let f = Fx::new();
        f.mark_setup(None);
        write(
            &f.config().join("settings.json"),
            &settings_with(&f, r#"{"anthology-guardrails@anthology": true}"#),
            None,
        );
        let out = f.go();
        assert!(!has_msg(&out), "{out:?}");
    }

    #[test]
    fn test_registry_entry_wins_over_the_sibling_folder() {
        let f = Fx::new();
        let root = cached_root(&f);
        // a sibling that would NOT block
        guard_plugin(&root.parent().unwrap().join("anthology-guardrails"), EXIT_0);
        f.mark_setup(Some(&root));
        installed(
            &f,
            &format!(
                r#"{{"anthology-guardrails@anthology": [{{"scope": "user", "installPath": {}}}]}}"#,
                jstr(&p(&guard_install(&f, "guard")))
            ),
        );
        let out = f.go_root(&root);
        assert!(!has_msg(&out), "{out:?}");
    }

    #[test]
    fn test_project_scoped_install_of_another_project_is_ignored() {
        let f = Fx::new();
        f.mark_setup(None);
        let (other, mine) = (f.tmp().join("other-proj"), f.tmp().join("my-proj"));
        fs::create_dir(&other).unwrap();
        fs::create_dir(&mine).unwrap();
        installed(
            &f,
            &format!(
                r#"{{"anthology@anthology": [{{"scope": "user", "installPath": {}}}],
                "anthology-guardrails@anthology": [{{"scope": "project", "projectPath": {}, "installPath": {}}}]}}"#,
                jstr(&p(f.plugin())),
                jstr(&p(&other)),
                jstr(&p(&guard_install(&f, "a")))
            ),
        );
        assert!(sys(&f.go_cwd(&mine)).contains("not installed"));
        fs::create_dir(other.join("sub")).unwrap();
        let out = f.go_cwd(&other.join("sub"));
        assert!(!has_msg(&out), "{out:?}");
    }

    #[test]
    fn test_newest_applicable_scope_wins() {
        let f = Fx::new();
        let root = cached_root(&f);
        f.mark_setup(Some(&root));
        let proj = f.tmp().join("proj");
        fs::create_dir(&proj).unwrap();
        let bad = guard_plugin(&f.tmp().join("installs/bad"), EXIT_0);
        installed(
            &f,
            &format!(
                r#"{{"anthology-guardrails@anthology": [
                {{"scope": "user", "installPath": {}, "lastUpdated": "2026-01-01T00:00:00Z"}},
                {{"scope": "project", "projectPath": {}, "installPath": {}, "lastUpdated": "2026-06-01T00:00:00Z"}}]}}"#,
                jstr(&p(&bad)),
                jstr(&p(&proj)),
                jstr(&p(&guard_install(&f, "guard")))
            ),
        );
        let env = f.env().with_os("CLAUDE_PLUGIN_ROOT", &root);
        let out = f.go_d(&f.deps(), Some(&proj), &env, &[]);
        assert!(!has_msg(&out), "{out:?}");
        let out = f.go_d(&f.deps(), Some(f.tmp()), &env, &[]);
        assert!(sys(&out).to_lowercase().contains("did not block"));
    }

    #[test]
    fn test_exit_2_without_the_guards_message_is_not_a_pass() {
        let f = Fx::new();
        let root = f.c.fake_plugins("#!/bin/sh\ncat >/dev/null\nexit 2\n");
        f.mark_setup(Some(&root));
        assert!(
            sys(&f.go_root(&root))
                .to_lowercase()
                .contains("did not block")
        );
    }
}

// --- DeadlineTest -------------------------------------------------------------------------

mod deadline_test {
    use super::*;

    #[test]
    fn test_rtk_reconcile_is_skipped_with_a_note_when_time_is_short() {
        let f = Fx::new();
        f.mark_setup(None);
        let late = || DEADLINE - 10.0;
        let mut d = f.deps();
        d.elapsed = Some(&late);
        let out = f.go_d(&d, None, &f.env(), &[]);
        assert!(f.atl.calls.borrow().is_empty());
        assert!(lines(&out).contains(
            &"anthology: skipped the atl check this session (not enough time left)".to_string()
        ));
        assert!(context(&out).contains("# Anthology routing guide"));
    }

    #[test]
    fn test_rtk_reconcile_runs_when_there_is_time() {
        let f = Fx::new();
        f.mark_setup(None);
        let early = || 1.0;
        let mut d = f.deps();
        d.elapsed = Some(&early);
        f.go_d(&d, None, &f.env(), &[]);
        assert_eq!(f.atl.calls.borrow().len(), 1);
    }
}

// --- SkillsFolderTest and GuideTest: the plugin's files -----------------------------------

fn frontmatter(text: &str) -> &str {
    text.split("\n---").next().unwrap_or("")
}

mod skills_folder_test {
    use super::*;

    #[test]
    fn test_skills_folder_is_the_three_commands_and_large_builds() {
        let mut names: Vec<String> = fs::read_dir(real_plugin().join("skills"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, vec!["export", "import", "large-builds", "setup"]);
    }

    #[test]
    fn test_large_builds_is_model_invoked_and_names_real_skills() {
        let text = fs::read_to_string(real_plugin().join("skills/large-builds/SKILL.md")).unwrap();
        let head = text.split("\n---").next().unwrap();
        let lines: Vec<&str> = head.lines().collect();
        assert!(lines.contains(&"name: large-builds"));
        assert!(lines.iter().any(|l| {
            l.strip_prefix("description: ")
                .and_then(|r| r.chars().next())
                .is_some_and(|c| !c.is_whitespace())
        }));
        assert!(!head.contains("disable-model-invocation"));
        // Python: re.findall(r"anthology-superpowers:([a-z-]+)", text)
        let mut refs = std::collections::BTreeSet::new();
        for (i, _) in text.match_indices("anthology-superpowers:") {
            let rest = &text[i + "anthology-superpowers:".len()..];
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_lowercase() || *c == '-')
                .collect();
            if !name.is_empty() {
                refs.insert(name);
            }
        }
        assert!(!refs.is_empty());
        let skills = real_plugin().join("../anthology-superpowers/skills");
        for r in &refs {
            assert!(skills.join(r).join("SKILL.md").is_file(), "{r}");
        }
    }

    #[test]
    fn test_every_command_skill_is_manual_only() {
        for name in ["setup", "export", "import"] {
            let text = fs::read_to_string(real_plugin().join("skills").join(name).join("SKILL.md"))
                .unwrap();
            let head: Vec<&str> = frontmatter(&text).lines().collect();
            assert!(head.contains(&format!("name: {name}").as_str()), "{name}");
            assert!(
                head.iter().any(|l| l
                    .strip_prefix("description: ")
                    .and_then(|r| r.chars().next())
                    .is_some_and(|c| !c.is_whitespace())),
                "{name}"
            );
            assert!(head.contains(&"disable-model-invocation: true"), "{name}");
        }
    }

    #[test]
    fn test_setup_skill_content() {
        let text = fs::read_to_string(real_plugin().join("skills/setup/SKILL.md")).unwrap();
        for want in [
            "--plugin-root \"${CLAUDE_PLUGIN_ROOT}\"",
            "\"${CLAUDE_PLUGIN_ROOT}/bin/anthology\" setup ",
            "/anthology:setup remove",
            "atl bypass <cmd>",
            "--expect-sha256",
            "claude plugin uninstall",
        ] {
            assert!(text.contains(want), "{want}");
        }
        assert!(!text.contains(&format!("/U{}", "sers/")));
        assert!(!has_email_like(&text, &["com", "org", "dev"], true));
    }
}

/// `@[a-z0-9.-]+\.(com|org|dev)` (`strict`) or `@\w+\.\w+`.
fn has_email_like(text: &str, tlds: &[&str], strict: bool) -> bool {
    let b = text.as_bytes();
    for (i, _) in text.match_indices('@') {
        if strict {
            let mut j = i + 1;
            while j < b.len()
                && (b[j].is_ascii_lowercase()
                    || b[j].is_ascii_digit()
                    || b[j] == b'.'
                    || b[j] == b'-')
            {
                j += 1;
            }
            let run = &text[i + 1..j];
            // backtracking: any `.` inside the run followed by a TLD
            for (k, _) in run.match_indices('.') {
                if k > 0 && tlds.iter().any(|t| run[k + 1..].starts_with(t)) {
                    return true;
                }
            }
        } else {
            let word = |c: char| c.is_alphanumeric() || c == '_';
            let rest = &text[i + 1..];
            let first: String = rest.chars().take_while(|c| word(*c)).collect();
            if first.is_empty() {
                continue;
            }
            let after = &rest[first.len()..];
            if let Some(tail) = after.strip_prefix('.')
                && tail.chars().next().is_some_and(word)
            {
                return true;
            }
        }
    }
    false
}

mod guide_test {
    use super::*;

    #[test]
    fn test_size_and_shape() {
        let text = guide();
        assert!(text.chars().count() <= 3800);
        assert!(!text.starts_with("---"));
        for n in 1..9 {
            let head = format!("## {n}. ");
            assert!(text.lines().any(|l| l.starts_with(&head)), "{n}");
        }
    }

    #[test]
    fn test_uses_anthology_ids_for_every_roster_agent() {
        let text = guide();
        for agent in AGENTS {
            assert!(text.contains(&format!("anthology:{agent}")), "{agent}");
        }
        assert!(text.contains("claude-security"));
        assert!(text.contains("ui-designer builds, then ui-reviewer checks"));
    }

    #[test]
    fn test_lead_hands_off_all_making_and_changing() {
        let text = guide();
        assert!(!text.contains("do it inline"));
        let line = |pred: &dyn Fn(&str) -> bool| -> String {
            text.lines()
                .find(|l| pred(l))
                .expect("guide line")
                .to_string()
        };
        let lead = line(&|l| l.starts_with("- Lead:"));
        assert!(
            lead.contains(
                "plan, decide, brief agents, check results, merge. Never produce the work"
            ),
            "{lead}"
        );
        let hand = line(&|l| l.contains("matching agent"));
        for word in [
            "code", "files", "docs", "designs", "canvas", "MCP", "browser",
        ] {
            assert!(hand.contains(word), "{word}");
        }
        let inline = line(&|l| l.starts_with("- Inline only:"));
        for part in [
            "reads",
            "lookups",
            "answering the user",
            "at most a few lines in one file the user asked for",
        ] {
            assert!(inline.contains(part), "{part}");
        }
        let ui = line(&|l| l.contains("ui-designer builds, then ui-reviewer checks"));
        assert!(ui.contains("canvas"), "{ui}");
    }

    #[test]
    fn test_browser_rules() {
        let text = guide();
        assert!(text.contains("BrowserOS Neo"));
        assert!(text.contains("BrowserOS Neo (MCP), the only browser"));
        assert!(text.contains("BrowserOS Neo unavailable: stop and ask. Never install a browser"));
        assert!(text.contains("Never launch Chrome"));
        for tool in [
            "agent-browser",
            "Playwright",
            "chrome-devtools",
            "Claude in Chrome",
        ] {
            assert!(text.contains(tool), "{tool}");
        }
        for line in text.lines() {
            if ["agent-browser", "chrome-devtools", "Claude in Chrome"]
                .iter()
                .any(|t| line.contains(t))
            {
                assert!(line.contains("Never "), "{line}");
            }
        }
    }

    #[test]
    fn test_worktree_docker_rtk_and_memory_rules() {
        let text = guide();
        for want in [
            "worktree.baseRef: \"head\"",
            "never `git worktree add`",
            "chore(memory)",
            "commit before finishing",
            "Docker unavailable: stop and tell the user",
            "atl bypass <cmd>",
            "anthology-mem search",
            "[[note-name]]",
            "Never save secrets",
        ] {
            assert!(text.contains(want), "{want}");
        }
        // Before each merge or pull the lead commits any uncommitted agent memory.
        assert!(
            text.contains(
                "Before each merge or pull, and at task end, commit any uncommitted \
                 `.claude/agent-memory/` changes, whichever agent wrote them"
            ),
            "merge/pull memory rule"
        );
    }

    // The text of one `## n. ` section of the guide, up to the next heading.
    fn section_of(text: &str, n: u32) -> String {
        let head = format!("## {n}. ");
        let mut lines = text.lines().skip_while(|l| !l.starts_with(&head));
        let mut out = vec![lines.next().expect("section heading")];
        out.extend(lines.take_while(|l| !l.starts_with("## ")));
        out.join("\n")
    }

    #[test]
    fn test_deletes_are_literal_paths_only() {
        let text = guide();
        let docker = section_of(&text, 5);
        assert!(
            docker.contains(
                "- Delete literal paths only, never `rm -rf $VAR`: Claude Code asks on \
                 removals it can't prove safe."
            ),
            "{docker}"
        );
    }

    #[test]
    fn test_commits_and_prs_carry_only_the_git_author() {
        let text = guide();
        let worktrees = section_of(&text, 4);
        assert!(
            worktrees.contains(
                "- Commits, PRs and code comments carry only the git config author: never add \
                 Co-Authored-By or \"Generated with\" lines, never mention Claude or AI."
            ),
            "{worktrees}"
        );
    }

    #[test]
    fn test_lead_reads_the_project_map_before_planning() {
        let text = guide();
        let roles = section_of(&text, 1);
        assert!(
            roles.contains(
                "- Before planning, read `.claude/memory/reference-project-map.md`; \
                 missing: dispatch anthology:architect."
            ),
            "{roles}"
        );
    }

    #[test]
    fn test_personal_index_is_below_the_guide() {
        let text = guide();
        assert!(text.contains("personal memory at the path given below"));
        assert!(!text.contains("above"));
    }

    #[test]
    fn test_user_notes_never_go_into_the_repo() {
        let text = guide();
        let line = text
            .lines()
            .find(|l| l.starts_with("- user notes"))
            .expect("a user notes line");
        assert!(line.contains("never into the repo"));
        assert!(line.contains("Never commit a `user` note"));
    }

    #[test]
    fn test_no_personal_details() {
        let text = guide();
        assert!(!text.contains(&format!("/U{}", "sers/")));
        let home_word = text.match_indices("/home/").any(|(i, _)| {
            text[i + 6..]
                .chars()
                .next()
                .is_some_and(|c| c.is_alphanumeric() || c == '_')
        });
        assert!(!home_word);
        assert!(!has_email_like(&text, &[], false));
    }
}

// --- test_guardrails_loc SelfTestFollowsTest ----------------------------------------------

mod self_test_follows_test {
    use super::*;

    /// `LiveRootCase` inside a `SessionCase`.
    struct Live {
        f: Fx,
        cache: PathBuf,
        market: PathBuf,
        live: PathBuf,
    }

    fn live() -> Live {
        let f = Fx::new();
        let cache = f
            .config()
            .join("plugins/cache/anthology/anthology-guardrails/0.1.0");
        let market = f.tmp().join("market");
        let live = market.join("plugins/anthology-guardrails");
        Live {
            f,
            cache,
            market,
            live,
        }
    }

    impl Live {
        /// `copy_guardrails(dest, guard)`: the real guard is the blocking stand-in.
        fn copy_guardrails(&self, dest: &Path, guard: Option<&str>) {
            guard_plugin(dest, guard.unwrap_or(BLOCKS));
        }

        fn installed(&self) {
            write(
                &self.f.config().join("plugins/installed_plugins.json"),
                &format!(
                    r#"{{"version": 2, "plugins": {{"anthology-guardrails@anthology": [{{"scope": "user", "installPath": {}, "version": "0.1.0", "lastUpdated": "2026-01-01T00:00:00Z"}}]}}}}"#,
                    jstr(&p(&self.cache))
                ),
                None,
            );
        }

        fn known(&self, source: &str, location: &Path) {
            write(
                &self.f.config().join("plugins/known_marketplaces.json"),
                &format!(
                    r#"{{"anthology": {{"source": {source}, "installLocation": {}, "lastUpdated": "x"}}}}"#,
                    jstr(&p(location))
                ),
                None,
            );
        }

        fn directory_known(&self) {
            self.known(
                &format!(
                    r#"{{"source": "directory", "path": {}}}"#,
                    jstr(&p(&self.market))
                ),
                &self.market,
            );
        }

        fn manifest(&self) {
            write(
                &self.market.join(".claude-plugin/marketplace.json"),
                r#"{"name": "anthology", "plugins": [{"name": "anthology-guardrails", "source": "./plugins/anthology-guardrails"}]}"#,
                None,
            );
        }

        fn diverging(&self, live_guard: Option<&str>, cache_guard: Option<&str>) {
            self.copy_guardrails(&self.cache, cache_guard);
            self.copy_guardrails(&self.live, live_guard);
            self.manifest();
            self.directory_known();
            self.installed();
        }
    }

    #[test]
    fn test_live_copy_blocks_while_the_cache_copy_does_not_is_quiet() {
        let l = live();
        l.diverging(None, Some(EXIT_0));
        l.f.mark_setup(None); // after the layout exists: setup records the op set it gives
        let out = l.f.go();
        assert!(!has_msg(&out), "{out:?}");
    }

    #[test]
    fn test_live_copy_that_does_not_block_warns_though_the_cache_copy_does() {
        let l = live();
        l.f.mark_setup(None);
        l.diverging(Some(EXIT_0), None);
        assert!(sys(&l.f.go()).to_lowercase().contains("did not block"));
    }

    #[test]
    fn test_live_folder_without_guard_py_warns_and_never_runs_the_cache_copy() {
        let l = live();
        l.f.mark_setup(None);
        l.copy_guardrails(&l.cache, None); // the real guard: it would block and pass
        fs::create_dir_all(l.live.join("hooks")).unwrap();
        l.manifest();
        l.directory_known();
        l.installed();
        let m = sys(&l.f.go());
        assert!(
            m.contains("anthology-guardrails folder this session runs has no hooks/guard.sh"),
            "{m}"
        );
        assert!(m.contains("guardrails may be OFF"));
        assert!(m.contains("reinstall or repair"));
        assert!(m.chars().count() < 400);
    }

    #[test]
    fn test_github_marketplace_checks_the_cache_copy() {
        let l = live();
        l.diverging(Some(EXIT_0), None);
        l.known(
            r#"{"source": "github", "repo": "owner/anthology"}"#,
            &l.f.tmp().join("gh"),
        );
        l.f.mark_setup(None);
        let out = l.f.go();
        assert!(!has_msg(&out), "{out:?}");
    }
}

// --- Rust-only: a session folder that is not a git repo -----------------------------------

mod not_a_repo {
    use super::*;

    fn repo(dir: &Path) {
        fs::create_dir_all(dir.join(".git")).unwrap();
    }

    /// `<tmp>/<name>` with the given children: a name ending `.git` is a repo (a folder, or a
    /// file for `file.git`, as a linked worktree has), any other name a plain folder.
    fn parent(f: &Fx, name: &str, children: &[&str]) -> PathBuf {
        let dir = f.tmp().join(name);
        fs::create_dir_all(&dir).unwrap();
        for child in children {
            let path = dir.join(child.trim_end_matches(".git"));
            match *child {
                "file.git" => {
                    fs::create_dir_all(&path).unwrap();
                    fs::write(path.join(".git"), "gitdir: /elsewhere\n").unwrap();
                }
                c if c.ends_with(".git") => repo(&path),
                _ => fs::create_dir_all(&path).unwrap(),
            }
        }
        dir
    }

    fn warned(f: &Fx, cwd: &Path) -> bool {
        let out = f.go_cwd(cwd);
        assert!(
            lines(&out).iter().all(|l| l == session_start::NOT_A_REPO),
            "{out:?}"
        );
        lines(&out).contains(&session_start::NOT_A_REPO.to_string())
    }

    #[test]
    fn a_folder_that_holds_repos_gets_one_short_warning() {
        let f = Fx::new();
        f.mark_setup(None);
        let dir = parent(&f, "projects", &["a.git", "notes", "b.git"]);
        let out = f.go_cwd(&dir);
        assert_eq!(lines(&out), vec![session_start::NOT_A_REPO.to_string()]);
        assert!(session_start::NOT_A_REPO.contains("worktrees"));
        assert!(session_start::NOT_A_REPO.contains("inside the repo"));
        assert!(
            session_start::NOT_A_REPO.chars().count() < 200,
            "keep it short"
        );
        // The guide still arrives.
        assert!(context(&out).contains("# Anthology routing guide"));
    }

    #[test]
    fn a_linked_worktree_below_counts_as_a_repo() {
        let f = Fx::new();
        f.mark_setup(None);
        let dir = parent(&f, "projects", &["file.git"]);
        assert!(warned(&f, &dir));
    }

    #[test]
    fn inside_a_repo_there_is_no_warning() {
        let f = Fx::new();
        f.mark_setup(None);
        let dir = parent(&f, "work", &["inner.git"]);
        repo(&dir);
        assert!(!warned(&f, &dir));
        fs::create_dir_all(dir.join("src/deep")).unwrap();
        assert!(!warned(&f, &dir.join("src/deep")));
    }

    #[test]
    fn a_folder_with_no_repos_below_it_gets_no_warning() {
        let f = Fx::new();
        f.mark_setup(None);
        let dir = parent(&f, "plain", &["one", "two"]);
        fs::write(dir.join("file.txt"), "x").unwrap();
        assert!(!warned(&f, &dir));
    }

    #[test]
    fn only_one_level_down_is_looked_at() {
        let f = Fx::new();
        f.mark_setup(None);
        let dir = f.tmp().join("group");
        repo(&dir.join("team/service"));
        assert!(!warned(&f, &dir));
    }

    #[test]
    fn a_missing_folder_gets_no_warning() {
        let f = Fx::new();
        f.mark_setup(None);
        assert!(!warned(&f, &f.tmp().join("gone")));
    }

    #[test]
    fn a_subagent_never_gets_it() {
        let f = Fx::new();
        f.mark_setup(None);
        let dir = parent(&f, "projects", &["a.git"]);
        assert!(warned(&f, &dir));
        let out = f.go_d(
            &f.deps(),
            Some(&dir),
            &f.env(),
            &[("source", "compact"), ("agent_id", "a0b1c2d3e4f5a6b7c")],
        );
        assert!(!has_msg(&out), "{out:?}");
    }

    #[test]
    fn the_step_is_isolated() {
        let f = Fx::new();
        f.mark_setup(None);
        let dir = parent(&f, "projects", &["a.git"]);
        let fail = |n: &str| (n == "_not_a_repo_warning").then(|| exc("RuntimeError", "boom"));
        let mut d = f.deps();
        d.fail = &fail;
        let out = f.go_d(&d, Some(&dir), &f.env(), &[]);
        assert!(context(&out).contains("# Anthology routing guide"));
        assert_eq!(
            lines(&out),
            vec!["anthology: the git folder check failed (RuntimeError)".to_string()]
        );
    }
}

// --- Rust-only: GC-7 time rules, F-4, the new step, the prune -----------------------------

mod time_rules {
    use super::*;

    #[test]
    fn budget_formula_ensures_only_with_twenty_seconds_to_spare() {
        let env = Env::default().with("ANTHOLOGY_BOOTSTRAP_DOWNLOADED", "0");
        let t = Duration::from_secs(10);
        assert_eq!(session_start::ensure_budget(&env, 0.0, t), Some(20));
        assert_eq!(session_start::ensure_budget(&env, 15.0, t), Some(20));
        assert_eq!(session_start::ensure_budget(&env, 15.5, t), None);
        assert_eq!(session_start::ensure_budget(&env, 40.0, t), None);
        for v in [None, Some("1"), Some(""), Some("00"), Some(" 0")] {
            let env = match v {
                Some(v) => Env::default().with("ANTHOLOGY_BOOTSTRAP_DOWNLOADED", v),
                None => Env::default(),
            };
            assert_eq!(session_start::ensure_budget(&env, 0.0, t), None, "{v:?}");
        }
    }

    #[test]
    fn the_live_guard_binary_is_ensured_once_with_the_budget_before_the_self_test() {
        let f = Fx::new();
        f.mark_setup(None);
        let order: RefCell<Vec<String>> = RefCell::new(Vec::new());
        let ensure = |root: &Path, budget: u64, env: &Env| {
            assert_eq!(env.get("ANTHOLOGY_BOOTSTRAP_DOWNLOADED"), Some("0"));
            order
                .borrow_mut()
                .push(format!("ensure {} {budget}", p(root)));
            GuardBinaryStatus::Present
        };
        let test = |_: &Env, _: &Path, _: &Path| {
            order.borrow_mut().push("self-test".into());
            Ok(None)
        };
        let early = || 2.0;
        let mut d = f.deps();
        d.ensure_guard_binary = &ensure;
        d.self_test = Some(&test);
        d.elapsed = Some(&early);
        let env = f.env().with("ANTHOLOGY_BOOTSTRAP_DOWNLOADED", "0");
        let out = f.go_d(&d, None, &env, &[]);
        assert!(!has_msg(&out), "{out:?}");
        assert_eq!(
            *order.borrow(),
            vec![
                format!("ensure {} 20", p(&f.c.guard)),
                "self-test".to_string()
            ]
        );
    }

    #[test]
    fn a_download_in_this_hook_or_little_time_left_defers_the_guard_binary() {
        let f = Fx::new();
        f.mark_setup(None);
        let late = || 16.0;
        let mut d = f.deps(); // no_ensure panics, which would be a step failure line
        d.elapsed = Some(&late);
        let out = f.go_d(
            &d,
            None,
            &f.env().with("ANTHOLOGY_BOOTSTRAP_DOWNLOADED", "0"),
            &[],
        );
        // 16 s in: no ensure, and the atl check is skipped too (F-20)
        assert_eq!(lines(&out), vec![session_start::ATL_SKIPPED.to_string()]);
        let out = f.go_d(
            &f.deps(),
            None,
            &f.env().with("ANTHOLOGY_BOOTSTRAP_DOWNLOADED", "1"),
            &[],
        );
        assert!(!has_msg(&out), "{out:?}");
    }

    #[test]
    fn with_no_time_the_guard_self_test_is_not_run_and_a_missing_binary_still_fails_closed() {
        let f = Fx::new();
        f.mark_setup(None);
        let ran = RefCell::new(0u32);
        let spy = |spec: &RunSpec<'_>| {
            *ran.borrow_mut() += 1;
            session_start::run_with_timeout(spec)
        };
        // DEADLINE 50 s, self-test limit 10 s: it still fits at 40 s, not after.
        let fits = || 40.0;
        let mut d = f.deps();
        d.run = &spy;
        d.elapsed = Some(&fits);
        let out = f.go_d(&d, None, &f.env(), &[]);
        // The self-test is two guard runs: the plain sample and the one through an ATL runner.
        assert_eq!(*ran.borrow(), 2);
        assert_eq!(lines(&out), vec![session_start::ATL_SKIPPED.to_string()]);

        let late = || 40.5;
        d.elapsed = Some(&late);
        let out = f.go_d(&d, None, &f.env(), &[]);
        assert_eq!(*ran.borrow(), 2, "the self-test must not run with no time");
        assert_eq!(
            lines(&out),
            vec![
                format!(
                    "anthology: the guardrails are failing closed: {}",
                    session_start::BINARY_MISSING_REASON
                ),
                session_start::ATL_SKIPPED.to_string()
            ]
        );
    }

    #[test]
    fn with_no_time_a_missing_guardrails_plugin_is_still_named() {
        // Where the guard is needs no process, so lateness must not hide a plugin that is gone.
        let f = Fx::new();
        f.mark_setup(None);
        fs::remove_dir_all(&f.c.guard).unwrap();
        let late = || 45.0;
        let mut d = f.deps();
        d.elapsed = Some(&late);
        let out = f.go_d(&d, None, &f.env(), &[]);
        assert_eq!(
            lines(&out),
            vec![
                session_start::GUARD_NOT_FOUND.to_string(),
                session_start::ATL_SKIPPED.to_string()
            ]
        );
    }

    #[test]
    fn a_failing_ensure_step_is_named_with_the_new_label() {
        let f = Fx::new();
        f.mark_setup(None);
        let fail = |n: &str| (n == "ensure").then(|| exc("RuntimeError", "x"));
        let mut d = f.deps();
        d.fail = &fail;
        assert_eq!(
            lines(&f.go_d(&d, None, &f.env(), &[])),
            vec!["anthology: ensuring the guardrails binary failed (RuntimeError)".to_string()]
        );
        // a panic in the step is caught the same way
        let early = || 0.0;
        let mut d = f.deps();
        d.elapsed = Some(&early);
        let out = f.go_d(
            &d,
            None,
            &f.env().with("ANTHOLOGY_BOOTSTRAP_DOWNLOADED", "0"),
            &[],
        );
        assert_eq!(
            lines(&out),
            vec!["anthology: ensuring the guardrails binary failed (panic)".to_string()]
        );
    }

    fn now() -> f64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64()
    }

    #[test]
    fn the_atl_check_counts_from_the_hook_t0() {
        // F-20: a hook that started 12 s ago (it downloaded the binary) skips the atl check.
        let f = Fx::new();
        f.mark_setup(None);
        let t0 = format!("{}", now() as u64 - 12);
        let out = f.go_d(
            &f.deps(),
            None,
            &f.env().with("ANTHOLOGY_HOOK_T0", &t0),
            &[],
        );
        assert!(f.atl.calls.borrow().is_empty());
        assert_eq!(lines(&out), vec![session_start::ATL_SKIPPED.to_string()]);
        // 5 s ago: time enough
        let t0 = format!("{}", now() as u64 - 5);
        f.go_d(
            &f.deps(),
            None,
            &f.env().with("ANTHOLOGY_HOOK_T0", &t0),
            &[],
        );
        assert_eq!(f.atl.calls.borrow().len(), 1);
        // a T0 no hook could have (older than the 60 s hook timeout, garbage, the future)
        // counts from this run's start instead
        for t0 in [
            format!("{}", now() as u64 - 3600),
            "abc".to_string(),
            format!("{}", now() as u64 + 100),
            String::new(),
        ] {
            f.go_d(
                &f.deps(),
                None,
                &f.env().with("ANTHOLOGY_HOOK_T0", &t0),
                &[],
            );
        }
        assert_eq!(f.atl.calls.borrow().len(), 5);
    }
}

mod f4_self_test {
    use super::*;

    #[test]
    fn only_a_g1_block_passes() {
        assert_eq!(
            session_start::self_test_verdict(2, b"Blocked by anthology-guardrails (G1): x\n"),
            None
        );
        for (code, err) in [
            (2, &b"Blocked by anthology-guardrails (G2): x\n"[..]),
            (2, b"Blocked by anthology-guardrails (internal): x\n"),
            (0, b"Blocked by anthology-guardrails (G1): x\n"),
            (2, b""),
            (-9, b""),
        ] {
            let v = session_start::self_test_verdict(code, err).unwrap();
            assert_eq!(
                v,
                format!(
                    "anthology: the guardrails self-test did NOT block a known-bad command (exit {code}); the guardrails may be OFF"
                )
            );
        }
    }

    #[test]
    fn a_setup_block_reports_the_guards_own_reason() {
        let reason = "the guard binary is not installed (not attempted), so shell commands, file edits and MCP tools are blocked; run /anthology:setup (or start a new session while online) to install it; if this keeps happening, disable anthology-guardrails with /plugin";
        let v = session_start::self_test_verdict(
            2,
            format!("Blocked by anthology-guardrails (setup): {reason}\n").as_bytes(),
        )
        .unwrap();
        assert_eq!(
            v,
            format!("anthology: the guardrails are failing closed: {reason}")
        );
        let long = "é".repeat(400);
        let v = session_start::self_test_verdict(
            2,
            format!("Blocked by anthology-guardrails (setup): {long}\n").as_bytes(),
        )
        .unwrap();
        let shown = v
            .strip_prefix("anthology: the guardrails are failing closed: ")
            .unwrap();
        assert_eq!(shown.chars().count(), 300);
        assert!(shown.ends_with('…'));
    }

    #[test]
    fn the_guard_gets_the_sample_with_no_fetch() {
        let f = Fx::new();
        f.mark_setup(None);
        let seen = f.tmp().join("seen");
        let body = format!(
            "#!/bin/sh\ncat >> {0}.in\necho >> {0}.in\nprintf '%s' \"$ANTHOLOGY_GUARD_NO_FETCH\" > {0}.env\nprintf 'Blocked by anthology-guardrails (G1): x\\n' >&2\nexit 2\n",
            p(&seen)
        );
        let root = f.c.fake_plugins(&body);
        f.mark_setup(Some(&root));
        let out = f.go_root(&root);
        assert!(!has_msg(&out), "{out:?}");
        // the plain sample, then the same command run through an ATL runner
        assert_eq!(
            fs::read_to_string(seen.with_extension("in")).unwrap(),
            concat!(
                r#"{"hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {"command": "npx playwright install chromium"}}"#,
                "\n",
                r#"{"hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {"command": "atl run -c 'npx playwright install'"}}"#,
                "\n"
            )
        );
        assert_eq!(fs::read_to_string(seen.with_extension("env")).unwrap(), "1");
    }

    #[test]
    fn a_guard_too_old_to_look_inside_atl_warns() {
        // A guard binary from before the rename (bin 0.1.1) blocks the plain sample but lets
        // `atl run -c '…'` through: the self-test must not pass quietly.
        let f = Fx::new();
        let root = f.c.fake_plugins(
            "#!/bin/sh\nin=$(cat)\ncase $in in *'atl run'*) printf 'anthology-guard:allow:v1\\n' >&2; exit 0 ;; esac\nprintf 'Blocked by anthology-guardrails (G1): x\\n' >&2\nexit 2\n",
        );
        f.mark_setup(Some(&root));
        let got = lines(&f.go_root(&root));
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(got[0].contains("atl"), "{got:?}");
        assert!(got[0].contains("the guardrails may be OFF"), "{got:?}");
    }

    #[test]
    fn a_shim_that_fails_closed_is_reported_through_the_hook() {
        let f = Fx::new();
        let root = f.c.fake_plugins(
            "#!/bin/sh\ncat >/dev/null\nprintf 'Blocked by anthology-guardrails (setup): the guard binary is not installed yet; retry\\n' >&2\nexit 2\n",
        );
        f.mark_setup(Some(&root));
        assert_eq!(
            lines(&f.go_root(&root)),
            vec![
                "anthology: the guardrails are failing closed: the guard binary is not installed yet; retry"
                    .to_string()
            ]
        );
    }

    #[test]
    fn a_guard_that_cannot_start_names_the_os_error() {
        let f = Fx::new();
        let missing = |_: &RunSpec<'_>| {
            RunResult::Failed(PyExc::OSError {
                errno: 2,
                filename: None,
            })
        };
        let mut d = f.deps();
        d.run = &missing;
        f.mark_setup(None);
        assert_eq!(
            lines(&f.go_d(&d, None, &f.env(), &[])),
            vec![
                "anthology: the guardrails self-test could not run (No such file or directory); the guardrails may be OFF"
                    .to_string()
            ]
        );
    }

    #[test]
    fn a_hung_guard_and_its_children_are_killed() {
        let f = Fx::new();
        let pidfile = f.tmp().join("child.pid");
        let root = f.c.fake_plugins(&format!(
            "#!/bin/sh\nsleep 30 &\necho $! > {}\nwait\n",
            p(&pidfile)
        ));
        f.mark_setup(Some(&root));
        let mut d = f.deps();
        d.limits = SessionLimits {
            guard_timeout: Duration::from_secs(1),
        };
        let started = Instant::now();
        let out = f.go_d(&d, None, &f.env().with_os("CLAUDE_PLUGIN_ROOT", &root), &[]);
        assert!(sys(&out).contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(10));
        let pid = fs::read_to_string(&pidfile).unwrap();
        // the background sleep of the hung guard is gone with its group (polled: a killed child
        // may be reaped a moment later; a zombie counts as gone, since a container's PID 1 may
        // never reap the orphan)
        let check = format!(
            "kill -0 {pid} 2>/dev/null || exit 1; \
             [ -r /proc/{pid}/stat ] || exit 0; \
             [ \"$(cut -d' ' -f3 /proc/{pid}/stat)\" != Z ]",
            pid = pid.trim()
        );
        let alive = || {
            std::process::Command::new("/bin/sh")
                .args(["-c", &check])
                .status()
                .unwrap()
                .success()
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        while alive() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!alive());
    }
}

mod prune {
    use super::*;
    use anthology_core::lock::{ANTHOLOGY_TAG_PREFIX, render_anthology};
    use anthology_core::platform::Target;
    use std::collections::BTreeMap;

    fn lock(version: &str) -> String {
        let mut sha256 = BTreeMap::new();
        for t in Target::ALL {
            sha256.insert(t, "0".repeat(64));
        }
        render_anthology(&Lock {
            version: version.into(),
            tag: format!("{ANTHOLOGY_TAG_PREFIX}{version}"),
            repo: "example/anthology".into(),
            sha256,
        })
    }

    fn set_mtime(path: &Path, secs_ago: u64) {
        let t = SystemTime::now() - Duration::from_secs(secs_ago);
        fs::File::options()
            .write(true)
            .open(path)
            .or_else(|_| fs::File::open(path))
            .unwrap()
            .set_modified(t)
            .unwrap();
    }

    fn now() -> f64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64()
    }

    #[test]
    fn prune_removes_old_temps_old_quarantines_by_name_and_unpinned_old_versions() {
        let f = Fx::new();
        let env = f.env();
        let bin = f.c.mem.anth.join("bin");
        let day = 86_400u64;
        let n = now() as u64;
        fs::write(f.plugin().join("anthology.lock"), lock("0.2.0")).unwrap();
        fs::write(f.c.guard.join("anthology.lock"), lock("0.1.0")).unwrap();
        for v in ["0.1.0", "0.2.0", "0.0.9", "0.0.8"] {
            write(&bin.join(v).join(".sha256"), "x\n", None);
            write(&bin.join(v).join(".last-used"), "", None);
        }
        set_mtime(&bin.join("0.0.9/.last-used"), 31 * day);
        set_mtime(&bin.join("0.1.0/.last-used"), 31 * day); // pinned by the guard: kept
        set_mtime(&bin.join("0.0.8/.last-used"), 29 * day);
        fs::create_dir_all(bin.join(".tmp-old")).unwrap();
        fs::create_dir_all(bin.join(".tmp-new")).unwrap();
        set_mtime(&bin.join(".tmp-old"), 2 * day);
        let old_q = format!(".broken-0.1.0-{}-77", n - 2 * day);
        let new_q = format!(".broken-0.1.0-{}-78-2", n - 3600);
        fs::create_dir_all(bin.join(&old_q)).unwrap();
        fs::create_dir_all(bin.join(&new_q)).unwrap();
        // a quarantine whose mtime is old but whose name is new is kept: the name counts
        set_mtime(&bin.join(&new_q), 5 * day);
        fs::create_dir_all(bin.join(".broken-junk")).unwrap();
        set_mtime(&bin.join(".broken-junk"), 5 * day);
        // links to an old folder are never followed or removed, whatever their name says
        let elsewhere = f.tmp().join("elsewhere");
        write(&elsewhere.join(".sha256"), "x\n", None);
        write(&elsewhere.join(".last-used"), "", None);
        set_mtime(&elsewhere.join(".sha256"), 31 * day);
        set_mtime(&elsewhere.join(".last-used"), 31 * day);
        set_mtime(&elsewhere, 31 * day);
        let old_link_q = format!(".broken-0.1.0-{}-1", n - 2 * day);
        for link in ["0.0.7", ".tmp-link", old_link_q.as_str()] {
            symlink(&elsewhere, bin.join(link)).unwrap();
        }
        let removed = session_start::prune_bin(&env, f.plugin(), Some(&f.c.guard), now());
        assert_eq!(
            removed,
            vec![old_q.clone(), ".tmp-old".to_string(), "0.0.9".to_string()]
        );
        for kept in [
            "0.1.0",
            "0.2.0",
            "0.0.8",
            ".tmp-new",
            ".broken-junk",
            "0.0.7",
            ".tmp-link",
            old_link_q.as_str(),
        ] {
            assert!(fs::symlink_metadata(bin.join(kept)).is_ok(), "{kept}");
        }
        assert!(bin.join(&new_q).exists());
        assert!(elsewhere.join(".sha256").is_file() && elsewhere.join(".last-used").is_file());
    }

    #[test]
    fn prune_keeps_every_version_when_a_live_lock_does_not_read() {
        let f = Fx::new();
        let bin = f.c.mem.anth.join("bin");
        write(&bin.join("0.0.9/.last-used"), "", None);
        set_mtime(&bin.join("0.0.9/.last-used"), 90 * 86_400);
        // no anthology.lock in the plugin (every release before CO2)
        assert!(session_start::prune_bin(&f.env(), f.plugin(), None, now()).is_empty());
        fs::write(f.plugin().join("anthology.lock"), lock("0.2.0")).unwrap();
        fs::write(f.c.guard.join("anthology.lock"), "{}").unwrap();
        assert!(session_start::prune_bin(&f.env(), f.plugin(), Some(&f.c.guard), now()).is_empty());
        assert_eq!(
            session_start::prune_bin(&f.env(), f.plugin(), None, now()),
            vec!["0.0.9".to_string()]
        );
    }

    #[test]
    fn session_start_prunes_at_the_end() {
        let f = Fx::new();
        f.mark_setup(None);
        let bin = f.c.mem.anth.join("bin");
        let q = format!(".broken-0.1.0-{}-9", now() as u64 - 3 * 86_400);
        fs::create_dir_all(bin.join(&q)).unwrap();
        f.go();
        assert!(!bin.join(&q).exists());
    }
}

// --- embedded hosts, the startup-only lines, and a stale loaded plugin ---------------------

mod host_mode {
    use super::*;

    const GUIDE_HEADING: &str = "# Anthology routing guide";

    /// An SDK host: the entrypoint every SDK build sets, and a session nobody watches.
    fn sdk(f: &Fx) -> Env {
        f.env()
            .with("CLAUDE_CODE_ENTRYPOINT", "sdk-ts")
            .with("CLAUDE_CODE_SESSION_ATTENDED", "0")
    }

    /// One personal note, so a run that gets memory shows a `## Personal memory` section.
    fn personal_note(f: &Fx) {
        let name = "project-host-note";
        let note = Note::new(name).type_("project").desc("A host note").text();
        write(
            &f.c.mem
                .anth
                .join("personal-memory")
                .join(format!("{name}.md")),
            &note,
            None,
        );
    }

    /// A guard that lets the self-test sample through, so the self-test has something to say.
    fn weak_guard(f: &Fx) {
        write_mode(&f.c.guard.join("hooks/guard.sh"), EXIT_0, 0o755);
    }

    #[test]
    fn an_sdk_host_gets_no_guide_and_no_memory() {
        let f = Fx::new();
        f.mark_setup(None);
        personal_note(&f);
        let hosts = [
            ("sdk-ts", Some("0")),
            ("sdk-cli", None),
            // the entrypoint prefix marks an SDK build even when the flag says attended
            ("sdk-py", Some("1")),
            // a custom host nobody watches: the attended flag alone marks it
            ("hostapp", Some("0")),
        ];
        for (entrypoint, attended) in hosts {
            let mut env = f.env().with("CLAUDE_CODE_ENTRYPOINT", entrypoint);
            if let Some(a) = attended {
                env = env.with("CLAUDE_CODE_SESSION_ATTENDED", a);
            }
            let out = f.go_d(&f.deps(), None, &env, &[]);
            assert_eq!(context(&out), "", "{entrypoint} {attended:?}");
            assert!(!has_msg(&out), "{entrypoint}: {out:?}");
        }
    }

    #[test]
    fn host_mode_full_restores_the_guide() {
        let f = Fx::new();
        f.mark_setup(None);
        personal_note(&f);
        let env = sdk(&f).with("ANTHOLOGY_HOST_MODE", "full");
        let ctx = context(&f.go_d(&f.deps(), None, &env, &[]));
        assert!(ctx.contains(GUIDE_HEADING), "{ctx}");
        assert!(ctx.contains("## Personal memory"), "{ctx}");
        // only the exact word counts: anything else leaves the host mode on
        for other in ["", "FULL", "full ", "1", "off"] {
            let env = sdk(&f).with("ANTHOLOGY_HOST_MODE", other);
            let ctx = context(&f.go_d(&f.deps(), None, &env, &[]));
            assert_eq!(ctx, "", "{other:?}");
        }
    }

    #[test]
    fn cli_and_vscode_get_the_guide() {
        let f = Fx::new();
        f.mark_setup(None);
        for entrypoint in ["cli", "claude-vscode"] {
            let env = f
                .env()
                .with("CLAUDE_CODE_ENTRYPOINT", entrypoint)
                .with("CLAUDE_CODE_SESSION_ATTENDED", "1");
            let ctx = context(&f.go_d(&f.deps(), None, &env, &[]));
            assert!(ctx.contains(GUIDE_HEADING), "{entrypoint}: {ctx}");
        }
        // no variable at all: not a host
        assert!(context(&f.go()).contains(GUIDE_HEADING));
    }

    #[test]
    fn no_personal_memory_in_a_host() {
        let f = Fx::new();
        f.mark_setup(None);
        personal_note(&f);
        let control = context(&f.go());
        assert!(control.contains("## Personal memory"), "{control}");
        assert!(control.contains("project-host-note"), "{control}");
        for source in ["startup", "resume", "compact", "clear"] {
            let out = f.go_d(&f.deps(), None, &sdk(&f), &[("source", source)]);
            let ctx = context(&out);
            assert!(!ctx.contains("Personal memory"), "{source}: {ctx}");
            assert!(!ctx.contains("project-host-note"), "{source}: {ctx}");
        }
    }

    #[test]
    fn an_sdk_host_keeps_only_the_self_test_and_bootstrap_failures() {
        let f = Fx::new();
        // Setup never ran, there is no data folder and the guard lets the sample through: the
        // lead would get the nag, the data line and the self-test warning.
        weak_guard(&f);
        let env = sdk(&f).without("CLAUDE_PLUGIN_DATA");
        let lead = f.go_d(&f.deps(), None, &f.env().without("CLAUDE_PLUGIN_DATA"), &[]);
        let lead_lines = lines(&lead);
        assert!(lead_lines.contains(&NAG.to_string()), "{lead_lines:?}");
        assert!(lead_lines.iter().any(|l| l.contains("did NOT block")));

        let out = f.go_d(&f.deps(), None, &env, &[]);
        let host_lines = lines(&out);
        assert_eq!(host_lines.len(), 1, "{host_lines:?}");
        assert!(host_lines[0].contains("did NOT block"), "{host_lines:?}");
        assert_eq!(context(&out), "");

        // a failing bootstrap step is kept as well
        let fail = |n: &str| (n == "ensure").then(|| exc("RuntimeError", "x"));
        let mut d = f.deps();
        d.fail = &fail;
        let out = f.go_d(&d, None, &env, &[]);
        let host_lines = lines(&out);
        assert!(
            host_lines.contains(
                &"anthology: ensuring the guardrails binary failed (RuntimeError)".to_string()
            ),
            "{host_lines:?}"
        );
        assert!(!host_lines.contains(&NAG.to_string()), "{host_lines:?}");
    }

    #[test]
    fn a_host_drops_the_memory_messages_and_the_other_warnings() {
        let f = Fx::new();
        f.state_write(vec![("setup_done", Value::Str("0.1.0".into()))]);
        // no settings file: a lead would hear that the plugin moved
        let mem = |_: &Path, _: &Env| Ok(("note".to_string(), vec!["memory said hi".to_string()]));
        let mut d = f.deps();
        d.memory_run = &mem;
        let lead = f.go_d(&d, None, &f.env(), &[]);
        assert!(lines(&lead).contains(&MOVED.to_string()), "{lead:?}");
        assert!(lines(&lead).contains(&"memory said hi".to_string()));
        let out = f.go_d(&d, None, &sdk(&f), &[]);
        assert!(!has_msg(&out), "{out:?}");
        assert_eq!(context(&out), "");
    }

    #[test]
    fn the_setup_nag_shows_only_at_startup() {
        let f = Fx::new();
        let at = |source: &str| lines(&f.go_d(&f.deps(), None, &f.env(), &[("source", source)]));
        assert!(at("startup").contains(&NAG.to_string()));
        for source in ["resume", "compact", "clear"] {
            assert!(!at(source).contains(&NAG.to_string()), "{source}");
        }
    }

    #[test]
    fn a_hook_input_with_no_source_is_read_as_a_start() {
        let f = Fx::new();
        let out = session_start::run(&f.env(), "{}", &f.deps());
        let out = json::loads(&out).unwrap();
        assert!(lines(&out).contains(&NAG.to_string()), "{out:?}");
        let out = session_start::run(&f.env(), r#"{"source": "other"}"#, &f.deps());
        let out = json::loads(&out).unwrap();
        assert!(!lines(&out).contains(&NAG.to_string()), "{out:?}");
    }

    #[test]
    fn the_settings_changed_note_shows_only_at_startup() {
        let f = Fx::new();
        f.mark_setup(None);
        f.state_write(vec![(
            "settings_ops_version",
            Value::Str("1-0000000000000000".into()),
        )]);
        let at = |source: &str| lines(&f.go_d(&f.deps(), None, &f.env(), &[("source", source)]));
        assert_eq!(at("startup"), vec![OPS_CHANGED.to_string()]);
        for source in ["resume", "compact", "clear"] {
            assert!(at(source).is_empty(), "{source}");
        }
    }

    /// `installed_plugins.json` with anthology at `installed` and the sibling guardrails, and the
    /// loaded plugin's manifest at `loaded`.
    fn versions(f: &Fx, loaded: &str, installed: &str) {
        write(
            &f.plugin().join(".claude-plugin/plugin.json"),
            &format!(r#"{{"name": "anthology", "version": "{loaded}"}}"#),
            None,
        );
        write(
            &f.config().join("plugins/installed_plugins.json"),
            &format!(
                r#"{{"version": 2, "plugins": {{
                "anthology@anthology": [{{"scope": "user", "installPath": {}, "version": "{installed}"}}],
                "anthology-guardrails@anthology": [{{"scope": "user", "installPath": {}, "version": "0.2.9"}}]}}}}"#,
                jstr(&p(f.plugin())),
                jstr(&p(&f.c.guard)),
            ),
            None,
        );
    }

    fn stale_line(loaded: &str, installed: &str) -> String {
        format!(
            "anthology: this session loaded anthology {loaded}, but {installed} is installed; restart Claude Code to load the new version"
        )
    }

    #[test]
    fn a_stale_loaded_plugin_is_named() {
        let f = Fx::new();
        f.mark_setup(None);
        versions(&f, "0.4.1", "0.5.0");
        for source in ["compact", "resume"] {
            let out = f.go_d(&f.deps(), None, &f.env(), &[("source", source)]);
            assert_eq!(lines(&out), vec![stale_line("0.4.1", "0.5.0")], "{source}");
        }
        // a numeric order, not a text one
        versions(&f, "0.9.0", "0.10.0");
        let out = f.go_d(&f.deps(), None, &f.env(), &[("source", "compact")]);
        assert_eq!(lines(&out), vec![stale_line("0.9.0", "0.10.0")]);
        // a release is newer than its own prerelease
        versions(&f, "0.5.0-rc.1", "0.5.0");
        let out = f.go_d(&f.deps(), None, &f.env(), &[("source", "compact")]);
        assert_eq!(lines(&out), vec![stale_line("0.5.0-rc.1", "0.5.0")]);
    }

    #[test]
    fn a_build_tagged_installed_version_is_read_as_a_prerelease() {
        let f = Fx::new();
        f.mark_setup(None);
        // the release is newer than a build of the same number: nothing to say
        versions(&f, "0.2.9", "0.2.9-f99aa8723fd9");
        let out = f.go_d(&f.deps(), None, &f.env(), &[("source", "compact")]);
        assert!(!has_msg(&out), "{out:?}");
        // an older release than that build is stale, and the line names the installed version
        versions(&f, "0.2.8", "0.2.9-f99aa8723fd9");
        let out = f.go_d(&f.deps(), None, &f.env(), &[("source", "compact")]);
        assert_eq!(lines(&out), vec![stale_line("0.2.8", "0.2.9-f99aa8723fd9")]);
    }

    #[test]
    fn a_current_or_newer_loaded_plugin_is_quiet() {
        let f = Fx::new();
        f.mark_setup(None);
        for (loaded, installed) in [
            ("0.5.0", "0.5.0"),
            ("0.6.0", "0.5.0"),
            ("0.5.0", "0.5.0-rc.1"),
        ] {
            versions(&f, loaded, installed);
            for source in ["compact", "resume"] {
                let out = f.go_d(&f.deps(), None, &f.env(), &[("source", source)]);
                assert!(!has_msg(&out), "{loaded} {installed} {source}: {out:?}");
            }
        }
    }

    #[test]
    fn the_stale_check_is_for_compact_and_resume_only() {
        let f = Fx::new();
        f.mark_setup(None);
        versions(&f, "0.4.1", "0.5.0");
        for source in ["startup", "clear"] {
            let out = f.go_d(&f.deps(), None, &f.env(), &[("source", source)]);
            assert!(!has_msg(&out), "{source}: {out:?}");
        }
    }

    #[test]
    fn a_stale_check_that_cannot_read_a_version_stays_quiet() {
        let f = Fx::new();
        f.mark_setup(None);
        for (loaded, installed) in [("0.4.1", "unknown"), ("dev", "0.5.0"), ("0.4", "0.5.0")] {
            versions(&f, loaded, installed);
            let out = f.go_d(&f.deps(), None, &f.env(), &[("source", "compact")]);
            assert!(!has_msg(&out), "{loaded} {installed}: {out:?}");
        }
        // no manifest in the loaded folder, no installed list, an entry for another project
        versions(&f, "0.4.1", "0.5.0");
        fs::remove_file(f.plugin().join(".claude-plugin/plugin.json")).unwrap();
        let out = f.go_d(&f.deps(), None, &f.env(), &[("source", "compact")]);
        assert!(
            !lines(&out).iter().any(|l| l.contains("restart")),
            "{out:?}"
        );
        let other = f.tmp().join("other-project");
        fs::create_dir(&other).unwrap();
        versions(&f, "0.4.1", "0.5.0");
        let list = fs::read_to_string(f.config().join("plugins/installed_plugins.json")).unwrap();
        let scoped = list.replacen(
            r#"{"scope": "user", "installPath": "#,
            &format!(
                r#"{{"scope": "project", "projectPath": {}, "installPath": "#,
                jstr(&p(&other))
            ),
            1,
        );
        write(
            &f.config().join("plugins/installed_plugins.json"),
            &scoped,
            None,
        );
        let out = f.go_d(&f.deps(), None, &f.env(), &[("source", "compact")]);
        assert!(
            !lines(&out).iter().any(|l| l.contains("restart")),
            "{out:?}"
        );
    }

    #[test]
    fn a_host_gets_no_stale_line() {
        let f = Fx::new();
        f.mark_setup(None);
        versions(&f, "0.4.1", "0.5.0");
        let out = f.go_d(&f.deps(), None, &sdk(&f), &[("source", "compact")]);
        assert!(!has_msg(&out), "{out:?}");
    }
}

// --- F-4 through the real staged guard shim (a fake binary stands in for GD2's) -------------

mod real_shim {
    use super::*;
    use anthology_testkit::bs::{self, Release, Sandbox};

    const BLOCK_G1: &str = "cat >/dev/null; printf '%s\\n' 'Blocked by anthology-guardrails (G1): run it inside Docker (OrbStack); for browser work use BrowserOS Neo' >&2; exit 2";
    const ALLOW: &str = "cat >/dev/null; printf 'anthology-guard:allow:v1\\n' >&2; exit 0";

    fn sandbox() -> Sandbox {
        Sandbox::new(Release::new("0.1.0", bs::release_archive(b"unused")))
    }

    fn self_test(sb: &Sandbox) -> Option<String> {
        self_test_with(sb, &SessionDeps::default())
    }

    fn self_test_with(sb: &Sandbox, deps: &SessionDeps<'_>) -> Option<String> {
        let env = Env::default()
            .with_os("HOME", &sb.home)
            .with_os("CLAUDE_CONFIG_DIR", &sb.config)
            .with_os("TMPDIR", &sb.tmpdir)
            .with("PATH", "/usr/bin:/bin");
        session_start::guard_selftest(&env, &sb.plugin("anthology"), &sb.root, deps)
            .expect("the self-test runs")
    }

    /// What the shim answers under `ANTHOLOGY_GUARD_NO_FETCH` when its own install check found
    /// no binary: a setup block that names no download.
    fn not_attempted(_: &RunSpec<'_>) -> RunResult {
        RunResult::Exited {
            code: 2,
            stdout: Vec::new(),
            stderr: b"Blocked by anthology-guardrails (setup): the guard binary is not installed (not attempted), so shell commands, file edits and MCP tools are blocked; run /anthology:setup (or start a new session while online) to install it; if this keeps happening, disable anthology-guardrails with /plugin\n".to_vec(),
        }
    }

    #[test]
    fn a_shim_that_did_not_attempt_while_the_binary_is_installed_makes_no_failing_closed_claim() {
        // The shim looked, found no binary (a parallel session was still placing it, or this
        // hook's own ensure step had been deferred), and says "not attempted". By the time
        // session start reads that answer the binary is in <ah>/bin.
        let sb = sandbox();
        let line = sb.release.version_line();
        sb.install(&bs::fake_binary(&line, BLOCK_G1, "exit 0"), 0o755);
        let deps = SessionDeps {
            run: &not_attempted,
            ..SessionDeps::default()
        };
        let m = self_test_with(&sb, &deps).expect("a line");
        assert_eq!(
            m,
            "anthology: skipped the guard self-test (the guard binary was not in place when it ran, but it is installed now); the guard still runs on every command"
        );
        for claim in ["failing closed", "blocked", "OFF"] {
            assert!(!m.contains(claim), "{m}");
        }
    }

    #[test]
    fn the_same_answer_with_no_binary_installed_is_still_the_failing_closed_warning() {
        let sb = sandbox();
        let deps = SessionDeps {
            run: &not_attempted,
            ..SessionDeps::default()
        };
        let m = self_test_with(&sb, &deps).expect("a line");
        assert!(
            m.starts_with(
                "anthology: the guardrails are failing closed: the guard binary is not installed (not attempted), so shell commands, file edits and MCP tools are blocked"
            ),
            "{m}"
        );
    }

    #[test]
    fn a_self_test_that_times_out_says_so_and_claims_nothing_about_the_guard() {
        let sb = sandbox();
        let line = sb.release.version_line();
        sb.install(&bs::fake_binary(&line, BLOCK_G1, "exit 0"), 0o755);
        // A guard that sleeps, with a one-second limit: no real 10 s wait.
        let guard = sb.plugin("anthology-guardrails").join("hooks/guard.sh");
        fs::write(&guard, "#!/bin/sh\nsleep 30\n").unwrap();
        let deps = SessionDeps {
            limits: SessionLimits {
                guard_timeout: Duration::from_secs(1),
            },
            ..SessionDeps::default()
        };
        let started = Instant::now();
        let m = self_test_with(&sb, &deps).expect("a line");
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(
            m,
            "anthology: skipped the guard self-test (it timed out, the machine may be busy); the guard still runs on every command; if commands hang or are blocked, run /anthology:setup"
        );
        for claim in ["failing closed", "OFF", "not installed"] {
            assert!(!m.contains(claim), "{m}");
        }
    }

    const FAILING_CLOSED: &str = "anthology: the guardrails are failing closed: the guard binary is not installed (not attempted)";

    fn stale_answer(sb: &Sandbox) -> String {
        let deps = SessionDeps {
            run: &not_attempted,
            ..SessionDeps::default()
        };
        self_test_with(sb, &deps).expect("a line")
    }

    #[test]
    fn a_not_attempted_answer_with_an_empty_marker_is_failing_closed() {
        let sb = sandbox();
        let line = sb.release.version_line();
        sb.install(&bs::fake_binary(&line, BLOCK_G1, "exit 0"), 0o755);
        fs::write(sb.vdir().join(".sha256"), "").unwrap();
        let m = stale_answer(&sb);
        assert!(m.starts_with(FAILING_CLOSED), "{m}");
    }

    #[test]
    fn a_not_attempted_answer_with_a_symlinked_version_dir_is_failing_closed() {
        let sb = sandbox();
        let line = sb.release.version_line();
        sb.install(&bs::fake_binary(&line, BLOCK_G1, "exit 0"), 0o755);
        let real = sb.bindir().join("real");
        fs::rename(sb.vdir(), &real).unwrap();
        symlink(&real, sb.vdir()).unwrap();
        let m = stale_answer(&sb);
        assert!(m.starts_with(FAILING_CLOSED), "{m}");
    }

    #[test]
    fn a_not_attempted_answer_with_only_another_version_installed_is_failing_closed() {
        let sb = sandbox();
        let line = sb.release.version_line();
        sb.install(&bs::fake_binary(&line, BLOCK_G1, "exit 0"), 0o755);
        fs::rename(sb.vdir(), sb.bindir().join("9.9.9")).unwrap();
        let m = stale_answer(&sb);
        assert!(m.starts_with(FAILING_CLOSED), "{m}");
    }

    /// The self-test lines of a whole session start run `elapsed` seconds in, with a guard
    /// that must not be run.
    fn lines_with_elapsed(sb: &Sandbox, elapsed: f64) -> Vec<String> {
        let tripwire = |_: &RunSpec<'_>| -> RunResult { panic!("the guard must not run") };
        let late = move || elapsed;
        let deps = SessionDeps {
            run: &tripwire,
            elapsed: Some(&late),
            ..SessionDeps::default()
        };
        let env = Env::default()
            .with_os("HOME", &sb.home)
            .with_os("CLAUDE_CONFIG_DIR", &sb.config)
            .with_os("TMPDIR", &sb.tmpdir)
            .with_os("CLAUDE_PLUGIN_ROOT", sb.plugin("anthology"))
            .with("PATH", "/usr/bin:/bin");
        let stdin = format!(
            r#"{{"hook_event_name": "SessionStart", "source": "startup", "cwd": {}}}"#,
            jstr(&p(&sb.root))
        );
        let out = json::loads(&session_start::run(&env, &stdin, &deps)).expect("JSON");
        lines(&out)
            .into_iter()
            .filter(|l| {
                l.starts_with("anthology: skipped the guard") || l.contains("failing closed")
            })
            .collect()
    }

    #[test]
    fn with_no_time_and_no_binary_session_start_says_failing_closed() {
        let sb = sandbox();
        assert_eq!(
            lines_with_elapsed(&sb, 45.0),
            vec![format!(
                "anthology: the guardrails are failing closed: {}",
                session_start::BINARY_MISSING_REASON
            )]
        );
    }

    #[test]
    fn with_no_time_and_the_binary_installed_the_self_test_is_skipped() {
        let sb = sandbox();
        let line = sb.release.version_line();
        sb.install(&bs::fake_binary(&line, BLOCK_G1, "exit 0"), 0o755);
        assert_eq!(
            lines_with_elapsed(&sb, 45.0),
            vec![session_start::SELF_TEST_NO_TIME.to_string()]
        );
    }

    #[test]
    fn the_real_shim_running_a_blocking_binary_passes() {
        let sb = sandbox();
        let line = sb.release.version_line();
        sb.install(&bs::fake_binary(&line, BLOCK_G1, "exit 0"), 0o755);
        assert_eq!(self_test(&sb), None);
    }

    #[test]
    fn the_real_shim_without_its_binary_fails_closed_and_says_why() {
        let sb = sandbox();
        let m = self_test(&sb).expect("a warning");
        assert!(
            m.starts_with(
                "anthology: the guardrails are failing closed: the guard binary is not installed (not attempted), so shell commands, file edits and MCP tools are blocked"
            ),
            "{m}"
        );
        assert!(m.contains("/plugin"), "{m}");
        // NO_FETCH: nothing was downloaded or attempted
        assert!(!sb.bindir().join(".attempt").exists());
    }

    #[test]
    fn the_real_shim_running_an_allowing_binary_did_not_block() {
        let sb = sandbox();
        let line = sb.release.version_line();
        sb.install(&bs::fake_binary(&line, ALLOW, "exit 0"), 0o755);
        assert_eq!(
            self_test(&sb).as_deref(),
            Some(
                "anthology: the guardrails self-test did NOT block a known-bad command (exit 0); the guardrails may be OFF"
            )
        );
    }
}
