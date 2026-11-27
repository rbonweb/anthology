//! Session start warns, once per start, when the official `superpowers` plugin is enabled next to
//! Anthology's own `anthology-superpowers`: both sets of instructions would load.
//!
//! The line names both plugins and the command that disables the official one. It cannot name
//! `anthology-superpowers`: `anthology` lists it as a dependency, and Claude Code refuses to
//! disable a plugin that an enabled one requires (`claude plugin disable` fails with
//! "still required by anthology"). The same detection is `doctor::find_duplicates_in`, which
//! `/anthology:setup` uses.
#![forbid(unsafe_code)]

use std::path::Path;

use anthology_core::Env;
use anthology_pycompat::json::{self, Value};
use anthology_pycompat::pyerr::PyExc;
use anthology_runtime::bootstrap::GuardBinaryStatus;
use anthology_runtime::session_start::{self, SessionDeps};
use anthology_testkit::mem::write;
use anthology_testkit::session::SessionCase;

const OFFICIAL: &str = "superpowers@claude-plugins-official";

fn no_ensure(_: &Path, _: u64, _: &Env) -> GuardBinaryStatus {
    panic!("the guard binary must not be ensured without ANTHOLOGY_BOOTSTRAP_DOWNLOADED=0");
}

fn line(key: &str, scope: &str) -> String {
    format!(
        "anthology: superpowers loads twice: {key} and anthology-superpowers@anthology are both enabled; disable the official one with: claude plugin disable {key} -s {scope}"
    )
}

struct Case {
    c: SessionCase,
}

impl Case {
    fn new() -> Case {
        Case {
            c: SessionCase::new(),
        }
    }

    fn installed(&self, entries: &str) {
        write(
            &self.c.mem.config.join("plugins/installed_plugins.json"),
            &format!(r#"{{"version": 2, "plugins": {{{entries}}}}}"#),
            None,
        );
    }

    /// Both plugins installed at user scope, and `enabled` as the user settings' `enabledPlugins`.
    fn both(&self, enabled: &str) {
        self.installed(&format!(
            r#""{OFFICIAL}": [{{"scope": "user", "version": "5.1.0"}}],
               "anthology-superpowers@anthology": [{{"scope": "user", "version": "0.2.3"}}]"#
        ));
        write(
            &self.c.mem.config.join("settings.json"),
            &format!(r#"{{"enabledPlugins": {{{enabled}}}}}"#),
            None,
        );
    }

    fn start_d(&self, deps: &SessionDeps<'_>, env: &Env, source: &str) -> Value {
        let input = format!(
            r#"{{"hook_event_name": "SessionStart", "source": "{source}", "cwd": {}}}"#,
            json::dumps(
                &Value::Str(self.c.mem.tmp.to_string_lossy().into_owned()),
                &json::PY_DEFAULT
            )
        );
        let text = session_start::run(env, &input, deps);
        json::loads(&text).unwrap_or_else(|e| panic!("not JSON: {text:?}: {e:?}"))
    }

    fn start(&self, source: &str) -> Vec<String> {
        let deps = SessionDeps {
            ensure_guard_binary: &no_ensure,
            ..SessionDeps::default()
        };
        lines(&self.start_d(&deps, &self.c.env, source))
    }
}

fn lines(out: &Value) -> Vec<String> {
    out.as_object()
        .and_then(|o| o.get("systemMessage"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn an_enabled_official_superpowers_is_named_at_startup() {
    let t = Case::new();
    t.both(&format!(
        r#""{OFFICIAL}": true, "anthology-superpowers@anthology": true"#
    ));
    let got = t.start("startup");
    assert_eq!(
        got.iter().filter(|l| l.contains("loads twice")).count(),
        1,
        "{got:?}"
    );
    assert!(got.contains(&line(OFFICIAL, "user")), "{got:?}");
}

#[test]
fn it_is_said_at_startup_only() {
    let t = Case::new();
    t.both(&format!(r#""{OFFICIAL}": true"#));
    for source in ["resume", "compact", "clear"] {
        let got = t.start(source);
        assert!(
            !got.iter().any(|l| l.contains("loads twice")),
            "{source}: {got:?}"
        );
    }
}

#[test]
fn a_disabled_official_superpowers_is_not_named() {
    let t = Case::new();
    t.both(&format!(r#""{OFFICIAL}": false"#));
    let got = t.start("startup");
    assert!(!got.iter().any(|l| l.contains("loads twice")), "{got:?}");
}

#[test]
fn an_official_superpowers_that_is_installed_but_not_enabled_is_not_named() {
    let t = Case::new();
    t.both("");
    let got = t.start("startup");
    assert!(!got.iter().any(|l| l.contains("loads twice")), "{got:?}");
}

#[test]
fn a_project_setting_that_disables_it_wins_over_the_user_one() {
    let t = Case::new();
    t.both(&format!(r#""{OFFICIAL}": true"#));
    write(
        &t.c.mem.tmp.join(".claude/settings.local.json"),
        &format!(r#"{{"enabledPlugins": {{"{OFFICIAL}": false}}}}"#),
        None,
    );
    let got = t.start("startup");
    assert!(!got.iter().any(|l| l.contains("loads twice")), "{got:?}");
}

#[test]
fn only_anthologys_own_copy_is_no_duplicate() {
    let t = Case::new();
    t.installed(r#""anthology-superpowers@anthology": [{"scope": "user", "version": "0.2.3"}]"#);
    write(
        &t.c.mem.config.join("settings.json"),
        r#"{"enabledPlugins": {"anthology-superpowers@anthology": true}}"#,
        None,
    );
    let got = t.start("startup");
    assert!(!got.iter().any(|l| l.contains("loads twice")), "{got:?}");
}

#[test]
fn the_command_carries_the_installs_scope() {
    let t = Case::new();
    t.installed(&format!(
        r#""{OFFICIAL}": [{{"scope": "project", "projectPath": "/p", "version": "5.1.0"}}]"#
    ));
    write(
        &t.c.mem.tmp.join(".claude/settings.json"),
        &format!(r#"{{"enabledPlugins": {{"{OFFICIAL}": true}}}}"#),
        None,
    );
    let got = t.start("startup");
    assert!(got.contains(&line(OFFICIAL, "project")), "{got:?}");
}

#[test]
fn an_embedded_host_hears_nothing_of_it() {
    let t = Case::new();
    t.both(&format!(r#""{OFFICIAL}": true"#));
    let env = t.c.env.clone().with("CLAUDE_CODE_SESSION_ATTENDED", "0");
    let deps = SessionDeps {
        ensure_guard_binary: &no_ensure,
        ..SessionDeps::default()
    };
    let got = lines(&t.start_d(&deps, &env, "startup"));
    assert!(!got.iter().any(|l| l.contains("loads twice")), "{got:?}");
}

#[test]
fn a_failing_check_costs_one_line_and_never_the_guide() {
    let t = Case::new();
    t.both(&format!(r#""{OFFICIAL}": true"#));
    let fail = |n: &str| {
        (n == "_duplicate_superpowers_warning").then(|| PyExc::Other("RuntimeError", "boom".into()))
    };
    let deps = SessionDeps {
        ensure_guard_binary: &no_ensure,
        fail: &fail,
        ..SessionDeps::default()
    };
    let out = t.start_d(&deps, &t.c.env, "startup");
    let got = lines(&out);
    assert!(
        got.contains(
            &"anthology: the duplicate superpowers check failed (RuntimeError)".to_string()
        ),
        "{got:?}"
    );
    let ctx = out
        .as_object()
        .and_then(|o| o.get("hookSpecificOutput"))
        .and_then(Value::as_object)
        .and_then(|h| h.get("additionalContext"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(ctx.contains("# Anthology routing guide"));
}
