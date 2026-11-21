//! Port of `tests/test_setup.py` (RT4), plus Rust tests for what the port adds (F-15: the
//! `guard_binary` step and the `<ah>/bin` remove of D§8.8).
//!
//! Modules are the Python classes; every ported test keeps the Python method name. Python's
//! `mock.patch` sites go through `SetupDeps` (GC-19): `rtk_install` is a fake `RtkWiring`,
//! `platform.system` is `uname`, `state.write` is `state_write`, `settings_patch.write_text` is
//! `settings_write`, `memory_sync.{repo_root,repo_key,approve}` is `memory`. The layout is
//! `anthology_testkit::rt_c::SetupCase` (its docs say why the plugin is a small copy).
//!
//! Python's `apply` raised where the Rust `apply` returns `main`'s
//! `{"ok": false, "error": "<Class>: <msg>"}` result.
//!
//! Ledger exceptions (port/ledger/test_setup.tsv): the cases that need the real doctor (RT1b)
//! or the real memory sync and approval (MS2) are `todo:RT4` until those merge; the cheap ones
//! are written here, ignored as `SWAP(MS2)`.
#![forbid(unsafe_code)]

use std::cell::RefCell;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use anthology_core::lock::Lock;
use anthology_core::{Env, state};
use anthology_pycompat::json::{self, Map, Value};
use anthology_pycompat::pyerr::PyExc;
use anthology_runtime::bootstrap::GuardBinaryStatus;
use anthology_runtime::rtk_install::Reconciled;
use anthology_runtime::session_start::RtkWiring;
use anthology_runtime::setup::{self, GUARD_NOT_FOUND, MemoryDeps, RTK_TIMEOUT, SetupDeps};
use anthology_settings as settings;
use anthology_testkit::mem::{self, MemCase, Note, write};
use anthology_testkit::rows::{Row, rows};
use anthology_testkit::rt_c::{SETTINGS, SetupCase, pretty};
use anthology_testkit::sandbox::repo_root;

const BYPASS: &str = "Note: `Bash(rtk *)` also auto-approves `rtk bypass <cmd>`, which runs any \
                      command. rtk's hook still applies the deny and ask rules in your settings \
                      files (user, project, local and managed) to the command it wraps, but not \
                      rules given with `--settings` or `--disallowedTools`, server-managed \
                      settings or an MDM profile.";

// --- the fixture --------------------------------------------------------------------------

/// `FakeRtk`: rtk_install's interface; `load_lock` returns the parsed lock (no validation),
/// `reconcile` records the call and returns `result`.
struct FakeRtk {
    calls: RefCell<Vec<(PathBuf, Lock, f64)>>,
    result: RefCell<Reconciled>,
    exc: RefCell<Option<PyExc>>,
    on_reconcile: RefCell<Option<Box<dyn Fn()>>>,
}

fn parsed_lock(path: &Path) -> Result<Lock, PyExc> {
    let text = fs::read_to_string(path)?;
    let v = json::loads(&text).map_err(|_| PyExc::ValueError("not JSON".into()))?;
    let field = |k: &str| {
        v.as_object()
            .and_then(|m| m.get(k))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    Ok(Lock {
        version: field("version"),
        tag: field("tag"),
        repo: field("repo"),
        sha256: Default::default(),
    })
}

impl FakeRtk {
    fn new() -> FakeRtk {
        FakeRtk {
            calls: RefCell::new(Vec::new()),
            result: RefCell::new(Reconciled {
                rtk_path: None,
                warning: None,
                installed: true,
            }),
            exc: RefCell::new(None),
            on_reconcile: RefCell::new(None),
        }
    }
}

impl RtkWiring for FakeRtk {
    fn load_lock(&self, path: &Path) -> Result<Lock, PyExc> {
        parsed_lock(path)
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
        if let Some(f) = &*self.on_reconcile.borrow() {
            f();
        }
        match self.exc.borrow().clone() {
            Some(e) => Err(e),
            None => Ok(self.result.borrow().clone()),
        }
    }
}

fn present(_: &Path, _: u64, _: &Env) -> GuardBinaryStatus {
    GuardBinaryStatus::Present
}

fn windows() -> (String, String) {
    ("Windows".into(), "AMD64".into())
}

fn linux() -> (String, String) {
    ("Linux".into(), "x86_64".into())
}

fn sha(text: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn p(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// `out[k1][k2]…`; panics when a key is missing.
fn at<'a>(v: &'a Value, path: &[&str]) -> &'a Value {
    path.iter().fold(v, |v, k| {
        v.as_object()
            .and_then(|m| m.get(k))
            .unwrap_or_else(|| panic!("no {k:?} in {v:?}"))
    })
}

fn text_at<'a>(v: &'a Value, path: &[&str]) -> &'a str {
    at(v, path)
        .as_str()
        .unwrap_or_else(|| panic!("{path:?} is not a str"))
}

fn ok(v: &Value) -> bool {
    at(v, &["ok"]).as_bool().expect("ok is a bool")
}

fn dumped(v: &Value) -> String {
    json::dumps(v, &json::PY_DEFAULT)
}

fn allow(settings_file: &Path) -> Vec<String> {
    let v = json::loads(&fs::read_to_string(settings_file).unwrap()).unwrap();
    at(&v, &["permissions", "allow"])
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap().to_string())
        .collect()
}

struct Fx {
    c: SetupCase,
    rtk: FakeRtk,
}

impl Fx {
    fn new() -> Fx {
        Fx {
            c: SetupCase::new(),
            rtk: FakeRtk::new(),
        }
    }

    fn tmp(&self) -> &Path {
        &self.c.mem.tmp
    }

    fn env(&self) -> &Env {
        &self.c.env
    }

    fn anth(&self) -> &Path {
        &self.c.mem.anth
    }

    fn deps(&self) -> SetupDeps<'_> {
        SetupDeps {
            rtk: Some(&self.rtk),
            ensure_guard_binary: &present,
            ..SetupDeps::default()
        }
    }

    fn state(&self) -> Map {
        state::read(self.env())
    }

    fn state_exists(&self) -> bool {
        state::path(self.env()).exists()
    }

    fn read_settings(&self) -> String {
        fs::read_to_string(&self.c.settings).unwrap()
    }

    fn rtk_calls(&self) -> usize {
        self.rtk.calls.borrow().len()
    }

    /// `self.preview(cwd)`.
    fn preview(&self, cwd: Option<&Path>) -> Value {
        self.preview_d(cwd, &self.deps())
    }

    fn preview_d(&self, cwd: Option<&Path>, deps: &SetupDeps<'_>) -> Value {
        setup::preview(
            &self.c.plugin,
            Some(cwd.unwrap_or(self.tmp())),
            self.env(),
            deps,
        )
    }

    fn digest(&self) -> String {
        text_at(&self.preview(None), &["settings", "sha256"]).to_string()
    }

    /// `self.apply()`: with the preview's digest.
    fn apply(&self) -> Value {
        self.apply_d(&self.deps())
    }

    fn apply_d(&self, deps: &SetupDeps<'_>) -> Value {
        let digest = text_at(&self.preview_d(None, deps), &["settings", "sha256"]).to_string();
        self.apply_with(&self.c.plugin, &digest, deps)
    }

    fn apply_with(&self, root: &Path, digest: &str, deps: &SetupDeps<'_>) -> Value {
        setup::apply(root, digest, Some(self.tmp()), false, self.env(), &[], deps)
    }

    /// `self.remove(**kw)`.
    fn remove(&self, unapprove: bool, unapprove_all: bool, preview: bool) -> Value {
        self.remove_env(self.env(), unapprove, unapprove_all, preview)
    }

    fn remove_env(&self, env: &Env, unapprove: bool, unapprove_all: bool, preview: bool) -> Value {
        setup::remove(
            &self.c.plugin,
            Some(self.tmp()),
            unapprove,
            unapprove_all,
            env,
            preview,
            &self.deps(),
        )
    }

    fn env_without_data(&self) -> Env {
        self.env().clone().without("CLAUDE_PLUGIN_DATA")
    }
}

mod preview_test {
    use super::*;

    #[test]
    fn test_preview_writes_nothing() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        fs::create_dir_all(&f.c.data_dir).unwrap();
        let before = mem::snapshot(f.tmp());
        let out = f.preview(None);
        assert_eq!(mem::snapshot(f.tmp()), before);
        assert_eq!(f.rtk_calls(), 0);
        assert!(json::loads(&dumped(&out)).is_ok()); // JSON-serialisable
    }

    #[test]
    fn test_preview_reports_every_step() {
        let f = Fx::new();
        let text = f.c.write_settings(SETTINGS);
        let out = f.preview(None);
        assert_eq!(at(&out, &["supported"]), &Value::Bool(true));
        let names: Vec<&str> = at(&out, &["doctor", "checks"])
            .as_array()
            .unwrap()
            .iter()
            .map(|c| text_at(c, &["name"]))
            .collect();
        // F-5: `anthology_binary` replaces `python3`.
        let mut want = vec![
            "anthology_binary",
            "node",
            "docker",
            "browseros",
            "rtk",
            "platform_supported",
            "plugin_read_rule",
            "browseros_tools",
            "plugin_data",
            "path_wrappers",
            "agent_memory_path",
            "subagent_model",
        ];
        let mut got = names.clone();
        got.sort_unstable();
        want.sort_unstable();
        assert_eq!(got, want);
        assert_eq!(at(&out, &["duplicates"]), &Value::Array(Vec::new()));
        let diff = text_at(&out, &["settings", "diff"]);
        assert_eq!(text_at(&out, &["settings", "sha256"]), sha(&text));
        assert_eq!(at(&out, &["settings", "changed"]), &Value::Bool(true));
        assert!(diff.contains("+++ settings.json"));
        assert!(diff.contains("Bash(rtk *)"));
        assert!(diff.contains("Bash(anthology-mem save"));
        assert!(diff.contains("\"head\""));
        assert_eq!(text_at(&out, &["settings", "bypass_note"]), BYPASS);
        assert_eq!(
            text_at(&out, &["memory", "personal_dir"]),
            p(&f.anth().join("personal-memory"))
        );
        assert_eq!(
            at(&out, &["memory", "personal_exists"]),
            &Value::Bool(false)
        );
    }

    #[test]
    fn test_preview_runs_the_compat_checks_in_its_cwd() {
        let f = Fx::new();
        let proj = f.tmp().join("proj");
        write(
            &proj.join(".claude/settings.local.json"),
            r#"{"env": {"CLAUDE_CODE_SUBAGENT_MODEL": "m"}}"#,
            None,
        );
        let check = |cwd: &Path| {
            let out = f.preview(Some(cwd));
            at(&out, &["doctor", "checks"])
                .as_array()
                .unwrap()
                .iter()
                .find(|c| text_at(c, &["name"]) == "subagent_model")
                .cloned()
                .unwrap()
        };
        let c = check(&proj);
        assert_eq!(at(&c, &["ok"]), &Value::Bool(false));
        assert!(!dumped(&c).contains("\"m\""));
        assert_eq!(at(&check(f.tmp()), &["ok"]), &Value::Bool(true));
    }

    #[test]
    fn test_missing_settings_file_previews_a_new_file() {
        let f = Fx::new();
        let out = f.preview(None);
        assert_eq!(text_at(&out, &["settings", "sha256"]), sha(""));
        assert!(text_at(&out, &["settings", "diff"]).contains("worktree"));
    }

    #[test]
    fn test_rtk_step_comes_from_the_lock() {
        let f = Fx::new();
        let root = f.c.plugin_with_guard(&f.tmp().join("p").join("anthology"));
        write(
            &root.join("rtk.lock"),
            r#"{"version": "1.2.1-anthology.1", "tag": "t", "targets": {}}"#,
            None,
        );
        // Python's fake `target` returned "test-target"; the Rust target is
        // rtk_install::target for the `uname` seam.
        let deps = SetupDeps {
            uname: linux,
            ..f.deps()
        };
        let out = setup::preview(&root, Some(f.tmp()), f.env(), &deps);
        assert_eq!(text_at(&out, &["rtk", "version"]), "1.2.1-anthology.1");
        assert_eq!(
            text_at(&out, &["rtk", "target"]),
            "x86_64-unknown-linux-musl"
        );
        assert_eq!(
            text_at(&out, &["rtk", "dest"]),
            p(&f.c.data_dir.join("rtk/1.2.1-anthology.1/rtk"))
        );
    }

    #[test]
    fn test_rtk_step_skipped_when_wiring_is_absent() {
        let f = Fx::new();
        let deps = SetupDeps {
            rtk: None,
            ..f.deps()
        };
        let out = f.preview_d(None, &deps);
        assert!(text_at(&out, &["rtk", "skipped"]).contains("rtk wiring not present"));
    }

    fn installed(f: &Fx, text: &str) {
        write(
            &f.c.mem.config.join("plugins/installed_plugins.json"),
            text,
            None,
        );
    }

    #[test]
    fn test_duplicates_are_listed_with_uninstall_commands() {
        let f = Fx::new();
        installed(
            &f,
            r#"{"version": 2, "plugins": {"superpowers@claude-plugins-official": [{"scope": "user", "installPath": "/p"}]}}"#,
        );
        let out = f.preview(None);
        let first = &at(&out, &["duplicates"]).as_array().unwrap()[0];
        assert_eq!(
            text_at(first, &["command"]),
            "claude plugin uninstall superpowers@claude-plugins-official -s user"
        );
    }

    #[test]
    fn test_an_odd_scope_is_listed_without_a_command() {
        let f = Fx::new();
        installed(
            &f,
            r#"{"version": 2, "plugins": {"superpowers@m": [{"scope": "user; touch x", "installPath": "/p"}]}}"#,
        );
        let out = f.preview(None);
        let first = &at(&out, &["duplicates"]).as_array().unwrap()[0];
        assert_eq!(at(first, &["command"]), &Value::Null);
        assert_eq!(text_at(&out, &["installed_plugins"]), "ok");
    }

    #[test]
    fn test_an_unreadable_installed_plugins_file_is_reported_not_a_crash() {
        let f = Fx::new();
        installed(&f, &("[".repeat(200_000) + &"]".repeat(200_000)));
        let out = f.preview(None);
        assert_eq!(at(&out, &["duplicates"]), &Value::Array(Vec::new()));
        assert_eq!(text_at(&out, &["installed_plugins"]), "unreadable");
    }

    #[test]
    fn test_the_skill_tells_the_agent_what_to_do_without_a_command() {
        let text = fs::read_to_string(repo_root().join("plugins/anthology/skills/setup/SKILL.md"))
            .unwrap();
        assert!(text.contains("`command` is `null`"));
        assert!(text.contains("`installed_plugins` is `unreadable`"));
        assert!(text.contains("not approved"));
    }

    #[test]
    fn test_a_missing_installed_plugins_file_is_reported() {
        let f = Fx::new();
        assert_eq!(text_at(&f.preview(None), &["installed_plugins"]), "missing");
    }

    #[test]
    fn test_not_a_git_repo_is_reported() {
        let f = Fx::new();
        let out = f.preview(Some(f.tmp()));
        let mut want = Map::new();
        want.insert("skipped".into(), Value::Str("not a git repo".into()));
        assert_eq!(at(&out, &["memory", "repo"]), &Value::Object(want));
    }

    #[test]
    #[ignore = "SWAP(MS2): repo_root and repo_key are the real memory functions"]
    fn test_repo_step_names_the_key() {
        if !mem::needs_git() {
            return;
        }
        let f = Fx::new();
        let repo =
            f.c.mem
                .make_repo("repo", Some("https://example.invalid/acme/app.git"));
        let out = f.preview(Some(&repo));
        assert_eq!(
            text_at(&out, &["memory", "repo", "key"]),
            "example.invalid/acme/app"
        );
        assert_eq!(
            at(&out, &["memory", "repo", "approved"]),
            &Value::Bool(false)
        );
    }

    #[test]
    fn test_unparseable_settings_are_reported_not_raised() {
        let f = Fx::new();
        write(&f.c.settings, "{nope", None);
        let out = f.preview(None);
        assert!(text_at(&out, &["settings", "error"]).contains("not valid JSON"));
    }

    #[test]
    fn test_windows_is_refused() {
        let f = Fx::new();
        let deps = SetupDeps {
            uname: windows,
            ..f.deps()
        };
        let out = f.preview_d(None, &deps);
        assert_eq!(at(&out, &["supported"]), &Value::Bool(false));
        assert!(text_at(&out, &["message"]).contains("macOS and Linux"));
        assert!(out.as_object().unwrap().get("settings").is_none());
    }
}

mod real_rtk_test {
    use super::*;
    use anthology_runtime::fetch::Fetch;
    use anthology_runtime::rtk_install::{self, RtkDeps};
    use anthology_testkit::release::{self, RTK_BODY, TARGET, VERSION};
    use anthology_testkit::rt_b::{FakeAnswer, RtkFakeFetch, rtk_url_for};
    use std::io::Write;

    struct Fake(RtkFakeFetch);

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

    /// The real `rtk_install` behind setup's adapters, with a fake download.
    struct Real<'a>(&'a Fake);

    impl RtkWiring for Real<'_> {
        fn load_lock(&self, path: &Path) -> Result<Lock, PyExc> {
            rtk_install::load_lock(path)
        }

        fn reconcile(
            &self,
            env: &Env,
            data_dir: &Path,
            lock: &Lock,
            timeout: f64,
        ) -> Result<Reconciled, PyExc> {
            let deps = RtkDeps {
                uname: linux,
                ..RtkDeps::new(self.0)
            };
            Ok(rtk_install::reconcile(env, data_dir, lock, timeout, &deps))
        }
    }

    struct Rx {
        m: MemCase,
        data_dir: PathBuf,
        blob: Vec<u8>,
        root: PathBuf,
        lock: Lock,
    }

    impl Rx {
        fn new() -> Rx {
            let m = MemCase::new();
            let data_dir = m.tmp.join("plugin-data");
            let blob = release::tarball(RTK_BODY);
            let root = m.tmp.join("plugin");
            let lock = release::write_lock(&root.join("rtk.lock"), &blob, VERSION);
            Rx {
                m,
                data_dir,
                blob,
                root,
                lock,
            }
        }

        fn data(&self) -> String {
            p(&self.data_dir)
        }
    }

    fn deps<'a>(real: &'a Real<'a>) -> SetupDeps<'a> {
        SetupDeps {
            uname: linux,
            rtk: Some(real),
            ensure_guard_binary: &present,
            ..SetupDeps::default()
        }
    }

    #[test]
    fn test_preview_names_the_version_target_and_destination() {
        let r = Rx::new();
        let fetch = Fake(RtkFakeFetch::default());
        let real = Real(&fetch);
        let out = setup::rtk_preview(&r.root, Some(&r.data()), &deps(&real));
        let mut want = Map::new();
        want.insert("version".into(), Value::Str(VERSION.into()));
        want.insert("target".into(), Value::Str(TARGET.into()));
        want.insert(
            "dest".into(),
            Value::Str(p(&r.data_dir.join("rtk").join(VERSION).join("rtk"))),
        );
        assert_eq!(out, Value::Object(want));
    }

    #[test]
    fn test_apply_installs_then_reports_up_to_date() {
        let r = Rx::new();
        let fetch = Fake(RtkFakeFetch::serving(
            &rtk_url_for(&r.lock, TARGET),
            &r.blob,
        ));
        let real = Real(&fetch);
        let d = deps(&real);
        let data = r.data();
        assert_eq!(
            setup::rtk_apply(&r.root, Some(&data), &r.m.env, &d),
            ("installed".to_string(), None)
        );
        assert_eq!(
            setup::rtk_apply(&r.root, Some(&data), &r.m.env, &d),
            ("up to date".to_string(), None)
        );
        let want = r.data_dir.join("rtk").join(VERSION).join("rtk");
        assert_eq!(
            state::read(&r.m.env)
                .get("rtk_path")
                .and_then(Value::as_str),
            Some(p(&want).as_str())
        );
        assert!(fs::metadata(&want).unwrap().permissions().mode() & 0o111 != 0);
        let calls = fetch.0.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, RTK_TIMEOUT);
    }

    #[test]
    fn test_a_bad_download_is_a_warning() {
        let r = Rx::new();
        let fetch = Fake(RtkFakeFetch::serving(
            &rtk_url_for(&r.lock, TARGET),
            b"wrong",
        ));
        let real = Real(&fetch);
        let (status, warning) = setup::rtk_apply(&r.root, Some(&r.data()), &r.m.env, &deps(&real));
        assert_eq!(status, "warning");
        assert!(
            warning
                .unwrap()
                .contains("rtk is not installed or up to date")
        );
        assert!(state::read(&r.m.env).get("rtk_path").is_none());
    }

    #[test]
    fn test_an_invalid_lock_is_a_warning() {
        let r = Rx::new();
        fs::write(r.root.join("rtk.lock"), r#"{"version": "1"}"#).unwrap();
        let fetch = Fake(RtkFakeFetch::default());
        let real = Real(&fetch);
        let d = deps(&real);
        let (status, warning) = setup::rtk_apply(&r.root, Some(&r.data()), &r.m.env, &d);
        assert_eq!(status, "failed");
        assert!(warning.unwrap().contains("rtk.lock"));
        let mut want = Map::new();
        want.insert(
            "skipped".into(),
            Value::Str("rtk wiring not present (no rtk.lock)".into()),
        );
        assert_eq!(
            setup::rtk_preview(&r.root, Some(&r.data()), &d),
            Value::Object(want)
        );
    }
}

mod apply_remove_test {
    use super::*;

    #[test]
    fn test_round_trip_leaves_settings_byte_identical_and_state_gone() {
        let f = Fx::new();
        let text = f.c.write_settings(SETTINGS);
        let res = f.apply();
        assert!(ok(&res), "{res:?}");
        let data = json::loads(&f.read_settings()).unwrap();
        assert!(allow(&f.c.settings).contains(&"Bash(rtk *)".to_string()));
        assert_eq!(text_at(&data, &["worktree", "baseRef"]), "head");
        assert_eq!(text_at(&data, &["model"]), "opus");
        let st = f.state();
        assert_eq!(st.get("setup_done").and_then(Value::as_str), Some("0.1.0"));
        assert!(
            st.get("settings_ops")
                .and_then(Value::as_array)
                .is_some_and(|a| !a.is_empty())
        );
        let res = f.remove(false, false, false);
        assert!(ok(&res), "{res:?}");
        assert_eq!(fs::read(&f.c.settings).unwrap(), text.as_bytes());
        assert!(!f.state_exists());
    }

    #[test]
    fn test_round_trip_with_crlf_and_no_settings_file() {
        let f = Fx::new();
        let text = pretty(SETTINGS).replace('\n', "\r\n");
        write(&f.c.settings, &text, None);
        f.apply();
        f.remove(false, false, false);
        assert_eq!(fs::read(&f.c.settings).unwrap(), text.as_bytes());
        fs::remove_file(&f.c.settings).unwrap();
        f.apply();
        assert!(f.c.settings.exists());
        f.remove(false, false, false);
        assert!(!f.c.settings.exists());
        assert!(!f.state_exists());
    }

    #[test]
    fn test_apply_twice_then_remove_still_restores_everything() {
        let f = Fx::new();
        let text = f.c.write_settings(SETTINGS);
        f.apply();
        f.apply();
        f.remove(false, false, false);
        assert_eq!(f.read_settings(), text);
    }

    #[test]
    fn test_apply_refuses_when_settings_changed_since_the_preview() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        let digest = f.digest();
        let changed =
            f.c.write_settings(r#"{"model": "sonnet", "permissions": {"allow": ["Bash(ls)"]}}"#);
        let res = f.apply_with(&f.c.plugin, &digest, &f.deps());
        assert!(!ok(&res));
        assert!(text_at(&res, &["error"]).contains("changed since the preview"));
        assert_eq!(f.read_settings(), changed);
        assert!(!f.state_exists());
        assert_eq!(f.rtk_calls(), 0);
    }

    #[test]
    fn test_unparseable_settings_stop_apply_before_anything_else() {
        let f = Fx::new();
        write(&f.c.settings, "{nope", None);
        let res = f.apply();
        assert!(!ok(&res));
        assert!(text_at(&res, &["error"]).contains("not valid JSON"));
        assert_eq!(f.read_settings(), "{nope");
        assert_eq!(f.rtk_calls(), 0);
        assert!(!f.anth().join("personal-memory").exists());
    }

    #[test]
    fn test_apply_creates_the_personal_folder_with_mode_0700() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        let res = f.apply();
        let personal = f.anth().join("personal-memory");
        assert!(personal.is_dir());
        assert_eq!(
            fs::metadata(&personal).unwrap().permissions().mode() & 0o7777,
            0o700
        );
        assert_eq!(text_at(&res, &["memory", "personal_dir"]), p(&personal));
    }

    #[test]
    fn test_apply_runs_the_rtk_reconcile_and_surfaces_its_warning() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        *f.rtk.result.borrow_mut() = Reconciled {
            rtk_path: None,
            warning: Some("rtk download failed: offline".into()),
            installed: false,
        };
        let res = f.apply();
        let calls = f.rtk.calls.borrow();
        assert_eq!(calls.len(), 1);
        let (data_dir, lock, timeout) = &calls[0];
        assert_eq!(data_dir, &f.c.data_dir);
        // The parsed lock (Python's `fetch` argument is gone: the real wiring always uses the
        // production fetch).
        assert_eq!(lock, &parsed_lock(&f.c.plugin.join("rtk.lock")).unwrap());
        assert!(*timeout >= 60.0);
        assert_eq!(text_at(&res, &["rtk", "status"]), "warning");
        assert!(
            at(&res, &["warnings"])
                .as_array()
                .unwrap()
                .contains(&Value::Str("rtk download failed: offline".into()))
        );
        assert!(ok(&res));
        assert_eq!(
            f.state().get("setup_done").and_then(Value::as_str),
            Some("0.1.0")
        );
    }

    #[test]
    fn test_apply_reports_installed_and_up_to_date() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        assert_eq!(text_at(&f.apply(), &["rtk", "status"]), "installed");
        *f.rtk.result.borrow_mut() = Reconciled {
            rtk_path: Some(PathBuf::from("x")),
            warning: None,
            installed: false,
        };
        assert_eq!(text_at(&f.apply(), &["rtk", "status"]), "up to date");
    }

    #[test]
    fn test_missing_lock_is_a_warning_not_an_exception() {
        let f = Fx::new();
        let root = f.tmp().join("p").join("anthology");
        let data = p(&f.c.data_dir);
        let mut want = Map::new();
        want.insert(
            "skipped".into(),
            Value::Str("rtk wiring not present (no rtk.lock)".into()),
        );
        assert_eq!(
            setup::rtk_preview(&root, Some(&data), &f.deps()),
            Value::Object(want)
        );
        let (status, warning) = setup::rtk_apply(&root, Some(&data), f.env(), &f.deps());
        assert_eq!(status, "failed");
        assert!(warning.unwrap().contains("rtk install failed"));
        assert_eq!(f.rtk_calls(), 0);
    }

    #[test]
    fn test_rtk_exception_is_a_warning() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        *f.rtk.exc.borrow_mut() = Some(PyExc::Other("RuntimeError", "kaboom".into()));
        let res = f.apply();
        assert!(ok(&res));
        assert!(dumped(at(&res, &["warnings"])).contains("kaboom"));
    }

    #[test]
    fn test_absent_rtk_wiring_is_skipped() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        let deps = SetupDeps {
            rtk: None,
            ..f.deps()
        };
        let res = f.apply_d(&deps);
        assert!(ok(&res));
        assert!(text_at(&res, &["rtk", "status"]).contains("skipped"));
    }

    #[test]
    fn test_data_dir_falls_back_to_state_when_env_is_unset() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        state::write(
            f.env(),
            vec![("data_dir".into(), Value::Str(p(&f.c.data_dir)))],
        )
        .unwrap();
        let env = f.env_without_data();
        let deps = f.deps();
        let out = setup::preview(&f.c.plugin, Some(f.tmp()), &env, &deps);
        let digest = text_at(&out, &["settings", "sha256"]).to_string();
        setup::apply(&f.c.plugin, &digest, Some(f.tmp()), false, &env, &[], &deps);
        assert_eq!(f.rtk.calls.borrow()[0].0, f.c.data_dir);
    }

    #[test]
    fn test_symlinked_settings_are_written_through() {
        let f = Fx::new();
        let target = f.tmp().join("dotfiles").join("settings.json");
        let text = pretty(SETTINGS);
        write(&target, &text, None);
        fs::create_dir_all(&f.c.mem.config).unwrap();
        symlink(&target, &f.c.settings).unwrap();
        f.apply();
        assert!(
            fs::symlink_metadata(&f.c.settings)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(fs::read_to_string(&target).unwrap().contains("Bash(rtk *)"));
        f.remove(false, false, false);
        assert_eq!(fs::read_to_string(&target).unwrap(), text);
    }

    #[test]
    fn test_plugin_moved_replaces_the_stale_plugin_read_rule() {
        let f = Fx::new();
        let text = f.c.write_settings(SETTINGS);
        let old =
            f.c.plugin_with_guard(&f.tmp().join("old").join("anthology"));
        let deps = f.deps();
        let out = setup::preview(&old, Some(f.tmp()), f.env(), &deps);
        let digest = text_at(&out, &["settings", "sha256"]).to_string();
        assert!(ok(&f.apply_with(&old, &digest, &deps)));
        assert!(allow(&f.c.settings).contains(&settings::plugin_read_rule(&old)));
        f.apply();
        let rules = allow(&f.c.settings);
        assert!(!rules.contains(&settings::plugin_read_rule(&old)));
        assert!(rules.contains(&settings::plugin_read_rule(&f.c.plugin)));
        f.remove(false, false, false);
        assert_eq!(f.read_settings(), text);
    }

    #[test]
    fn test_guardrails_not_found_stops_with_a_message() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        let lone = f.tmp().join("lone").join("anthology");
        fs::create_dir_all(lone.join(".claude-plugin")).unwrap();
        fs::copy(
            f.c.plugin.join(".claude-plugin/plugin.json"),
            lone.join(".claude-plugin/plugin.json"),
        )
        .unwrap();
        let res = f.apply_with(&lone, &sha(&f.read_settings()), &f.deps());
        assert!(!ok(&res));
        assert!(text_at(&res, &["error"]).contains("guardrails"));
        assert_eq!(text_at(&res, &["error"]), GUARD_NOT_FOUND); // F-6
    }

    #[test]
    fn test_windows_is_refused_without_touching_anything() {
        let f = Fx::new();
        let text = f.c.write_settings(SETTINGS);
        let before = mem::snapshot(f.tmp());
        let deps = SetupDeps {
            uname: windows,
            ..f.deps()
        };
        let applied = f.apply_with(&f.c.plugin, &sha(&text), &deps);
        let removed = setup::remove(
            &f.c.plugin,
            Some(f.tmp()),
            false,
            false,
            f.env(),
            false,
            &deps,
        );
        for res in [applied, removed] {
            assert!(!ok(&res));
            assert!(text_at(&res, &["error"]).contains("macOS and Linux"));
        }
        assert_eq!(mem::snapshot(f.tmp()), before);
    }

    #[test]
    fn test_unsafe_target_with_a_stubbed_approve() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        let tmp = f.tmp().to_path_buf();
        let repo_root = move |_: &Path, _: &Env| Some(tmp.clone());
        let repo_key = |_: &Path, _: &Env| "k".to_string();
        let approve = |_: &Path, _: &Env| {
            Err(PyExc::Other(
                "UnsafeTarget",
                "refusing to use .gitattributes".into(),
            ))
        };
        let deps = SetupDeps {
            memory: MemoryDeps {
                repo_root: &repo_root,
                repo_key: &repo_key,
                approve: &approve,
            },
            ..f.deps()
        };
        let res = f.apply_d(&deps);
        assert!(ok(&res));
        assert!(text_at(&res, &["memory", "approve_error"]).contains("refusing"));
        assert_eq!(text_at(&res, &["memory", "approved_key"]), "k");
    }
}

mod fix_round1_test {
    use super::*;

    const SONNET: &str = r#"{"model": "sonnet", "permissions": {"allow": ["Bash(ls)"]}}"#;

    /// `unsafe_root()`: a plugin folder directly inside the (temp) home folder.
    fn unsafe_root(f: &Fx) -> PathBuf {
        f.c.plugin_with_guard(&f.c.mem.home.join("anthology"))
    }

    #[test]
    fn test_unsafe_plugin_root_is_refused_in_preview_and_apply_with_nothing_written() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        let root = unsafe_root(&f);
        let deps = f.deps();
        let out = setup::preview(&root, Some(f.tmp()), f.env(), &deps);
        assert!(text_at(&out, &["settings", "error"]).contains("home folder"));
        assert_eq!(at(&out, &["settings", "changed"]), &Value::Bool(false));
        let res = f.apply_with(&root, &sha(&f.read_settings()), &deps);
        assert!(!ok(&res));
        assert!(text_at(&res, &["error"]).contains("home folder"));
        assert_eq!(f.rtk_calls(), 0);
        assert!(!f.state_exists());
        assert!(!f.read_settings().contains("Read(//"));
    }

    #[test]
    fn test_empty_plugin_root_is_refused() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        let res = f.apply_with(Path::new(""), &sha(&f.read_settings()), &f.deps());
        assert!(!ok(&res));
        assert!(text_at(&res, &["error"]).contains("absolute"));
    }

    #[test]
    fn test_edit_during_the_rtk_download_is_never_overwritten() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        let digest = f.digest();
        let edited = pretty(SONNET);
        let (path, e) = (f.c.settings.clone(), edited.clone());
        // The user edits while we "download".
        *f.rtk.on_reconcile.borrow_mut() = Some(Box::new(move || fs::write(&path, &e).unwrap()));
        *f.rtk.result.borrow_mut() = Reconciled {
            rtk_path: None,
            warning: None,
            installed: false,
        };
        let res = f.apply_with(&f.c.plugin, &digest, &f.deps());
        assert!(!ok(&res));
        assert!(text_at(&res, &["error"]).contains("changed during setup"));
        assert_eq!(f.read_settings(), edited);
        assert!(
            f.state()
                .get("settings_ops")
                .and_then(Value::as_array)
                .is_none_or(Vec::is_empty)
        );
        assert!(f.state().get("setup_done").is_none());
    }

    #[test]
    fn test_edit_right_before_the_write_is_never_overwritten() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        let digest = f.digest();
        let edited = pretty(SONNET);
        let path = f.c.settings.clone();
        let e = edited.clone();
        let write_then_edit = move |env: &Env, kv: Vec<(String, Value)>| {
            let edit = kv
                .iter()
                .any(|(k, v)| k == "settings_ops" && v.as_array().is_some_and(|a| !a.is_empty()));
            let out = state::write(env, kv);
            if edit {
                fs::write(&path, &e).unwrap();
            }
            out
        };
        let deps = SetupDeps {
            state_write: &write_then_edit,
            ..f.deps()
        };
        let res = f.apply_with(&f.c.plugin, &digest, &deps);
        assert!(!ok(&res));
        assert_eq!(f.read_settings(), edited);
        assert_eq!(
            f.state().get("settings_ops"),
            Some(&Value::Array(Vec::new()))
        );
    }

    #[test]
    fn test_ops_are_recorded_before_settings_json_is_written() {
        let f = Fx::new();
        let text = f.c.write_settings(SETTINGS);
        let digest = f.digest();
        let disk_full = |_: &Path, _: &str| Err(PyExc::Other("OSError", "disk full".into()));
        let deps = SetupDeps {
            settings_write: &disk_full,
            ..f.deps()
        };
        // Python raised OSError; the Rust result is what main prints for it.
        let res = f.apply_with(&f.c.plugin, &digest, &deps);
        assert!(!ok(&res));
        assert_eq!(text_at(&res, &["error"]), "OSError: disk full");
        assert!(
            f.state()
                .get("settings_ops")
                .and_then(Value::as_array)
                .is_some_and(|a| !a.is_empty())
        );
        assert_eq!(f.read_settings(), text);
    }

    #[test]
    fn test_diff_has_no_neighbouring_secret_lines() {
        let f = Fx::new();
        f.c.write_settings(
            r#"{"model": "opus", "permissions": {"allow": ["Bash(ls)"]}, "env": {"SECRET_TOKEN": "hunter2"}}"#,
        );
        assert!(!dumped(&f.preview(None)).contains("hunter2"));
        assert!(!dumped(&f.apply()).contains("hunter2"));
    }

    #[test]
    fn test_env_and_secret_values_never_reach_any_diff_in_any_layout() {
        // The subTest rows name the layouts; each is built from one dict, as in Python.
        let data = r#"{"model": "opus", "permissions": {"allow": ["Bash(ls)"]}, "apiKeyHelper": "/bin/echo sk-live-123", "env": {"SECRET_TOKEN": "hunter2"}}"#;
        let layout = |name: &str| {
            match name {
            "minified" => r#"{"model":"opus","permissions":{"allow":["Bash(ls)"]},"apiKeyHelper":"/bin/echo sk-live-123","env":{"SECRET_TOKEN":"hunter2"}}"#
                .to_string(),
            "one-line env last" => "{\n  \"model\": \"opus\",\n  \"permissions\": {\"allow\": \
                                    [\"Bash(ls)\"]},\n  \"env\": {\"SECRET_TOKEN\": \"hunter2\", \
                                    \"OTHER\": \"sk-live-123\"}\n}\n"
                .to_string(),
            "pretty" => pretty(data),
            other => panic!("no layout {other:?}"),
        }
        };
        let table: Vec<Row> = rows(
            "test_setup",
            "test_setup.FixRound1Test.test_env_and_secret_values_never_reach_any_diff_in_any_layout",
        );
        assert_eq!(table.len(), 3);
        for row in table {
            let name = row.str("$msg");
            let text = layout(name);
            let f = Fx::new();
            write(&f.c.settings, &text, None);
            let outs = [
                f.preview(None),
                f.apply(),
                f.remove(false, false, true),
                f.remove(false, false, false),
            ];
            for out in &outs {
                let diff = text_at(out, &["settings", "diff"]);
                assert!(!diff.is_empty(), "{name}");
                assert!(diff.contains("Bash(rtk *)"), "{name}");
                let d = dumped(out);
                assert!(!d.contains("hunter2"), "{name}");
                assert!(!d.contains("sk-live-123"), "{name}");
            }
            if name != "pretty" {
                // The env line itself is in the diff, with its key kept.
                let diff = text_at(&outs[0], &["settings", "diff"]);
                assert!(
                    diff.contains(r#""SECRET_TOKEN": "<redacted>""#)
                        || diff.contains(r#""SECRET_TOKEN":"<redacted>""#),
                    "{name}: {diff}"
                );
            }
            assert_eq!(f.read_settings(), text, "{name}");
        }
    }

    #[test]
    fn test_remove_preview_writes_nothing_and_shows_the_diff() {
        let f = Fx::new();
        let text = f.c.write_settings(SETTINGS);
        f.apply();
        write(&f.c.data_dir.join("rtk/1.0/rtk"), "bin", None);
        let applied = f.read_settings();
        let before = mem::snapshot(f.tmp());
        let out = f.remove(false, false, true);
        assert_eq!(mem::snapshot(f.tmp()), before);
        assert!(ok(&out));
        assert_eq!(at(&out, &["preview"]), &Value::Bool(true));
        assert_eq!(at(&out, &["settings", "changed"]), &Value::Bool(true));
        let diff = text_at(&out, &["settings", "diff"]);
        assert!(diff.contains("-      \"Bash(rtk *)\""), "{diff}");
        assert!(diff.contains("\"baseRef\""));
        assert_eq!(text_at(&out, &["rtk", "dir"]), p(&f.c.data_dir.join("rtk")));
        assert_eq!(text_at(&out, &["state", "path"]), p(&state::path(f.env())));
        assert_eq!(at(&out, &["state", "exists"]), &Value::Bool(true));
        assert_eq!(f.read_settings(), applied);
        assert_ne!(applied, text);
    }

    #[test]
    fn test_remove_returns_the_same_diff() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        f.apply();
        let preview = f.remove(false, false, true);
        let res = f.remove(false, false, false);
        assert_eq!(
            at(&res, &["settings", "diff"]),
            at(&preview, &["settings", "diff"])
        );
    }

    #[test]
    fn test_remove_preview_error_for_unparseable_settings() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        f.apply();
        write(&f.c.settings, "{nope", None);
        let out = f.remove(false, false, true);
        assert!(!ok(&out));
        assert!(text_at(&out, &["error"]).contains("not valid JSON"));
    }

    /// Rust-only (review): Python's `revert` checks for a blank file and parses the text before
    /// it reads the stored ops, so a malformed `settings_ops` is not an error then.
    #[test]
    fn a_malformed_settings_ops_matters_only_for_a_settings_file_that_parses() {
        let f = Fx::new();
        write(&state::path(f.env()), "{\"settings_ops\": 7}\n", None);
        let out = f.remove(false, false, true);
        assert!(ok(&out), "{out:?}");
        write(&f.c.settings, "{nope", None);
        let out = f.remove(false, false, true);
        assert!(
            text_at(&out, &["error"]).contains("not valid JSON"),
            "{out:?}"
        );
        write(&f.c.settings, "{}\n", None);
        let out = f.remove(false, false, true);
        assert_eq!(
            text_at(&out, &["error"]),
            "TypeError: 'int' object is not iterable"
        );
    }

    #[test]
    fn test_remove_preview_cli_flag() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        f.apply();
        let argv: Vec<String> = [
            "remove",
            "--preview",
            "--plugin-root",
            &p(&f.c.plugin),
            "--cwd",
            &p(f.tmp()),
        ]
        .iter()
        .map(|a| a.to_string())
        .collect();
        let (code, stdout, _) = setup::main(&argv, f.env(), &f.deps());
        assert_eq!(code, 0);
        assert_eq!(
            at(&json::loads(&stdout).unwrap(), &["preview"]),
            &Value::Bool(true)
        );
        assert!(f.state_exists());
    }

    #[test]
    fn test_rtk_is_never_deleted_when_data_dir_is_the_config_folder_or_above_plugins_data() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        f.apply();
        let config = f.c.mem.config.clone();
        for data in [
            config.clone(),
            config.join("plugins"),
            config.join("plugins/data"),
            f.c.mem.home.clone(),
        ] {
            write(&data.join("rtk/precious.txt"), "keep", None);
            state::write(f.env(), vec![("data_dir".into(), Value::Str(p(&data)))]).unwrap();
            let res = f.remove_env(&f.env_without_data(), false, false, false);
            assert!(data.join("rtk/precious.txt").exists(), "{data:?}");
            assert!(
                dumped(at(&res, &["warnings"])).contains("rtk"),
                "{data:?}: {res:?}"
            );
            f.apply();
        }
    }

    #[test]
    fn test_rtk_dir_that_resolves_elsewhere_is_left_alone() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        f.apply();
        let elsewhere = f.c.mem.config.join("elsewhere").join("rtk");
        write(&elsewhere.join("precious.txt"), "keep", None);
        fs::create_dir_all(&f.c.data_dir).unwrap();
        symlink(elsewhere.parent().unwrap(), f.c.data_dir.join("sub")).unwrap();
        state::write(
            f.env(),
            vec![("data_dir".into(), Value::Str(p(&f.c.data_dir.join("sub"))))],
        )
        .unwrap();
        f.remove_env(&f.env_without_data(), false, false, false);
        assert!(elsewhere.join("precious.txt").exists());
    }
}

mod remove_test {
    use super::*;

    #[test]
    fn test_remove_never_deletes_memory_notes_or_the_personal_folder() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        let personal = f.anth().join("personal-memory");
        write(
            &personal.join("user-me.md"),
            &Note::new("user-me").type_("user").desc("Me").text(),
            None,
        );
        f.apply();
        f.remove(false, true, false);
        assert!(personal.join("user-me.md").exists());
        assert!(personal.is_dir());
        assert!(!f.state_exists());
    }

    #[test]
    fn test_remove_deletes_only_the_rtk_folder_under_the_data_dir() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        f.apply();
        write(&f.c.data_dir.join("rtk/1.0/rtk"), "bin", None);
        write(&f.c.data_dir.join("other/keep.txt"), "keep", None);
        let res = f.remove(false, false, false);
        assert!(ok(&res), "{res:?}");
        assert!(!f.c.data_dir.join("rtk").exists());
        assert!(f.c.data_dir.join("other/keep.txt").exists());
    }

    fn pairs(kv: &[(&str, &str)]) -> Value {
        let mut m = Map::new();
        for (k, v) in kv {
            m.insert((*k).into(), Value::Str((*v).into()));
        }
        Value::Object(m)
    }

    #[test]
    fn test_remove_deletes_the_rtk_history_folder() {
        // I4: the folder bin/rtk and bin/rtk-hook point RTK_DB_PATH, RTK_RECALL_DB and
        // RTK_TEE_DIR at.
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        f.apply();
        let history = state::rtk_data_dir(f.env());
        write(&history.join("history.db"), "commands", None);
        write(&history.join("tee").join("1_x.log"), "out", None);
        let preview = f.remove(false, false, true);
        assert_eq!(
            at(&preview, &["rtk_history"]),
            &pairs(&[("dir", &p(&history)), ("status", "delete")])
        );
        assert!(history.join("history.db").exists());
        let res = f.remove(false, false, false);
        assert!(ok(&res), "{res:?}");
        assert_eq!(at(&res, &["rtk_history"]), &pairs(&[("status", "deleted")]));
        assert!(!history.exists());
    }

    #[test]
    fn test_remove_without_rtk_history_reports_nothing_to_delete() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        f.apply();
        let preview = f.remove(false, false, true);
        assert_eq!(
            text_at(&preview, &["rtk_history", "status"]),
            "nothing to delete"
        );
        assert_eq!(
            at(&f.remove(false, false, false), &["rtk_history"]),
            &pairs(&[("status", "nothing to delete")])
        );
    }

    #[test]
    fn test_remove_does_not_follow_a_symlinked_rtk_history_folder() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        f.apply();
        let elsewhere = f.c.mem.config.join("elsewhere-history");
        write(&elsewhere.join("precious.txt"), "keep", None);
        let history = state::rtk_data_dir(f.env());
        fs::create_dir_all(history.parent().unwrap()).unwrap();
        symlink(&elsewhere, &history).unwrap();
        let res = f.remove(false, false, false);
        assert!(elsewhere.join("precious.txt").exists());
        let warnings = at(&res, &["warnings"]).as_array().unwrap();
        assert!(
            warnings
                .iter()
                .any(|w| w.as_str().unwrap().contains("history"))
        );
    }

    #[test]
    fn test_remove_refuses_a_data_dir_outside_the_config_folder() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        f.apply();
        let outside = f.tmp().join("outside");
        write(&outside.join("rtk/precious.txt"), "keep", None);
        state::write(f.env(), vec![("data_dir".into(), Value::Str(p(&outside)))]).unwrap();
        let res = f.remove_env(&f.env_without_data(), false, false, false);
        assert!(outside.join("rtk/precious.txt").exists());
        assert!(dumped(at(&res, &["warnings"])).contains("rtk"));
    }

    #[test]
    fn test_remove_does_not_follow_a_symlinked_rtk_folder() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        f.apply();
        let elsewhere = f.c.mem.config.join("elsewhere");
        write(&elsewhere.join("precious.txt"), "keep", None);
        fs::create_dir_all(&f.c.data_dir).unwrap();
        symlink(&elsewhere, f.c.data_dir.join("rtk")).unwrap();
        f.remove(false, false, false);
        assert!(elsewhere.join("precious.txt").exists());
    }

    #[test]
    fn test_remove_with_unparseable_settings_keeps_state_for_a_retry() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        f.apply();
        write(&f.c.settings, "{nope", None);
        let res = f.remove(false, false, false);
        assert!(!ok(&res));
        assert!(text_at(&res, &["error"]).contains("not valid JSON"));
        assert!(f.state_exists());
        assert_eq!(f.read_settings(), "{nope");
    }

    #[test]
    fn test_remove_without_a_prior_setup_is_harmless() {
        let f = Fx::new();
        let text = f.c.write_settings(SETTINGS);
        let res = f.remove(false, false, false);
        assert!(ok(&res));
        assert_eq!(f.read_settings(), text);
    }

    #[test]
    fn test_users_own_rules_survive_remove() {
        let f = Fx::new();
        f.c.write_settings(
            r#"{"model": "opus", "permissions": {"allow": ["Bash(rtk *)", "Bash(ls)"]}}"#,
        );
        f.apply();
        let mut data = json::loads(&f.read_settings()).unwrap();
        data.as_object_mut()
            .and_then(|m| m.get_mut("permissions"))
            .and_then(Value::as_object_mut)
            .and_then(|m| m.get_mut("allow"))
            .and_then(Value::as_array_mut)
            .unwrap()
            .push(Value::Str("Bash(make *)".into()));
        write(
            &f.c.settings,
            &(json::dumps(&data, &json::INDENT2) + "\n"),
            None,
        );
        f.remove(false, false, false);
        assert_eq!(
            allow(&f.c.settings),
            ["Bash(rtk *)", "Bash(ls)", "Bash(make *)"]
        );
        let got = json::loads(&f.read_settings()).unwrap();
        assert!(got.as_object().unwrap().get("worktree").is_none());
    }
}

/// Not Python ports: the F-15 additions (D§8.8, GC-14).
mod guard_binary {
    use super::*;

    #[test]
    fn apply_ensures_the_located_guardrails_binary_with_a_60_s_budget() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        let calls = RefCell::new(Vec::new());
        let ensure = |g: &Path, budget: u64, _: &Env| {
            calls.borrow_mut().push((g.to_path_buf(), budget));
            GuardBinaryStatus::Failed("download failed".into())
        };
        let deps = SetupDeps {
            ensure_guard_binary: &ensure,
            ..f.deps()
        };
        let out = f.preview_d(None, &deps);
        let guard = f.tmp().join("plugins").join("anthology-guardrails");
        assert_eq!(text_at(&out, &["guard_binary", "root"]), p(&guard));
        assert!(calls.borrow().is_empty(), "preview never ensures");
        let res = f.apply_d(&deps);
        assert!(ok(&res));
        assert_eq!(*calls.borrow(), [(guard, 60)]);
        assert_eq!(
            text_at(&res, &["guard_binary", "status"]),
            "failed: download failed"
        );
    }

    #[test]
    fn a_refused_apply_downloads_nothing() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        let ensure = |_: &Path, _: u64, _: &Env| -> GuardBinaryStatus {
            panic!("nothing is downloaded for a stale preview")
        };
        let deps = SetupDeps {
            ensure_guard_binary: &ensure,
            ..f.deps()
        };
        let res = f.apply_with(&f.c.plugin, &sha("other"), &deps);
        assert_eq!(
            text_at(&res, &["error"]),
            "settings.json changed since the preview; run the preview again"
        );
    }

    #[test]
    fn a_stale_result_after_the_downloads_keeps_the_guard_status() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        let digest = f.digest();
        let path = f.c.settings.clone();
        let ensure = move |_: &Path, _: u64, _: &Env| {
            fs::write(&path, "{}\n").unwrap();
            GuardBinaryStatus::Installed
        };
        let deps = SetupDeps {
            ensure_guard_binary: &ensure,
            ..f.deps()
        };
        let res = f.apply_with(&f.c.plugin, &digest, &deps);
        assert!(text_at(&res, &["error"]).contains("changed during setup"));
        assert_eq!(text_at(&res, &["guard_binary", "status"]), "installed");
        assert_eq!(f.read_settings(), "{}\n");
    }

    fn installed_plugins(f: &Fx, keys: &[&str]) {
        let plugins: Vec<String> = keys
            .iter()
            .map(|k| format!(r#""{k}": [{{"scope": "user", "installPath": "/p"}}]"#))
            .collect();
        write(
            &f.c.mem.config.join("plugins/installed_plugins.json"),
            &format!(r#"{{"version": 2, "plugins": {{{}}}}}"#, plugins.join(", ")),
            None,
        );
    }

    #[test]
    fn remove_deletes_ah_bin_only_without_the_guardrails_plugin() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        let bin = f.anth().join("bin");
        let status = |out: &Value| text_at(out, &["guard_binary", "status"]).to_string();

        assert_eq!(status(&f.remove(false, false, true)), "nothing to delete");

        write(&bin.join("1.0.0/anthology"), "x", None);
        installed_plugins(&f, &["anthology@m", "anthology-guardrails@m"]);
        let out = f.remove(false, false, true);
        assert_eq!(status(&out), "kept (anthology-guardrails still uses it)");
        assert_eq!(at(&out, &["guard_binary", "dir"]), &Value::Null);
        assert_eq!(
            status(&f.remove(false, false, false)),
            "kept (anthology-guardrails still uses it)"
        );
        assert!(bin.join("1.0.0/anthology").exists());

        write(
            &f.c.mem.config.join("plugins/installed_plugins.json"),
            "{not json",
            None,
        );
        assert_eq!(
            status(&f.remove(false, false, false)),
            "kept (anthology-guardrails still uses it)"
        );
        // a registry of an unexpected shape keeps it too
        for odd in ["[]", r#"{"version": 2}"#, r#"{"plugins": []}"#] {
            write(
                &f.c.mem.config.join("plugins/installed_plugins.json"),
                odd,
                None,
            );
            assert_eq!(
                status(&f.remove(false, false, false)),
                "kept (anthology-guardrails still uses it)",
                "{odd}"
            );
        }

        installed_plugins(&f, &["anthology@m"]);
        let out = f.remove(false, false, true);
        assert_eq!(status(&out), "delete");
        assert_eq!(text_at(&out, &["guard_binary", "dir"]), p(&bin));
        assert!(bin.exists());
        assert_eq!(status(&f.remove(false, false, false)), "deleted");
        assert!(!bin.exists());
    }

    #[test]
    fn a_symlinked_ah_bin_loses_only_the_link() {
        let f = Fx::new();
        let elsewhere = f.tmp().join("elsewhere");
        write(&elsewhere.join("keep"), "x", None);
        fs::create_dir_all(f.anth()).unwrap();
        symlink(&elsewhere, f.anth().join("bin")).unwrap();
        let out = f.remove(false, false, false);
        assert_eq!(text_at(&out, &["guard_binary", "status"]), "deleted");
        assert!(!f.anth().join("bin").exists());
        assert!(elsewhere.join("keep").exists());
    }
}

mod cli_main {
    //! `main`'s argument parsing and exit codes beyond `CliTest` (not Python ports).
    use super::*;

    fn run(f: &Fx, args: &[&str]) -> (i32, String, String) {
        let argv: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        setup::main(&argv, f.env(), &f.deps())
    }

    #[test]
    fn usage_errors_exit_2_with_argparse_text() {
        let f = Fx::new();
        let (code, out, err) = run(&f, &["apply", "--plugin-root", "/x"]);
        assert_eq!((code, out.as_str()), (2, ""));
        assert!(
            err.ends_with(
                "anthology setup apply: error: the following arguments are required: \
                 --expect-sha256\n"
            ),
            "{err}"
        );
        let (code, _, err) = run(&f, &[]);
        assert_eq!(code, 2);
        assert!(err.contains("required: command"), "{err}");
    }

    #[test]
    fn an_exception_is_printed_as_ok_false_and_exits_1() {
        let f = Fx::new();
        fs::create_dir_all(&f.c.settings).unwrap(); // reading it raises IsADirectoryError
        let (code, out, _) = run(
            &f,
            &[
                "preview",
                "--plugin-root",
                &p(&f.c.plugin),
                "--cwd",
                &p(f.tmp()),
            ],
        );
        assert_eq!(code, 1);
        let v = json::loads(&out).unwrap();
        assert_eq!(at(&v, &["ok"]), &Value::Bool(false));
        assert!(
            text_at(&v, &["error"]).starts_with("IsADirectoryError: [Errno 21]"),
            "{out}"
        );
        assert!(out.starts_with("{\n  \"ok\": false,\n") && out.ends_with("}\n"));
    }
}

mod cli_test {
    //! `CliTest`'s exit-code rows run through the CLI wrapper
    //! (`crates/anthology-cli/src/cmd/setup.rs`); this module is the runtime half of
    //! `LegacyPromotionTest.test_cli_promote_flag_is_repeatable`'s flag parsing.
    use super::*;

    #[test]
    fn promote_is_repeatable_and_reaches_apply() {
        let f = Fx::new();
        let text = f.c.write_settings(SETTINGS);
        let argv: Vec<String> = [
            "apply",
            "--plugin-root",
            &p(&f.c.plugin),
            "--expect-sha256",
            &sha(&text),
            "--cwd",
            &p(f.tmp()),
            "--promote",
            "clean.md",
            "--promote",
            "mail.md",
        ]
        .iter()
        .map(|a| a.to_string())
        .collect();
        // A repo with no legacy notes: the refusal names both flags' files.
        let tmp = f.tmp().to_path_buf();
        let repo_root = move |_: &Path, _: &Env| Some(tmp.clone());
        let repo_key = |_: &Path, _: &Env| "k".to_string();
        let approve = |_: &Path, _: &Env| Ok("k".to_string());
        let deps = SetupDeps {
            memory: MemoryDeps {
                repo_root: &repo_root,
                repo_key: &repo_key,
                approve: &approve,
            },
            ..f.deps()
        };
        let (code, out, _) = setup::main(&argv, f.env(), &deps);
        assert_eq!(code, 1);
        let v = json::loads(&out).unwrap();
        assert!(
            text_at(&v, &["error"]).contains("not one: clean.md, mail.md."),
            "{out}"
        );
    }
}
