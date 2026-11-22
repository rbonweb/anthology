//! Port of `tests/test_setup.py` (RT4), plus Rust tests for what the port adds (F-15: the
//! `guard_binary` step and the `<ah>/bin` remove of D§8.8).
//!
//! Modules are the Python classes; every ported test keeps the Python method name. Python's
//! `mock.patch` sites go through `SetupDeps` (GC-19): `atl_install` is a fake `AtlWiring`,
//! `platform.system` is `uname`, `state.write` is `state_write`, `settings_patch.write_text` is
//! `settings_write`, `memory_sync.{repo_root,repo_key,approve}` is `memory`. The layout is
//! `anthology_testkit::rt_c::SetupCase` (its docs say why the plugin is a small copy).
//!
//! Python's `apply` raised where the Rust `apply` returns `main`'s
//! `{"ok": false, "error": "<Class>: <msg>"}` result.
//!
//! The memory functions are the real `anthology_memory` ones. Python's `fake_repo` (no git)
//! patched `memory_sync`'s git calls; these tests use a real `git init` repo instead and skip
//! without git (`needs_git`). The privacy scan's `socket.gethostname` patch is
//! `MemoryDeps::hostname`.
#![forbid(unsafe_code)]

use std::cell::RefCell;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use anthology_core::lock::Lock;
use anthology_core::{Env, state};
use anthology_memory::approve as memory_approve;
use anthology_memory::sync::{self, SyncDeps};
use anthology_pycompat::json::{self, Map, Value};
use anthology_pycompat::pyerr::PyExc;
use anthology_runtime::atl_install::Reconciled;
use anthology_runtime::bootstrap::GuardBinaryStatus;
use anthology_runtime::session_start::AtlWiring;
use anthology_runtime::setup::{self, ATL_TIMEOUT, GUARD_NOT_FOUND, MemoryDeps, SetupDeps};
use anthology_settings as settings;
use anthology_testkit::mem::{self, FAKE_REPO_KEY, MemCase, Note, note_text, write};
use anthology_testkit::rows::{Row, rows};
use anthology_testkit::rt_c::{SETTINGS, SetupCase, pretty};
use anthology_testkit::sandbox::repo_root;

const BYPASS: &str = "Note: `Bash(atl *)` also auto-approves `atl bypass <cmd>`, which runs any \
                      command. atl's hook still applies the deny and ask rules in your settings \
                      files (user, project, local and managed) to the command it wraps, but not \
                      rules given with `--settings` or `--disallowedTools`, server-managed \
                      settings or an MDM profile.";

// --- the fixture --------------------------------------------------------------------------

/// `FakeAtl`: atl_install's interface; `load_lock` returns the parsed lock (no validation),
/// `reconcile` records the call and returns `result`.
struct FakeAtl {
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

impl FakeAtl {
    fn new() -> FakeAtl {
        FakeAtl {
            calls: RefCell::new(Vec::new()),
            result: RefCell::new(Reconciled {
                atl_path: None,
                warning: None,
                installed: true,
            }),
            exc: RefCell::new(None),
            on_reconcile: RefCell::new(None),
        }
    }
}

impl AtlWiring for FakeAtl {
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

/// The version in `<plugin>/.claude-plugin/plugin.json`, which setup records as `setup_done`.
fn manifest_version(plugin: &Path) -> String {
    let text = fs::read_to_string(plugin.join(".claude-plugin").join("plugin.json")).unwrap();
    text_at(&json::loads(&text).unwrap(), &["version"]).to_string()
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
    atl: FakeAtl,
}

impl Fx {
    fn new() -> Fx {
        Fx {
            c: SetupCase::new(),
            atl: FakeAtl::new(),
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
            atl: Some(&self.atl),
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

    fn atl_calls(&self) -> usize {
        self.atl.calls.borrow().len()
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

    /// `self.apply(cwd=cwd, skip_approve=…, promote=…)`: with the preview's digest.
    fn apply_in(
        &self,
        cwd: &Path,
        skip_approve: bool,
        promote: &[&str],
        deps: &SetupDeps<'_>,
    ) -> Value {
        let preview = setup::preview(&self.c.plugin, Some(cwd), self.env(), deps);
        let digest = text_at(&preview, &["settings", "sha256"]).to_string();
        let promote: Vec<String> = promote.iter().map(|s| s.to_string()).collect();
        setup::apply(
            &self.c.plugin,
            &digest,
            Some(cwd),
            skip_approve,
            self.env(),
            &promote,
            deps,
        )
    }

    /// `self.remove(cwd=cwd, …)`.
    fn remove_in(&self, cwd: &Path, unapprove: bool, unapprove_all: bool, preview: bool) -> Value {
        setup::remove(
            &self.c.plugin,
            Some(cwd),
            unapprove,
            unapprove_all,
            self.env(),
            preview,
            &self.deps(),
        )
    }

    /// `memory_sync.is_approved(key)`.
    fn approved(&self, key: &str) -> bool {
        memory_approve::is_approved(key, self.env())
    }

    /// `memory_sync._save_approved({"version": 1, "repos": {key: {}, …}})`.
    fn save_approved(&self, keys: &[&str]) {
        let repos: Vec<String> = keys.iter().map(|k| format!("{k:?}: {{}}")).collect();
        write(
            &memory_approve::approved_path(self.env()),
            &format!("{{\"version\": 1, \"repos\": {{{}}}}}", repos.join(", ")),
            None,
        );
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
        assert_eq!(f.atl_calls(), 0);
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
            "atl",
            "platform_supported",
            "plugin_read_rule",
            "browseros_tools",
            "plugin_data",
            "path_wrappers",
            "agent_memory_path",
            "subagent_model",
            "atl_savings",
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
        assert!(diff.contains("Bash(atl *)"));
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
            &root.join("atl.lock"),
            r#"{"version": "1.2.1-anthology.1", "tag": "t", "targets": {}}"#,
            None,
        );
        // Python's fake `target` returned "test-target"; the Rust target is
        // atl_install::target for the `uname` seam.
        let deps = SetupDeps {
            uname: linux,
            ..f.deps()
        };
        let out = setup::preview(&root, Some(f.tmp()), f.env(), &deps);
        assert_eq!(text_at(&out, &["atl", "version"]), "1.2.1-anthology.1");
        assert_eq!(
            text_at(&out, &["atl", "target"]),
            "x86_64-unknown-linux-musl"
        );
        assert_eq!(
            text_at(&out, &["atl", "dest"]),
            p(&f.c.data_dir.join("atl/1.2.1-anthology.1/atl"))
        );
    }

    #[test]
    fn test_rtk_step_skipped_when_wiring_is_absent() {
        let f = Fx::new();
        let deps = SetupDeps {
            atl: None,
            ..f.deps()
        };
        let out = f.preview_d(None, &deps);
        assert!(text_at(&out, &["atl", "skipped"]).contains("atl wiring not present"));
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

mod real_atl_test {
    use super::*;
    use anthology_runtime::atl_install::{self, AtlDeps};
    use anthology_runtime::fetch::Fetch;
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

    /// The real `atl_install` behind setup's adapters, with a fake download.
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
            let blob = release::tarball(ATL_BODY);
            let root = m.tmp.join("plugin");
            let lock = release::write_lock(&root.join("atl.lock"), &blob, VERSION);
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
            atl: Some(real),
            ensure_guard_binary: &present,
            ..SetupDeps::default()
        }
    }

    #[test]
    fn test_preview_names_the_version_target_and_destination() {
        let r = Rx::new();
        let fetch = Fake(AtlFakeFetch::default());
        let real = Real(&fetch);
        let out = setup::atl_preview(&r.root, Some(&r.data()), &deps(&real));
        let mut want = Map::new();
        want.insert("version".into(), Value::Str(VERSION.into()));
        want.insert("target".into(), Value::Str(TARGET.into()));
        want.insert(
            "dest".into(),
            Value::Str(p(&r.data_dir.join("atl").join(VERSION).join("atl"))),
        );
        assert_eq!(out, Value::Object(want));
    }

    #[test]
    fn test_apply_installs_then_reports_up_to_date() {
        let r = Rx::new();
        let fetch = Fake(AtlFakeFetch::serving(
            &atl_url_for(&r.lock, TARGET),
            &r.blob,
        ));
        let real = Real(&fetch);
        let d = deps(&real);
        let data = r.data();
        assert_eq!(
            setup::atl_apply(&r.root, Some(&data), &r.m.env, &d),
            ("installed".to_string(), None)
        );
        assert_eq!(
            setup::atl_apply(&r.root, Some(&data), &r.m.env, &d),
            ("up to date".to_string(), None)
        );
        let want = r.data_dir.join("atl").join(VERSION).join("atl");
        assert_eq!(
            state::read(&r.m.env)
                .get("atl_path")
                .and_then(Value::as_str),
            Some(p(&want).as_str())
        );
        assert!(fs::metadata(&want).unwrap().permissions().mode() & 0o111 != 0);
        let calls = fetch.0.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, ATL_TIMEOUT);
    }

    #[test]
    fn test_a_bad_download_is_a_warning() {
        let r = Rx::new();
        let fetch = Fake(AtlFakeFetch::serving(
            &atl_url_for(&r.lock, TARGET),
            b"wrong",
        ));
        let real = Real(&fetch);
        let (status, warning) = setup::atl_apply(&r.root, Some(&r.data()), &r.m.env, &deps(&real));
        assert_eq!(status, "warning");
        assert!(
            warning
                .unwrap()
                .contains("atl is not installed or up to date")
        );
        assert!(state::read(&r.m.env).get("atl_path").is_none());
    }

    #[test]
    fn test_an_invalid_lock_is_a_warning() {
        let r = Rx::new();
        fs::write(r.root.join("atl.lock"), r#"{"version": "1"}"#).unwrap();
        let fetch = Fake(AtlFakeFetch::default());
        let real = Real(&fetch);
        let d = deps(&real);
        let (status, warning) = setup::atl_apply(&r.root, Some(&r.data()), &r.m.env, &d);
        assert_eq!(status, "failed");
        assert!(warning.unwrap().contains("atl.lock"));
        let mut want = Map::new();
        want.insert(
            "skipped".into(),
            Value::Str("atl wiring not present (no atl.lock)".into()),
        );
        assert_eq!(
            setup::atl_preview(&r.root, Some(&r.data()), &d),
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
        assert!(allow(&f.c.settings).contains(&"Bash(atl *)".to_string()));
        assert_eq!(text_at(&data, &["worktree", "baseRef"]), "head");
        assert_eq!(text_at(&data, &["model"]), "opus");
        let st = f.state();
        assert_eq!(
            st.get("setup_done").and_then(Value::as_str),
            Some(manifest_version(&f.c.plugin).as_str())
        );
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
        assert_eq!(f.atl_calls(), 0);
    }

    #[test]
    fn test_unparseable_settings_stop_apply_before_anything_else() {
        let f = Fx::new();
        write(&f.c.settings, "{nope", None);
        let res = f.apply();
        assert!(!ok(&res));
        assert!(text_at(&res, &["error"]).contains("not valid JSON"));
        assert_eq!(f.read_settings(), "{nope");
        assert_eq!(f.atl_calls(), 0);
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
    fn test_apply_runs_the_atl_reconcile_and_surfaces_its_warning() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        *f.atl.result.borrow_mut() = Reconciled {
            atl_path: None,
            warning: Some("atl download failed: offline".into()),
            installed: false,
        };
        let res = f.apply();
        let calls = f.atl.calls.borrow();
        assert_eq!(calls.len(), 1);
        let (data_dir, lock, timeout) = &calls[0];
        assert_eq!(data_dir, &f.c.data_dir);
        // The parsed lock (Python's `fetch` argument is gone: the real wiring always uses the
        // production fetch).
        assert_eq!(lock, &parsed_lock(&f.c.plugin.join("atl.lock")).unwrap());
        assert!(*timeout >= 60.0);
        assert_eq!(text_at(&res, &["atl", "status"]), "warning");
        assert!(
            at(&res, &["warnings"])
                .as_array()
                .unwrap()
                .contains(&Value::Str("atl download failed: offline".into()))
        );
        assert!(ok(&res));
        assert_eq!(
            f.state().get("setup_done").and_then(Value::as_str),
            Some(manifest_version(&f.c.plugin).as_str())
        );
    }

    #[test]
    fn test_apply_reports_installed_and_up_to_date() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        assert_eq!(text_at(&f.apply(), &["atl", "status"]), "installed");
        *f.atl.result.borrow_mut() = Reconciled {
            atl_path: Some(PathBuf::from("x")),
            warning: None,
            installed: false,
        };
        assert_eq!(text_at(&f.apply(), &["atl", "status"]), "up to date");
    }

    #[test]
    fn test_missing_lock_is_a_warning_not_an_exception() {
        let f = Fx::new();
        let root = f.tmp().join("p").join("anthology");
        let data = p(&f.c.data_dir);
        let mut want = Map::new();
        want.insert(
            "skipped".into(),
            Value::Str("atl wiring not present (no atl.lock)".into()),
        );
        assert_eq!(
            setup::atl_preview(&root, Some(&data), &f.deps()),
            Value::Object(want)
        );
        let (status, warning) = setup::atl_apply(&root, Some(&data), f.env(), &f.deps());
        assert_eq!(status, "failed");
        assert!(warning.unwrap().contains("atl install failed"));
        assert_eq!(f.atl_calls(), 0);
    }

    #[test]
    fn test_rtk_exception_is_a_warning() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        *f.atl.exc.borrow_mut() = Some(PyExc::Other("RuntimeError", "kaboom".into()));
        let res = f.apply();
        assert!(ok(&res));
        assert!(dumped(at(&res, &["warnings"])).contains("kaboom"));
    }

    #[test]
    fn test_absent_atl_wiring_is_skipped() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        let deps = SetupDeps {
            atl: None,
            ..f.deps()
        };
        let res = f.apply_d(&deps);
        assert!(ok(&res));
        assert!(text_at(&res, &["atl", "status"]).contains("skipped"));
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
        assert_eq!(f.atl.calls.borrow()[0].0, f.c.data_dir);
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
        assert!(fs::read_to_string(&target).unwrap().contains("Bash(atl *)"));
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

    const REMOTE: &str = "https://example.invalid/acme/app.git";

    #[test]
    fn test_apply_approves_the_repo_and_remove_keeps_it_unless_asked() {
        if !mem::needs_git() {
            return;
        }
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        let repo = f.c.mem.make_repo("repo", Some(REMOTE));
        let res = f.apply_in(&repo, false, &[], &f.deps());
        assert_eq!(text_at(&res, &["memory", "approved_key"]), FAKE_REPO_KEY);
        assert!(f.approved(FAKE_REPO_KEY));
        let attrs = fs::read_to_string(repo.join(".gitattributes")).unwrap();
        assert!(attrs.contains(".claude/memory/MEMORY.md merge=union"));
        f.remove_in(&repo, false, false, false);
        assert!(f.approved(FAKE_REPO_KEY));
        f.apply_in(&repo, false, &[], &f.deps());
        f.remove_in(&repo, true, false, false);
        assert!(!f.approved(FAKE_REPO_KEY));
    }

    #[test]
    fn test_skip_approve() {
        if !mem::needs_git() {
            return;
        }
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        let repo = f.c.mem.make_repo("repo", Some(REMOTE));
        let res = f.apply_in(&repo, true, &[], &f.deps());
        assert_eq!(at(&res, &["memory", "approved_key"]), &Value::Null);
        assert!(!f.approved(FAKE_REPO_KEY));
    }

    #[test]
    fn test_unsafe_target_is_reported_not_raised_and_setup_carries_on() {
        if !mem::needs_git() {
            return;
        }
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        let repo = f.c.mem.make_repo("repo", Some(REMOTE));
        fs::write(repo.join("elsewhere"), "x").unwrap();
        symlink(repo.join("elsewhere"), repo.join(".gitattributes")).unwrap();
        let res = f.apply_in(&repo, false, &[], &f.deps());
        assert!(ok(&res), "{res:?}");
        assert!(text_at(&res, &["memory", "approve_error"]).contains(".gitattributes"));
        assert!(f.approved(FAKE_REPO_KEY)); // approval is already recorded
        assert_eq!(
            text_at(&Value::Object(f.state()), &["setup_done"]),
            manifest_version(&f.c.plugin)
        );
        assert!(f.anth().join("personal-memory").is_dir());
        assert_eq!(fs::read_to_string(repo.join("elsewhere")).unwrap(), "x");
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
                ..MemoryDeps::default()
            },
            ..f.deps()
        };
        let res = f.apply_d(&deps);
        assert!(ok(&res));
        assert!(text_at(&res, &["memory", "approve_error"]).contains("refusing"));
        assert_eq!(text_at(&res, &["memory", "approved_key"]), "k");
    }
}

mod legacy_promotion_test {
    //! B3: moving a legacy note into repo memory is an explicit per-note choice, behind the
    //! privacy scan.
    use super::*;

    fn alice_laptop() -> Option<String> {
        Some("alice-laptop".into())
    }

    struct L {
        f: Fx,
        text: String,
        root: PathBuf,
        old: PathBuf,
        repo_mem: PathBuf,
        personal: PathBuf,
    }

    impl L {
        /// `setUp`; `None` without git.
        fn new() -> Option<L> {
            if !mem::needs_git() {
                return None;
            }
            let f = Fx::new();
            let text = f.c.write_settings(SETTINGS);
            let root =
                f.c.mem
                    .make_repo("repo", Some("https://example.invalid/acme/app.git"));
            let encoded: String = p(&root)
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                .collect();
            let old = f.c.mem.config.join("projects").join(encoded).join("memory");
            let note = |n: Note| note_text(&n);
            write(
                &old.join("clean.md"),
                &note(
                    Note::new("clean")
                        .desc("A clean note")
                        .body("Use Docker.\n"),
                ),
                None,
            );
            write(
                &old.join("dirty.md"),
                &note(
                    Note::new("dirty")
                        .type_("reference")
                        .desc("Where things live")
                        .body(&("Checkout in /".to_string() + "Users/alice/src\n")),
                ),
                None,
            );
            write(
                &old.join("mail.md"),
                &note(
                    Note::new("mail")
                        .type_("reference")
                        .desc("Contact")
                        .body(&("Ask alice".to_string() + "@" + "mail.acme.zz\n")),
                ),
                None,
            );
            write(
                &old.join("user-me.md"),
                &note(Note::new("user-me").type_("user").desc("About me")),
                None,
            );
            write(&old.join("MEMORY.md"), "- index\n", None);
            Some(L {
                repo_mem: root.join(".claude/memory"),
                personal: f.anth().join("personal-memory"),
                f,
                text,
                root,
                old,
            })
        }

        fn deps(&self) -> SetupDeps<'_> {
            SetupDeps {
                memory: MemoryDeps {
                    hostname: Some(alice_laptop),
                    ..MemoryDeps::default()
                },
                ..self.f.deps()
            }
        }

        fn apply(&self, promote: &[&str]) -> Value {
            self.f.apply_in(&self.root, false, promote, &self.deps())
        }

        /// `notes()`: the preview's legacy notes by file name.
        fn notes(&self) -> Vec<(String, Value)> {
            let out = setup::preview(
                &self.f.c.plugin,
                Some(&self.root),
                self.f.env(),
                &self.deps(),
            );
            at(&out, &["memory", "legacy_notes"])
                .as_array()
                .unwrap()
                .iter()
                .map(|n| (text_at(n, &["file"]).to_string(), n.clone()))
                .collect()
        }
    }

    fn note<'a>(notes: &'a [(String, Value)], file: &str) -> &'a Value {
        &notes.iter().find(|(f, _)| f == file).unwrap().1
    }

    fn files(dir: &Path, only_md: bool) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| !only_md || n.ends_with(".md"))
            .collect();
        v.sort();
        v
    }

    fn findings(kind: &str, line: i64) -> Value {
        json::loads(&format!("[{{\"kind\": {kind:?}, \"line\": {line}}}]")).unwrap()
    }

    fn strs_at(res: &Value, path: &[&str]) -> Vec<String> {
        at(res, path)
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect()
    }

    fn blocked_files(res: &Value) -> Vec<String> {
        at(res, &["memory", "blocked"])
            .as_array()
            .unwrap()
            .iter()
            .map(|b| text_at(b, &["file"]).to_string())
            .collect()
    }

    #[test]
    fn test_preview_lists_legacy_notes_defaults_personal_and_writes_nothing() {
        let Some(l) = L::new() else { return };
        write(&l.repo_mem.join("mail.md"), "the repo's own note\n", None);
        let before = mem::snapshot(l.f.tmp());
        let notes = l.notes();
        assert_eq!(mem::snapshot(l.f.tmp()), before);
        let mut names: Vec<&str> = notes.iter().map(|(f, _)| f.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, ["clean.md", "dirty.md", "mail.md", "user-me.md"]);
        let clean = note(&notes, "clean.md");
        assert_eq!(text_at(clean, &["name"]), "clean");
        assert_eq!(text_at(clean, &["description"]), "A clean note");
        assert_eq!(text_at(clean, &["type"]), "project");
        assert_eq!(text_at(clean, &["default"]), "personal");
        assert_eq!(at(clean, &["findings"]), &Value::Array(Vec::new()));
        assert_eq!(at(clean, &["movable"]), &Value::Bool(true));
        assert_eq!(text_at(clean, &["status"]), "new");
        let diff = text_at(clean, &["diff"]);
        assert!(diff.starts_with("--- /dev/null\n+++ b/.claude/memory/clean.md\n"));
        assert!(diff.contains("+Use Docker."));
        let dirty = note(&notes, "dirty.md");
        assert_eq!(at(dirty, &["findings"]), &findings("home-path", 8));
        assert_eq!(at(dirty, &["movable"]), &Value::Bool(false));
        assert!(text_at(dirty, &["reason"]).contains("home path (line 8)"));
        let mail = note(&notes, "mail.md");
        let kinds: Vec<&str> = at(mail, &["findings"])
            .as_array()
            .unwrap()
            .iter()
            .map(|f| text_at(f, &["kind"]))
            .collect();
        assert_eq!(kinds, ["email"]);
        assert_eq!(text_at(mail, &["status"]), "conflict");
        let would = text_at(mail, &["would_create"]);
        let date = would
            .strip_prefix("mail.migrated-")
            .and_then(|r| r.strip_suffix(".md"))
            .expect("mail.migrated-<date>.md");
        let parts: Vec<&str> = date.split('-').collect();
        assert_eq!(parts.iter().map(|d| d.len()).collect::<Vec<_>>(), [4, 2, 2]);
        assert!(parts.iter().all(|d| d.bytes().all(|b| b.is_ascii_digit())));
        assert!(text_at(mail, &["diff"]).contains("-the repo's own note"));
        assert!(text_at(mail, &["diff"]).contains(&format!("+++ b/.claude/memory/{would}")));
        let user = note(&notes, "user-me.md");
        assert_eq!(at(user, &["movable"]), &Value::Bool(false));
        assert!(text_at(user, &["reason"]).contains("type: user"));
        for (_, n) in &notes {
            for f in at(n, &["findings"]).as_array().unwrap() {
                let mut keys: Vec<&str> = f.as_object().unwrap().iter().map(|(k, _)| k).collect();
                keys.sort_unstable();
                assert_eq!(keys, ["kind", "line"]);
            }
        }
        let all: Vec<&Value> = notes.iter().map(|(_, n)| at(n, &["findings"])).collect();
        assert!(
            !all.iter()
                .any(|f| dumped(f).contains(&("alice".to_string() + "@")))
        );
    }

    #[test]
    fn test_preview_marks_identical_notes_and_lists_ledgered_ones() {
        let Some(l) = L::new() else { return };
        let deps = SyncDeps::from_env(l.f.env());
        sync::migrate(
            &l.old,
            None,
            &l.personal,
            Some(&l.f.anth().join("migrated.json")),
            None,
            &[],
            None,
            &deps,
        );
        write(
            &l.repo_mem.join("clean.md"),
            &fs::read_to_string(l.old.join("clean.md")).unwrap(),
            None,
        );
        let notes = l.notes();
        assert_eq!(notes.len(), 4); // the ledger never hides a note from the choice
        let clean = note(&notes, "clean.md");
        assert_eq!(text_at(clean, &["status"]), "identical");
        assert_eq!(text_at(clean, &["diff"]), "");
    }

    #[test]
    fn test_explicit_selection_moves_only_the_clean_ones() {
        let Some(l) = L::new() else { return };
        let res = l.apply(&["clean.md", "dirty.md"]);
        assert!(ok(&res), "{res:?}");
        assert_eq!(files(&l.repo_mem, false), ["MEMORY.md", "clean.md"]);
        assert_eq!(
            fs::read(l.repo_mem.join("clean.md")).unwrap(),
            fs::read(l.old.join("clean.md")).unwrap()
        );
        assert!(
            fs::read_to_string(l.repo_mem.join("MEMORY.md"))
                .unwrap()
                .contains("[clean](clean.md)")
        );
        assert_eq!(
            files(&l.personal, true),
            ["dirty.md", "mail.md", "user-me.md"]
        );
        let blocked = at(&res, &["memory", "blocked"]).as_array().unwrap();
        assert_eq!(blocked_files(&res), ["dirty.md"]);
        assert_eq!(at(&blocked[0], &["findings"]), &findings("home-path", 8));
        assert!(text_at(&blocked[0], &["reason"]).contains("home path"));
        assert!(!dumped(at(&res, &["memory", "blocked"])).contains("alice"));
        assert_eq!(strs_at(&res, &["memory", "migrated", "repo"]), ["clean.md"]);
        let mut personal = strs_at(&res, &["memory", "migrated", "personal"]);
        personal.sort();
        assert_eq!(personal, ["dirty.md", "mail.md", "user-me.md"]);
        let ledger =
            json::loads(&fs::read_to_string(l.f.anth().join("migrated.json")).unwrap()).unwrap();
        assert_eq!(at(&ledger, &["done"]).as_array().unwrap().len(), 4);
        // the next session start copies nothing more
        let mut sd = SyncDeps::from_env(l.f.env());
        sd.privacy.hostname = alice_laptop;
        let (_, msgs) = sync::run(&l.root, l.f.env(), &sd);
        assert!(!msgs.iter().any(|m| m.contains("note(s)")), "{msgs:?}");
        assert_eq!(files(&l.repo_mem, true), ["MEMORY.md", "clean.md"]);
        let mut personal = files(&l.personal, true);
        personal.retain(|n| n != "MEMORY.md");
        assert_eq!(personal, ["dirty.md", "mail.md", "user-me.md"]);
    }

    #[test]
    fn test_without_promote_everything_stays_personal() {
        let Some(l) = L::new() else { return };
        let res = l.apply(&[]);
        assert!(ok(&res), "{res:?}");
        assert!(!l.repo_mem.exists());
        assert_eq!(at(&res, &["memory", "blocked"]), &Value::Array(Vec::new()));
        let mut personal = strs_at(&res, &["memory", "migrated", "personal"]);
        personal.sort();
        assert_eq!(personal, ["clean.md", "dirty.md", "mail.md", "user-me.md"]);
    }

    #[test]
    fn test_a_promoted_user_note_is_blocked() {
        let Some(l) = L::new() else { return };
        let res = l.apply(&["user-me.md"]);
        assert_eq!(blocked_files(&res), ["user-me.md"]);
        assert!(!l.repo_mem.exists());
        assert!(l.personal.join("user-me.md").is_file());
    }

    #[test]
    fn test_refusals_write_nothing() {
        let Some(l) = L::new() else { return };
        let before = mem::snapshot(l.f.tmp());
        let digest = sha(&l.text);
        let deps = l.deps();
        let strings =
            |names: &[&str]| -> Vec<String> { names.iter().map(|s| s.to_string()).collect() };
        for (names, skip, needle) in [
            (&["nope.md"][..], false, "nope.md"),
            (&["../clean.md"][..], false, "../clean.md"),
            (&["clean.md"][..], true, "--skip-approve"),
        ] {
            let res = setup::apply(
                &l.f.c.plugin,
                &digest,
                Some(&l.root),
                skip,
                l.f.env(),
                &strings(names),
                &deps,
            );
            assert!(!ok(&res));
            assert!(text_at(&res, &["error"]).contains(needle));
            assert!(text_at(&res, &["error"]).contains("Nothing was written"));
        }
        let no_repo = |_: &Path, _: &Env| None;
        let no_repo_deps = SetupDeps {
            memory: MemoryDeps {
                repo_root: &no_repo,
                ..MemoryDeps::default()
            },
            ..l.f.deps()
        };
        let res = setup::apply(
            &l.f.c.plugin,
            &digest,
            Some(&l.root),
            false,
            l.f.env(),
            &strings(&["clean.md"]),
            &no_repo_deps,
        );
        assert!(!ok(&res));
        assert!(text_at(&res, &["error"]).contains("git repo"));
        assert_eq!(mem::snapshot(l.f.tmp()), before);
        assert_eq!(l.f.atl_calls(), 0);
        assert!(!l.f.approved(FAKE_REPO_KEY));
    }

    #[test]
    fn test_cli_promote_flag_is_repeatable() {
        let Some(l) = L::new() else { return };
        let argv: Vec<String> = [
            "apply",
            "--plugin-root",
            &p(&l.f.c.plugin),
            "--expect-sha256",
            &sha(&l.text),
            "--cwd",
            &p(&l.root),
            "--promote",
            "clean.md",
            "--promote",
            "mail.md",
        ]
        .iter()
        .map(|a| a.to_string())
        .collect();
        let (code, out, _) = setup::main(&argv, l.f.env(), &l.deps());
        let v = json::loads(&out).unwrap();
        assert_eq!(code, 0, "{out}");
        assert_eq!(strs_at(&v, &["memory", "migrated", "repo"]), ["clean.md"]);
        assert_eq!(blocked_files(&v), ["mail.md"]);
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
        assert_eq!(f.atl_calls(), 0);
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
    fn test_edit_during_the_atl_download_is_never_overwritten() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        let digest = f.digest();
        let edited = pretty(SONNET);
        let (path, e) = (f.c.settings.clone(), edited.clone());
        // The user edits while we "download".
        *f.atl.on_reconcile.borrow_mut() = Some(Box::new(move || fs::write(&path, &e).unwrap()));
        *f.atl.result.borrow_mut() = Reconciled {
            atl_path: None,
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
                assert!(diff.contains("Bash(atl *)"), "{name}");
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
        write(&f.c.data_dir.join("atl/1.0/atl"), "bin", None);
        let applied = f.read_settings();
        let before = mem::snapshot(f.tmp());
        let out = f.remove(false, false, true);
        assert_eq!(mem::snapshot(f.tmp()), before);
        assert!(ok(&out));
        assert_eq!(at(&out, &["preview"]), &Value::Bool(true));
        assert_eq!(at(&out, &["settings", "changed"]), &Value::Bool(true));
        let diff = text_at(&out, &["settings", "diff"]);
        assert!(diff.contains("-      \"Bash(atl *)\""), "{diff}");
        assert!(diff.contains("\"baseRef\""));
        assert_eq!(text_at(&out, &["atl", "dir"]), p(&f.c.data_dir.join("atl")));
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
    fn test_remove_preview_lists_the_approvals_each_flag_would_drop() {
        if !mem::needs_git() {
            return;
        }
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        let repo =
            f.c.mem
                .make_repo("repo", Some("https://example.invalid/acme/app.git"));
        f.save_approved(&["a/b"]);
        f.apply_in(&repo, false, &[], &f.deps());
        let out = f.remove_in(&repo, false, false, true);
        let list = |k: &str| -> Vec<String> {
            at(&out, &["memory", k])
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect()
        };
        assert_eq!(list("unapprove"), [FAKE_REPO_KEY]);
        assert_eq!(list("unapprove_all"), ["a/b", FAKE_REPO_KEY]);
        assert!(f.approved("a/b"));
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
            write(&data.join("atl/precious.txt"), "keep", None);
            state::write(f.env(), vec![("data_dir".into(), Value::Str(p(&data)))]).unwrap();
            let res = f.remove_env(&f.env_without_data(), false, false, false);
            assert!(data.join("atl/precious.txt").exists(), "{data:?}");
            assert!(
                dumped(at(&res, &["warnings"])).contains("atl"),
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
        let elsewhere = f.c.mem.config.join("elsewhere").join("atl");
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
    fn test_remove_keeps_repo_notes_and_gitattributes() {
        if !mem::needs_git() {
            return;
        }
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        let repo =
            f.c.mem
                .make_repo("repo", Some("https://example.invalid/acme/app.git"));
        let a = repo.join(".claude/memory/project-a.md");
        let b = repo.join(".claude/agent-memory/anthology-scout/project-b.md");
        write(&a, &note_text(&Note::new("project-a").desc("A")), None);
        write(&b, &note_text(&Note::new("project-b").desc("B")), None);
        f.apply_in(&repo, false, &[], &f.deps());
        f.remove_in(&repo, true, true, false);
        assert!(a.exists());
        assert!(b.exists());
        assert!(repo.join(".gitattributes").exists());
        assert!(!f.approved(FAKE_REPO_KEY));
    }

    #[test]
    fn test_unapprove_all_drops_every_approval() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        f.save_approved(&["a/b", "c/d"]);
        f.apply();
        f.remove(false, true, false);
        assert!(!f.approved("a/b"));
        assert!(!f.approved("c/d"));
    }

    #[test]
    fn test_remove_deletes_only_the_atl_folder_under_the_data_dir() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        f.apply();
        write(&f.c.data_dir.join("atl/1.0/atl"), "bin", None);
        write(&f.c.data_dir.join("other/keep.txt"), "keep", None);
        let res = f.remove(false, false, false);
        assert!(ok(&res), "{res:?}");
        assert!(!f.c.data_dir.join("atl").exists());
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
    fn test_remove_deletes_the_atl_history_folder() {
        // I4: the folder bin/atl and bin/atl-hook point ATL_DB_PATH, ATL_RECALL_DB and
        // ATL_TEE_DIR at.
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        f.apply();
        let history = state::atl_data_dir(f.env());
        write(&history.join("history.db"), "commands", None);
        write(&history.join("tee").join("1_x.log"), "out", None);
        let preview = f.remove(false, false, true);
        assert_eq!(
            at(&preview, &["atl_history"]),
            &pairs(&[("dir", &p(&history)), ("status", "delete")])
        );
        assert!(history.join("history.db").exists());
        let res = f.remove(false, false, false);
        assert!(ok(&res), "{res:?}");
        assert_eq!(at(&res, &["atl_history"]), &pairs(&[("status", "deleted")]));
        assert!(!history.exists());
    }

    #[test]
    fn test_remove_without_atl_history_reports_nothing_to_delete() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        f.apply();
        let preview = f.remove(false, false, true);
        assert_eq!(
            text_at(&preview, &["atl_history", "status"]),
            "nothing to delete"
        );
        assert_eq!(
            at(&f.remove(false, false, false), &["atl_history"]),
            &pairs(&[("status", "nothing to delete")])
        );
    }

    #[test]
    fn test_remove_does_not_follow_a_symlinked_atl_history_folder() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        f.apply();
        let elsewhere = f.c.mem.config.join("elsewhere-history");
        write(&elsewhere.join("precious.txt"), "keep", None);
        let history = state::atl_data_dir(f.env());
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
        write(&outside.join("atl/precious.txt"), "keep", None);
        state::write(f.env(), vec![("data_dir".into(), Value::Str(p(&outside)))]).unwrap();
        let res = f.remove_env(&f.env_without_data(), false, false, false);
        assert!(outside.join("atl/precious.txt").exists());
        assert!(dumped(at(&res, &["warnings"])).contains("atl"));
    }

    #[test]
    fn test_remove_does_not_follow_a_symlinked_atl_folder() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        f.apply();
        let elsewhere = f.c.mem.config.join("elsewhere");
        write(&elsewhere.join("precious.txt"), "keep", None);
        fs::create_dir_all(&f.c.data_dir).unwrap();
        symlink(&elsewhere, f.c.data_dir.join("atl")).unwrap();
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
            r#"{"model": "opus", "permissions": {"allow": ["Bash(atl *)", "Bash(ls)"]}}"#,
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
            ["Bash(atl *)", "Bash(ls)", "Bash(make *)"]
        );
        let got = json::loads(&f.read_settings()).unwrap();
        assert!(got.as_object().unwrap().get("worktree").is_none());
    }
}

mod subagent_model_test {
    //! `SubagentModelTest`: `CLAUDE_CODE_SUBAGENT_MODEL` is located in the preview, and removed
    //! from the user settings.json only when `apply --remove-subagent-model` says so.
    use super::*;

    const KEY: &str = "CLAUDE_CODE_SUBAGENT_MODEL";

    /// `self.with_model()`: the default settings with `env.CLAUDE_CODE_SUBAGENT_MODEL` set.
    fn with_model(f: &Fx) -> String {
        f.c.write_settings(
            r#"{"model": "opus", "env": {"A": "1", "CLAUDE_CODE_SUBAGENT_MODEL": "SECRETMODEL"}}"#,
        )
    }

    /// `json.loads(text)`.
    fn parsed(text: &str) -> Value {
        json::loads(text).expect("valid JSON")
    }

    /// Whether `text`'s `env` block (`json.loads`) has `KEY`.
    fn has_key(text: &str) -> bool {
        at(&parsed(text), &["env"])
            .as_object()
            .is_some_and(|m| m.contains_key(KEY))
    }

    /// `self.apply(cwd=cwd, remove_subagent_model=True, skip_approve=…)`: with the preview's
    /// digest.
    fn apply_removing(f: &Fx, cwd: &Path, skip_approve: bool) -> Value {
        let digest = text_at(&f.preview(Some(cwd)), &["settings", "sha256"]).to_string();
        setup::apply_with(
            &f.c.plugin,
            &digest,
            Some(cwd),
            skip_approve,
            f.env(),
            &[],
            true,
            &f.deps(),
        )
    }

    #[test]
    fn test_not_set() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        let out = f.preview(None);
        assert_eq!(
            at(&out, &["subagent_model"]),
            &parsed(r#"{"set_in": [], "removable": false, "diff": "", "elsewhere": []}"#)
        );
    }

    #[test]
    fn test_preview_offers_a_separate_redacted_removal_and_writes_nothing() {
        let f = Fx::new();
        with_model(&f);
        let before = mem::snapshot(f.tmp());
        let out = f.preview(None);
        assert_eq!(mem::snapshot(f.tmp()), before);
        let sm = at(&out, &["subagent_model"]);
        assert_eq!(at(sm, &["set_in"]), &parsed(r#"["user settings.json"]"#));
        assert_eq!(at(sm, &["removable"]), &Value::Bool(true));
        assert_eq!(at(sm, &["elsewhere"]), &parsed("[]"));
        let diff = text_at(sm, &["diff"]);
        assert!(
            diff.contains("-    \"CLAUDE_CODE_SUBAGENT_MODEL\": \"<redacted>\""),
            "{diff}"
        );
        assert!(!text_at(&out, &["settings", "diff"]).contains(KEY));
        assert!(!dumped(&out).contains("SECRETMODEL"));
    }

    #[test]
    fn test_apply_removes_it_only_when_asked_and_remove_restores_it() {
        let f = Fx::new();
        let text = with_model(&f);
        assert!(ok(&f.apply()));
        assert!(has_key(&f.read_settings()));
        let before = f.state();
        let res = apply_removing(&f, f.tmp(), false);
        assert!(ok(&res), "{}", dumped(&res));
        assert_eq!(
            at(&res, &["subagent_model"]),
            &parsed(r#"{"removed": true}"#)
        );
        assert!(!dumped(&res).contains("SECRETMODEL"));
        assert_eq!(
            at(&parsed(&f.read_settings()), &["env"]),
            &parsed(r#"{"A": "1"}"#)
        );
        let after = f.state();
        assert_eq!(
            after.get("settings_ops_version"),
            before.get("settings_ops_version")
        ); // the opt-in is not an op change
        let ops: Vec<&str> = after
            .get("settings_ops")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|o| o.as_object()?.get("op")?.as_str())
            .collect();
        assert!(ops.contains(&"unset"), "{ops:?}");
        assert!(ok(&f.apply())); // a later run keeps the record
        let preview = f.remove(false, false, true);
        assert!(
            text_at(&preview, &["settings", "diff"])
                .contains("+    \"CLAUDE_CODE_SUBAGENT_MODEL\": \"<redacted>\""),
        );
        assert!(!dumped(&preview).contains("SECRETMODEL"));
        assert!(ok(&f.remove(false, false, false)));
        assert_eq!(f.read_settings(), text);
    }

    #[test]
    fn test_remove_keeps_a_value_set_again_since() {
        let f = Fx::new();
        with_model(&f);
        apply_removing(&f, f.tmp(), false);
        let mut data = parsed(&f.read_settings());
        data.as_object_mut()
            .unwrap()
            .get_mut("env")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert(KEY.into(), Value::Str("mine-now".into()));
        write(
            &f.c.settings,
            &(json::dumps(&data, &json::INDENT2) + "\n"),
            None,
        );
        assert!(ok(&f.remove(false, false, false)));
        let data = parsed(&f.read_settings());
        assert_eq!(text_at(&data, &["env", KEY]), "mine-now");
        assert!(data.as_object().unwrap().get("permissions").is_none()); // setup's own entries came out
    }

    #[test]
    fn test_apply_with_the_flag_is_digest_protected() {
        let f = Fx::new();
        with_model(&f);
        let digest = f.digest();
        let edited = format!("{}\n", f.read_settings());
        write(&f.c.settings, &edited, None);
        let res = setup::apply_with(
            &f.c.plugin,
            &digest,
            Some(f.tmp()),
            false,
            f.env(),
            &[],
            true,
            &f.deps(),
        );
        assert!(!ok(&res));
        assert!(has_key(&f.read_settings()));
    }

    #[test]
    fn test_elsewhere_is_never_touched_and_says_how() {
        let mut f = Fx::new();
        let text = f.c.write_settings(SETTINGS);
        let proj = f.tmp().join("proj");
        write(
            &proj.join(".claude").join("settings.local.json"),
            r#"{"env": {"CLAUDE_CODE_SUBAGENT_MODEL": "m"}}"#,
            None,
        );
        f.c.env = f.c.env.clone().with(KEY, "from-the-shell");
        let out = f.preview(Some(&proj));
        let sm = at(&out, &["subagent_model"]);
        assert_eq!(
            at(sm, &["set_in"]),
            &parsed(r#"["the project's .claude/settings.local.json", "the process environment"]"#)
        );
        assert_eq!(at(sm, &["removable"]), &Value::Bool(false));
        assert_eq!(text_at(sm, &["diff"]), "");
        let elsewhere = at(sm, &["elsewhere"]).as_array().unwrap();
        let wheres: Vec<&str> = elsewhere.iter().map(|e| text_at(e, &["where"])).collect();
        assert_eq!(
            wheres,
            [
                "the project's .claude/settings.local.json",
                "the process environment"
            ]
        );
        for e in elsewhere {
            assert!(text_at(e, &["how"]).contains(KEY));
        }
        assert!(text_at(&elsewhere[1], &["how"]).contains("shell profile"));
        let res = apply_removing(&f, &proj, true);
        assert_eq!(
            at(&res, &["subagent_model"]),
            &parsed(r#"{"removed": false}"#)
        );
        assert_eq!(
            parsed(&fs::read_to_string(proj.join(".claude").join("settings.local.json")).unwrap()),
            parsed(r#"{"env": {"CLAUDE_CODE_SUBAGENT_MODEL": "m"}}"#)
        );
        f.remove_in(&proj, false, false, false);
        assert_eq!(f.read_settings(), text);
    }

    #[test]
    fn test_a_key_set_twice_is_not_offered() {
        let f = Fx::new();
        let text =
            r#"{"env": {"CLAUDE_CODE_SUBAGENT_MODEL": "a", "CLAUDE_CODE_SUBAGENT_MODEL": "b"}}"#;
        write(&f.c.settings, &format!("{text}\n"), None);
        let out = f.preview(None);
        let sm = at(&out, &["subagent_model"]);
        assert_eq!(at(sm, &["set_in"]), &parsed(r#"["user settings.json"]"#));
        assert_eq!(at(sm, &["removable"]), &Value::Bool(false));
        let elsewhere = at(sm, &["elsewhere"]).as_array().unwrap();
        assert_eq!(elsewhere.len(), 1);
        assert_eq!(text_at(&elsewhere[0], &["where"]), "user settings.json");
        assert!(text_at(&elsewhere[0], &["how"]).contains("every"));
        let res = apply_removing(&f, f.tmp(), true);
        assert_eq!(
            at(&res, &["subagent_model"]),
            &parsed(r#"{"removed": false}"#)
        );
        assert_eq!(
            at(&parsed(&f.read_settings()), &["env"]),
            &parsed(r#"{"CLAUDE_CODE_SUBAGENT_MODEL": "b"}"#)
        );
    }

    #[test]
    fn test_the_process_copy_of_the_user_setting_is_not_a_second_place() {
        let mut f = Fx::new();
        with_model(&f);
        // Claude Code copies settings env into the commands it runs
        f.c.env = f.c.env.clone().with(KEY, "SECRETMODEL");
        let out = f.preview(None);
        let sm = at(&out, &["subagent_model"]);
        assert_eq!(at(sm, &["set_in"]), &parsed(r#"["user settings.json"]"#));
        assert_eq!(at(sm, &["removable"]), &Value::Bool(true));
    }

    #[test]
    fn test_unparseable_user_settings_offer_nothing() {
        let f = Fx::new();
        write(
            &f.c.settings,
            r#"{"env": {"CLAUDE_CODE_SUBAGENT_MODEL": "m"},}"#,
            None,
        );
        let out = f.preview(None);
        assert!(
            at(&out, &["settings"])
                .as_object()
                .unwrap()
                .contains_key("error")
        );
        let sm = at(&out, &["subagent_model"]);
        assert_eq!(at(sm, &["removable"]), &Value::Bool(false));
        assert_eq!(at(sm, &["set_in"]), &parsed("[]")); // not valid JSON for Claude Code either
    }

    #[test]
    fn test_cli_flag() {
        let f = Fx::new();
        with_model(&f);
        let digest = f.digest();
        let argv: Vec<String> = [
            "apply",
            "--plugin-root",
            &p(&f.c.plugin),
            "--expect-sha256",
            &digest,
            "--cwd",
            &p(f.tmp()),
            "--skip-approve",
            "--remove-subagent-model",
        ]
        .iter()
        .map(|a| a.to_string())
        .collect();
        let (code, out, _) = setup::main(&argv, f.env(), &f.deps());
        assert_eq!(code, 0, "{out}");
        let v = json::loads(&out).unwrap();
        assert_eq!(at(&v, &["subagent_model", "removed"]), &Value::Bool(true));
        assert_eq!(
            at(&parsed(&f.read_settings()), &["env"]),
            &parsed(r#"{"A": "1"}"#)
        );
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
                ..MemoryDeps::default()
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

mod repo_auto_memory_test {
    //! A repo approved for Anthology memory gets `"autoMemoryEnabled": false` in its committed
    //! `.claude/settings.json`, so Claude Code's own auto memory does not run beside the repo's
    //! `.claude/memory/`. Setup never touches the user-wide settings for this, and `remove` puts
    //! the repo file back as it was.
    use super::*;

    const REMOTE: &str = "https://example.invalid/acme/app.git";
    const REPO_SETTINGS: &str = "{\n  \"model\": \"sonnet\",\n  \"permissions\": {\n    \"allow\": [\n      \"Bash(ls)\"\n    ]\n  }\n}\n";

    /// A fixture with user settings written and an approvable repo; `None` without git.
    fn repo_fx() -> Option<(Fx, PathBuf)> {
        if !mem::needs_git() {
            return None;
        }
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        let repo = f.c.mem.make_repo("repo", Some(REMOTE));
        Some((f, repo))
    }

    fn repo_settings(repo: &Path) -> PathBuf {
        repo.join(".claude/settings.json")
    }

    fn auto_memory_value(repo: &Path) -> Option<Value> {
        let text = fs::read_to_string(repo_settings(repo)).ok()?;
        json::loads(&text)
            .ok()?
            .as_object()?
            .get("autoMemoryEnabled")
            .cloned()
    }

    #[test]
    fn the_setting_is_written_for_an_approved_repo() {
        let Some((f, repo)) = repo_fx() else { return };
        let res = f.apply_in(&repo, false, &[], &f.deps());
        assert!(ok(&res), "{res:?}");
        assert!(f.approved(FAKE_REPO_KEY));
        assert_eq!(auto_memory_value(&repo), Some(Value::Bool(false)));
        let result = at(&res, &["memory", "auto_memory"]);
        assert_eq!(text_at(result, &["path"]), p(&repo_settings(&repo)));
        assert_eq!(at(result, &["changed"]), &Value::Bool(true));
        assert!(text_at(result, &["diff"]).contains("autoMemoryEnabled"));
    }

    #[test]
    fn the_setting_is_not_written_unless_the_repo_is_approved() {
        let Some((f, repo)) = repo_fx() else { return };
        // --skip-approve: the repo is not approved, so its settings stay untouched
        let res = f.apply_in(&repo, true, &[], &f.deps());
        assert!(ok(&res), "{res:?}");
        assert!(!f.approved(FAKE_REPO_KEY));
        assert!(!repo.join(".claude").exists());
        assert_eq!(at(&res, &["memory", "auto_memory"]), &Value::Null);
        // not a git repo: there is nothing to approve
        let res = f.apply_in(f.tmp(), false, &[], &f.deps());
        assert!(ok(&res), "{res:?}");
        assert!(!f.tmp().join(".claude").exists());
        assert_eq!(at(&res, &["memory", "auto_memory"]), &Value::Null);
    }

    #[test]
    fn the_setting_is_not_written_when_repo_memory_is_unusable() {
        let Some((f, repo)) = repo_fx() else { return };
        fs::write(repo.join("elsewhere"), "x").unwrap();
        symlink(repo.join("elsewhere"), repo.join(".gitattributes")).unwrap();
        let res = f.apply_in(&repo, false, &[], &f.deps());
        assert!(ok(&res), "{res:?}");
        assert!(f.approved(FAKE_REPO_KEY));
        assert!(text_at(&res, &["memory", "approve_error"]).contains(".gitattributes"));
        assert!(!repo_settings(&repo).exists());
        assert!(text_at(&res, &["memory", "auto_memory", "skipped"]).contains("repo memory"));
    }

    #[test]
    fn preview_shows_the_change_and_writes_nothing() {
        let Some((f, repo)) = repo_fx() else { return };
        let before = mem::snapshot(f.tmp());
        let out = f.preview(Some(&repo));
        assert_eq!(mem::snapshot(f.tmp()), before);
        let auto = at(&out, &["memory", "repo", "auto_memory"]);
        assert_eq!(text_at(auto, &["path"]), p(&repo_settings(&repo)));
        assert_eq!(at(auto, &["changed"]), &Value::Bool(true));
        let diff = text_at(auto, &["diff"]);
        assert!(diff.contains("autoMemoryEnabled"), "{diff}");
        // the value is redacted like every value outside the shown paths
        assert!(!diff.contains("false"), "{diff}");
        // once applied, the preview has nothing more to show
        f.apply_in(&repo, false, &[], &f.deps());
        let out = f.preview(Some(&repo));
        let auto = at(&out, &["memory", "repo", "auto_memory"]);
        assert_eq!(at(auto, &["changed"]), &Value::Bool(false));
        assert_eq!(text_at(auto, &["diff"]), "");
    }

    #[test]
    fn preview_outside_a_repo_has_no_repo_setting() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        let out = f.preview(None);
        assert_eq!(
            text_at(&out, &["memory", "repo", "skipped"]),
            "not a git repo"
        );
    }

    #[test]
    fn an_existing_settings_json_is_merged_not_overwritten() {
        let Some((f, repo)) = repo_fx() else { return };
        write(&repo_settings(&repo), REPO_SETTINGS, None);
        let res = f.apply_in(&repo, false, &[], &f.deps());
        assert!(ok(&res), "{res:?}");
        let text = fs::read_to_string(repo_settings(&repo)).unwrap();
        let v = json::loads(&text).unwrap();
        assert_eq!(text_at(&v, &["model"]), "sonnet");
        assert_eq!(
            at(&v, &["permissions", "allow"]).as_array().unwrap().len(),
            1
        );
        assert_eq!(at(&v, &["autoMemoryEnabled"]), &Value::Bool(false));
        let diff = text_at(at(&res, &["memory", "auto_memory"]), &["diff"]);
        assert!(!diff.contains("sonnet"), "{diff}");
    }

    #[test]
    fn apply_leaves_the_user_wide_settings_alone() {
        let Some((f, repo)) = repo_fx() else { return };
        f.apply_in(&repo, false, &[], &f.deps());
        let user = f.read_settings();
        assert!(!user.contains("autoMemoryEnabled"), "{user}");
    }

    #[test]
    fn remove_deletes_a_file_setup_created() {
        let Some((f, repo)) = repo_fx() else { return };
        f.apply_in(&repo, false, &[], &f.deps());
        assert!(repo_settings(&repo).is_file());
        let res = f.remove_in(&repo, false, false, false);
        assert!(ok(&res), "{res:?}");
        assert!(!repo_settings(&repo).exists());
        let entries = at(&res, &["memory", "repo_settings"]).as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(at(&entries[0], &["changed"]), &Value::Bool(true));
        assert!(f.approved(FAKE_REPO_KEY)); // remove keeps approvals unless asked
    }

    #[test]
    fn remove_restores_an_existing_file_byte_for_byte() {
        let Some((f, repo)) = repo_fx() else { return };
        write(&repo_settings(&repo), REPO_SETTINGS, None);
        f.apply_in(&repo, false, &[], &f.deps());
        assert_ne!(
            fs::read_to_string(repo_settings(&repo)).unwrap(),
            REPO_SETTINGS
        );
        f.remove_in(&repo, false, false, false);
        assert_eq!(
            fs::read_to_string(repo_settings(&repo)).unwrap(),
            REPO_SETTINGS
        );
    }

    #[test]
    fn remove_preview_shows_the_diff_and_writes_nothing() {
        let Some((f, repo)) = repo_fx() else { return };
        write(&repo_settings(&repo), REPO_SETTINGS, None);
        f.apply_in(&repo, false, &[], &f.deps());
        let before = mem::snapshot(f.tmp());
        let res = f.remove_in(&repo, false, false, true);
        assert_eq!(mem::snapshot(f.tmp()), before);
        let entries = at(&res, &["memory", "repo_settings"]).as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(text_at(&entries[0], &["path"]), p(&repo_settings(&repo)));
        assert_eq!(at(&entries[0], &["changed"]), &Value::Bool(true));
        assert!(text_at(&entries[0], &["diff"]).contains("autoMemoryEnabled"));
    }

    #[test]
    fn remove_puts_back_a_value_the_user_had() {
        let Some((f, repo)) = repo_fx() else { return };
        let before = "{\n  \"autoMemoryEnabled\": true\n}\n";
        write(&repo_settings(&repo), before, None);
        f.apply_in(&repo, false, &[], &f.deps());
        assert_eq!(auto_memory_value(&repo), Some(Value::Bool(false)));
        f.remove_in(&repo, false, false, false);
        assert_eq!(fs::read_to_string(repo_settings(&repo)).unwrap(), before);
    }

    #[test]
    fn a_setting_the_user_already_had_is_not_ours_to_remove() {
        let Some((f, repo)) = repo_fx() else { return };
        let before = "{\n  \"autoMemoryEnabled\": false\n}\n";
        write(&repo_settings(&repo), before, None);
        let res = f.apply_in(&repo, false, &[], &f.deps());
        assert_eq!(
            at(at(&res, &["memory", "auto_memory"]), &["changed"]),
            &Value::Bool(false)
        );
        f.remove_in(&repo, false, false, false);
        assert_eq!(fs::read_to_string(repo_settings(&repo)).unwrap(), before);
    }

    #[test]
    fn applying_twice_then_removing_still_restores_the_file() {
        let Some((f, repo)) = repo_fx() else { return };
        write(&repo_settings(&repo), REPO_SETTINGS, None);
        f.apply_in(&repo, false, &[], &f.deps());
        f.apply_in(&repo, false, &[], &f.deps());
        f.remove_in(&repo, false, false, false);
        assert_eq!(
            fs::read_to_string(repo_settings(&repo)).unwrap(),
            REPO_SETTINGS
        );
    }

    #[test]
    fn remove_keeps_what_the_user_added_to_the_file_since() {
        let Some((f, repo)) = repo_fx() else { return };
        f.apply_in(&repo, false, &[], &f.deps());
        let text = fs::read_to_string(repo_settings(&repo)).unwrap();
        let edited = text.replace("{\n", "{\n  \"model\": \"haiku\",\n");
        write(&repo_settings(&repo), &edited, None);
        f.remove_in(&repo, false, false, false);
        let v = json::loads(&fs::read_to_string(repo_settings(&repo)).unwrap()).unwrap();
        assert_eq!(text_at(&v, &["model"]), "haiku");
        assert!(!v.as_object().unwrap().contains_key("autoMemoryEnabled"));
    }

    #[test]
    fn a_settings_file_that_is_not_json_is_left_untouched_and_reported() {
        let Some((f, repo)) = repo_fx() else { return };
        write(&repo_settings(&repo), "{not json", None);
        let preview = f.preview(Some(&repo));
        assert!(!text_at(&preview, &["memory", "repo", "auto_memory", "skipped"]).is_empty());
        let res = f.apply_in(&repo, false, &[], &f.deps());
        assert!(ok(&res), "{res:?}");
        assert!(f.approved(FAKE_REPO_KEY));
        assert!(!text_at(&res, &["memory", "auto_memory", "skipped"]).is_empty());
        assert_eq!(
            fs::read_to_string(repo_settings(&repo)).unwrap(),
            "{not json"
        );
    }

    #[test]
    fn a_symlinked_settings_file_is_never_written_through() {
        let Some((f, repo)) = repo_fx() else { return };
        fs::create_dir_all(repo.join(".claude")).unwrap();
        let target = f.tmp().join("outside.json");
        fs::write(&target, "{}\n").unwrap();
        symlink(&target, repo_settings(&repo)).unwrap();
        let res = f.apply_in(&repo, false, &[], &f.deps());
        assert!(ok(&res), "{res:?}");
        assert!(text_at(&res, &["memory", "auto_memory", "skipped"]).contains("symlink"));
        assert_eq!(fs::read_to_string(&target).unwrap(), "{}\n");
    }

    /// A valid settings file bigger than the cap.
    fn oversized() -> String {
        format!("{{\"padding\": \"{}\"}}\n", "x".repeat(2 * 1024 * 1024))
    }

    #[test]
    fn an_oversized_settings_file_is_refused_in_preview_and_apply() {
        let Some((f, repo)) = repo_fx() else { return };
        let big = oversized();
        write(&repo_settings(&repo), &big, None);
        let preview = f.preview(Some(&repo));
        let skipped = text_at(&preview, &["memory", "repo", "auto_memory", "skipped"]);
        assert!(skipped.contains("too large"), "{skipped}");
        let res = f.apply_in(&repo, false, &[], &f.deps());
        assert!(ok(&res), "{res:?}");
        assert!(f.approved(FAKE_REPO_KEY));
        assert!(text_at(&res, &["memory", "auto_memory", "skipped"]).contains("too large"));
        assert_eq!(fs::read_to_string(repo_settings(&repo)).unwrap(), big);
    }

    #[test]
    fn remove_leaves_a_file_that_grew_past_the_cap_and_says_so() {
        let Some((f, repo)) = repo_fx() else { return };
        f.apply_in(&repo, false, &[], &f.deps());
        let big = oversized();
        write(&repo_settings(&repo), &big, None);
        let res = f.remove_in(&repo, false, false, false);
        assert!(ok(&res), "{res:?}");
        let entries = at(&res, &["memory", "repo_settings"]).as_array().unwrap();
        assert!(text_at(&entries[0], &["note"]).contains("too large"));
        assert_eq!(at(&entries[0], &["changed"]), &Value::Bool(false));
        assert_eq!(fs::read_to_string(repo_settings(&repo)).unwrap(), big);
        assert!(
            at(&res, &["warnings"])
                .as_array()
                .unwrap()
                .iter()
                .any(|w| w.as_str().unwrap().contains("too large"))
        );
    }

    #[test]
    fn remove_skips_a_repo_that_is_gone() {
        let Some((f, repo)) = repo_fx() else { return };
        f.apply_in(&repo, false, &[], &f.deps());
        fs::remove_dir_all(repo.join(".claude")).unwrap();
        let res = f.remove_in(f.tmp(), false, false, false);
        assert!(ok(&res), "{res:?}");
        assert!(!f.state_exists());
    }
}

mod old_notes_by_type_test {
    //! The notes Claude Code's auto memory saved for a repo before Anthology are moved, after the
    //! user confirms (`--move-old-notes`): project, feedback and reference notes into the repo's
    //! `.claude/memory/` as `<type>-<slug>.md`, user notes into personal memory. The privacy scan
    //! decides what may go into the repo; a refused note is listed and moved nowhere. The
    //! originals stay where they are.
    use super::*;

    fn alice_laptop() -> Option<String> {
        Some("alice-laptop".into())
    }

    struct N {
        f: Fx,
        root: PathBuf,
        old: PathBuf,
        repo_mem: PathBuf,
        personal: PathBuf,
    }

    impl N {
        /// `None` without git.
        fn new() -> Option<N> {
            if !mem::needs_git() {
                return None;
            }
            let f = Fx::new();
            f.c.write_settings(SETTINGS);
            let root =
                f.c.mem
                    .make_repo("repo", Some("https://example.invalid/acme/app.git"));
            let encoded: String = p(&root)
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                .collect();
            let old = f.c.mem.config.join("projects").join(encoded).join("memory");
            let note = |n: Note| note_text(&n);
            write(
                &old.join("project_release.md"),
                &note(Note::new("project_release").desc("Release plan")),
                None,
            );
            write(
                &old.join("feedback_testing.md"),
                &note(
                    Note::new("feedback_testing")
                        .type_("feedback")
                        .desc("How to test"),
                ),
                None,
            );
            write(
                &old.join("reference_dashboards.md"),
                &note(
                    Note::new("reference_dashboards")
                        .type_("reference")
                        .desc("Where the dashboards are"),
                ),
                None,
            );
            write(
                &old.join("topics.md"),
                &note(Note::new("topics").desc("No type prefix in the file name")),
                None,
            );
            write(
                &old.join("user_role.md"),
                &note(Note::new("user_role").type_("user").desc("About me")),
                None,
            );
            write(
                &old.join("leaky.md"),
                &note(
                    Note::new("leaky")
                        .type_("feedback")
                        .desc("Has a home path")
                        .body(&("Checkout in /".to_string() + "Users/alice/src\n")),
                ),
                None,
            );
            write(
                &old.join("untyped.md"),
                "Just some text with no frontmatter.\n",
                None,
            );
            write(&old.join("MEMORY.md"), "- index\n", None);
            Some(N {
                repo_mem: root.join(".claude/memory"),
                personal: f.anth().join("personal-memory"),
                f,
                root,
                old,
            })
        }

        fn deps(&self) -> SetupDeps<'_> {
            SetupDeps {
                memory: MemoryDeps {
                    hostname: Some(alice_laptop),
                    ..MemoryDeps::default()
                },
                ..self.f.deps()
            }
        }

        fn digest(&self) -> String {
            let out = setup::preview(
                &self.f.c.plugin,
                Some(&self.root),
                self.f.env(),
                &self.deps(),
            );
            text_at(&out, &["settings", "sha256"]).to_string()
        }

        fn apply_with(&self, cwd: &Path, skip: bool, promote: &[String], mv: bool) -> Value {
            setup::apply_full(
                &self.f.c.plugin,
                &self.digest(),
                Some(cwd),
                skip,
                self.f.env(),
                promote,
                false,
                mv,
                &self.deps(),
            )
        }

        /// `apply` with `--move-old-notes`.
        fn apply(&self) -> Value {
            self.apply_with(&self.root, false, &[], true)
        }

        fn notes(&self) -> Vec<(String, Value)> {
            let out = setup::preview(
                &self.f.c.plugin,
                Some(&self.root),
                self.f.env(),
                &self.deps(),
            );
            at(&out, &["memory", "legacy_notes"])
                .as_array()
                .unwrap()
                .iter()
                .map(|n| (text_at(n, &["file"]).to_string(), n.clone()))
                .collect()
        }
    }

    fn files(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    fn strs_at(res: &Value, path: &[&str]) -> Vec<String> {
        at(res, path)
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect()
    }

    fn blocked_files(res: &Value) -> Vec<String> {
        at(res, &["memory", "blocked"])
            .as_array()
            .unwrap()
            .iter()
            .map(|b| text_at(b, &["file"]).to_string())
            .collect()
    }

    #[test]
    fn preview_names_where_each_note_would_go() {
        let Some(n) = N::new() else { return };
        let before = mem::snapshot(n.f.tmp());
        let notes = n.notes();
        assert_eq!(mem::snapshot(n.f.tmp()), before);
        let get = |file: &str| &notes.iter().find(|(f, _)| f == file).unwrap().1;
        for (file, destination, target) in [
            ("project_release.md", "repo", Some("project-release.md")),
            ("feedback_testing.md", "repo", Some("feedback-testing.md")),
            (
                "reference_dashboards.md",
                "repo",
                Some("reference-dashboards.md"),
            ),
            ("topics.md", "repo", Some("project-topics.md")),
            ("user_role.md", "personal", Some("user-role.md")),
            ("leaky.md", "none", None),
            ("untyped.md", "none", None),
        ] {
            assert_eq!(text_at(get(file), &["destination"]), destination, "{file}");
            match target {
                Some(t) => assert_eq!(text_at(get(file), &["target"]), t, "{file}"),
                None => assert_eq!(at(get(file), &["target"]), &Value::Null, "{file}"),
            }
        }
        assert!(text_at(get("leaky.md"), &["reason"]).contains("home path"));
        assert!(text_at(get("untyped.md"), &["reason"]).contains("type"));
        // what happens without the confirmation is unchanged
        assert_eq!(text_at(get("project_release.md"), &["default"]), "personal");
    }

    #[test]
    fn each_type_goes_to_the_right_place_and_the_originals_stay() {
        let Some(n) = N::new() else { return };
        let before = mem::snapshot(&n.old);
        let res = n.apply();
        assert!(ok(&res), "{res:?}");
        assert_eq!(
            files(&n.repo_mem),
            [
                "MEMORY.md",
                "feedback-testing.md",
                "project-release.md",
                "project-topics.md",
                "reference-dashboards.md"
            ]
        );
        for (old, new) in [
            ("project_release.md", "project-release.md"),
            ("feedback_testing.md", "feedback-testing.md"),
            ("reference_dashboards.md", "reference-dashboards.md"),
            ("topics.md", "project-topics.md"),
        ] {
            assert_eq!(
                fs::read(n.repo_mem.join(new)).unwrap(),
                fs::read(n.old.join(old)).unwrap(),
                "{new}"
            );
        }
        assert_eq!(
            fs::read(n.personal.join("user-role.md")).unwrap(),
            fs::read(n.old.join("user_role.md")).unwrap()
        );
        let mut personal = files(&n.personal);
        personal.retain(|f| f != "MEMORY.md");
        assert_eq!(personal, ["user-role.md"]);
        assert!(
            fs::read_to_string(n.repo_mem.join("MEMORY.md"))
                .unwrap()
                .contains("project-release.md")
        );
        // in the order of the old file names
        assert_eq!(
            strs_at(&res, &["memory", "migrated", "repo"]),
            [
                "feedback-testing.md",
                "project-release.md",
                "reference-dashboards.md",
                "project-topics.md"
            ]
        );
        assert_eq!(
            strs_at(&res, &["memory", "migrated", "personal"]),
            ["user-role.md"]
        );
        // nothing is deleted or changed in the old folder
        assert_eq!(mem::snapshot(&n.old), before);
        let originals = text_at(&res, &["memory", "originals"]);
        assert!(originals.contains("left in place"), "{originals}");
        assert!(originals.contains("no longer loaded"), "{originals}");
    }

    #[test]
    fn a_note_the_privacy_scan_refuses_is_listed_and_not_moved() {
        let Some(n) = N::new() else { return };
        let res = n.apply();
        let mut blocked = blocked_files(&res);
        blocked.sort();
        assert_eq!(blocked, ["leaky.md", "untyped.md"]);
        let leaky = &at(&res, &["memory", "blocked"]).as_array().unwrap()[0];
        assert_eq!(text_at(leaky, &["file"]), "leaky.md");
        assert!(text_at(leaky, &["reason"]).contains("home path"));
        assert!(!dumped(at(&res, &["memory", "blocked"])).contains("alice"));
        for dir in [&n.repo_mem, &n.personal] {
            let names = files(dir);
            assert!(
                !names
                    .iter()
                    .any(|f| f.contains("leaky") || f.contains("untyped")),
                "{names:?}"
            );
        }
        assert!(n.old.join("leaky.md").is_file());
        assert!(n.old.join("untyped.md").is_file());
    }

    #[test]
    fn preview_status_is_for_the_file_the_move_would_write() {
        let Some(n) = N::new() else { return };
        write(
            &n.repo_mem.join("project-release.md"),
            "the repo's own note\n",
            None,
        );
        write(
            &n.repo_mem.join("feedback-testing.md"),
            &fs::read_to_string(n.old.join("feedback_testing.md")).unwrap(),
            None,
        );
        let notes = n.notes();
        let get = |file: &str| &notes.iter().find(|(f, _)| f == file).unwrap().1;
        let release = get("project_release.md");
        assert_eq!(text_at(release, &["target_status"]), "conflict");
        let would = text_at(release, &["target_would_create"]);
        assert!(would.starts_with("project-release.migrated-"), "{would}");
        assert_eq!(
            text_at(get("feedback_testing.md"), &["target_status"]),
            "identical"
        );
        assert_eq!(
            text_at(get("reference_dashboards.md"), &["target_status"]),
            "new"
        );
        assert_eq!(text_at(get("user_role.md"), &["target_status"]), "new");
        assert_eq!(at(get("leaky.md"), &["target_status"]), &Value::Null);
    }

    #[test]
    fn a_note_holding_a_secret_env_value_is_listed_and_not_moved() {
        let Some(n) = N::new() else { return };
        let value = "k3j9x0q8w7e6r5t4";
        write(
            &n.old.join("reference_ci.md"),
            &note_text(
                &Note::new("reference_ci")
                    .type_("reference")
                    .desc("CI login")
                    .body(&format!("The runner logs in with {value}.\n")),
            ),
            None,
        );
        let env = n.f.env().clone().with("CI_TOKEN", value);
        let deps = n.deps();
        let preview = setup::preview(&n.f.c.plugin, Some(&n.root), &env, &deps);
        let digest = text_at(&preview, &["settings", "sha256"]).to_string();
        let notes = at(&preview, &["memory", "legacy_notes"])
            .as_array()
            .unwrap();
        let ci = notes
            .iter()
            .find(|v| text_at(v, &["file"]) == "reference_ci.md")
            .unwrap();
        assert_eq!(text_at(ci, &["destination"]), "none");
        assert!(text_at(ci, &["reason"]).contains("$CI_TOKEN"));
        let res = setup::apply_full(
            &n.f.c.plugin,
            &digest,
            Some(&n.root),
            false,
            &env,
            &[],
            false,
            true,
            &deps,
        );
        assert!(ok(&res), "{res:?}");
        assert!(blocked_files(&res).contains(&"reference_ci.md".to_string()));
        assert!(!files(&n.repo_mem).iter().any(|f| f.contains("-ci")));
        assert!(!dumped(&res).contains(value));
    }

    #[test]
    fn an_existing_repo_note_is_never_overwritten() {
        let Some(n) = N::new() else { return };
        write(
            &n.repo_mem.join("project-release.md"),
            "the repo's own note\n",
            None,
        );
        let res = n.apply();
        assert!(ok(&res), "{res:?}");
        assert_eq!(
            fs::read_to_string(n.repo_mem.join("project-release.md")).unwrap(),
            "the repo's own note\n"
        );
        let side: Vec<String> = files(&n.repo_mem)
            .into_iter()
            .filter(|f| f.starts_with("project-release.migrated-"))
            .collect();
        assert_eq!(side.len(), 1, "{side:?}");
        assert_eq!(
            fs::read(n.repo_mem.join(&side[0])).unwrap(),
            fs::read(n.old.join("project_release.md")).unwrap()
        );
    }

    #[test]
    fn moving_twice_adds_nothing_the_second_time() {
        let Some(n) = N::new() else { return };
        n.apply();
        let repo_before = mem::snapshot(&n.repo_mem);
        let personal_before = mem::snapshot(&n.personal);
        let res = n.apply();
        assert!(ok(&res), "{res:?}");
        assert_eq!(mem::snapshot(&n.repo_mem), repo_before);
        assert_eq!(mem::snapshot(&n.personal), personal_before);
    }

    #[test]
    fn session_start_does_not_copy_moved_notes_into_personal_memory() {
        let Some(n) = N::new() else { return };
        n.apply();
        let mut sd = SyncDeps::from_env(n.f.env());
        sd.privacy.hostname = alice_laptop;
        sync::run(&n.root, n.f.env(), &sd);
        let personal = files(&n.personal);
        assert!(
            !personal
                .iter()
                .any(|f| f.contains("release") || f.contains("testing")),
            "{personal:?}"
        );
    }

    #[test]
    fn nothing_moves_without_the_confirmation() {
        let Some(n) = N::new() else { return };
        let res = n.apply_with(&n.root, false, &[], false);
        assert!(ok(&res), "{res:?}");
        assert!(!n.repo_mem.exists());
    }

    #[test]
    fn the_move_is_refused_where_it_cannot_be_honoured_and_writes_nothing() {
        let Some(n) = N::new() else { return };
        let before = mem::snapshot(n.f.tmp());
        let one = vec!["topics.md".to_string()];
        for (cwd, skip, promote, needle) in [
            (n.root.clone(), true, vec![], "--skip-approve"),
            (n.root.clone(), false, one, "--promote"),
            (n.f.tmp().to_path_buf(), false, vec![], "git repo"),
        ] {
            let res = n.apply_with(&cwd, skip, &promote, true);
            assert!(!ok(&res), "{needle}: {res:?}");
            let error = text_at(&res, &["error"]);
            assert!(error.contains(needle), "{error}");
            assert!(error.contains("Nothing was written"), "{error}");
        }
        assert_eq!(mem::snapshot(n.f.tmp()), before);
        assert_eq!(n.f.atl_calls(), 0);
    }

    #[test]
    fn the_cli_flag_reaches_apply() {
        let Some(n) = N::new() else { return };
        let argv: Vec<String> = [
            "apply",
            "--plugin-root",
            &p(&n.f.c.plugin),
            "--expect-sha256",
            &n.digest(),
            "--cwd",
            &p(&n.root),
            "--move-old-notes",
        ]
        .iter()
        .map(|a| a.to_string())
        .collect();
        let (code, out, _) = setup::main(&argv, n.f.env(), &n.deps());
        assert_eq!(code, 0, "{out}");
        assert!(n.repo_mem.join("project-release.md").is_file());
    }
}

/// The rename from the legacy rtk to ATL: what setup does about an install made before it (the plan's
/// upgrade path, item 4). Every old name below is a legacy one.
mod legacy_cleanup_test {
    use super::*;

    const LEGACY_DIR: &str = "rtk"; // legacy
    const LEGACY_KEY: &str = "rtk_path"; // legacy

    /// What a pre-rename install left: the downloaded folder, its history and the state key.
    struct Legacy {
        dir: PathBuf,
        history: PathBuf,
    }

    fn leave_legacy(f: &Fx) -> Legacy {
        let dir = f.c.data_dir.join(LEGACY_DIR);
        let binary = dir.join("1.2.1").join(LEGACY_DIR);
        write(&binary, "binary", None);
        let history = state::legacy_rtk_data_dir(f.env());
        write(&history.join("history.db"), "commands", None);
        write(&history.join("tee").join("1_x.log"), "out", None);
        state::write(
            f.env(),
            vec![
                (LEGACY_KEY.into(), Value::Str(p(&binary))),
                ("kept".into(), Value::Str("yes".into())),
            ],
        )
        .unwrap();
        Legacy { dir, history }
    }

    fn has_legacy_key(f: &Fx) -> bool {
        f.state().get(LEGACY_KEY).is_some()
    }

    fn statuses(kv: &[(&str, &str)]) -> Value {
        let mut m = Map::new();
        for (k, v) in kv {
            m.insert((*k).into(), Value::Str((*v).into()));
        }
        Value::Object(m)
    }

    /// The preview names all three leftovers, and writes nothing; apply writes settings.json
    /// first (everything is still there when it does), then drops the state key, then deletes
    /// the folder, then the history.
    #[test]
    fn setup_apply_removes_legacy_in_order() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        let old = leave_legacy(&f);
        let before = mem::snapshot(f.tmp());
        let preview = f.preview(None);
        assert_eq!(mem::snapshot(f.tmp()), before);
        let shown = at(&preview, &["legacy_rtk"]);
        assert_eq!(at(shown, &["state", "present"]), &Value::Bool(true));
        assert_eq!(text_at(shown, &["state", "key"]), LEGACY_KEY);
        assert_eq!(text_at(shown, &["dir", "dir"]), p(&old.dir));
        assert_eq!(text_at(shown, &["dir", "status"]), "delete");
        assert_eq!(text_at(shown, &["history", "dir"]), p(&old.history));
        assert_eq!(text_at(shown, &["history", "status"]), "delete");

        let seen: RefCell<Vec<(bool, bool, bool)>> = RefCell::new(Vec::new());
        let spy = |path: &Path, text: &str| {
            seen.borrow_mut()
                .push((has_legacy_key(&f), old.dir.exists(), old.history.exists()));
            settings::write_text(path, text)
        };
        let deps = SetupDeps {
            settings_write: &spy,
            ..f.deps()
        };
        let res = f.apply_d(&deps);
        assert!(ok(&res), "{res:?}");
        assert_eq!(*seen.borrow(), [(true, true, true)]);
        assert!(f.read_settings().contains("Bash(atl *)"));
        assert!(!has_legacy_key(&f));
        assert!(!old.dir.exists());
        assert!(!old.history.exists());
        assert_eq!(f.state().get("kept"), Some(&Value::Str("yes".into())));
        assert!(f.state().get("setup_done").is_some());
        assert_eq!(
            at(&res, &["legacy_rtk"]),
            &statuses(&[
                ("state", "dropped"),
                ("dir", "deleted"),
                ("history", "deleted")
            ])
        );
        // A second run finds nothing left.
        let again = f.apply();
        assert!(ok(&again), "{again:?}");
        assert_eq!(
            at(&again, &["legacy_rtk"]),
            &statuses(&[
                ("state", "nothing to drop"),
                ("dir", "nothing to delete"),
                ("history", "nothing to delete")
            ])
        );
    }

    /// Step 1 failing stops everything: nothing old is touched, so a retry still finds it.
    #[test]
    fn setup_apply_leaves_legacy_alone_when_the_settings_write_fails() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        let old = leave_legacy(&f);
        let disk_full = |_: &Path, _: &str| Err(PyExc::Other("OSError", "disk full".into()));
        let deps = SetupDeps {
            settings_write: &disk_full,
            ..f.deps()
        };
        let res = f.apply_d(&deps);
        assert!(!ok(&res));
        assert!(has_legacy_key(&f));
        assert!(old.dir.exists());
        assert!(old.history.exists());
    }

    /// The same guards as remove: a symlinked folder or history is never followed or deleted.
    #[test]
    fn setup_apply_keeps_a_symlinked_legacy_folder_and_history() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        let elsewhere = f.c.mem.config.join("elsewhere");
        write(&elsewhere.join("dir/precious.txt"), "keep", None);
        write(&elsewhere.join("history/precious.txt"), "keep", None);
        fs::create_dir_all(&f.c.data_dir).unwrap();
        let dir = f.c.data_dir.join(LEGACY_DIR);
        symlink(elsewhere.join("dir"), &dir).unwrap();
        let history = state::legacy_rtk_data_dir(f.env());
        fs::create_dir_all(history.parent().unwrap()).unwrap();
        symlink(elsewhere.join("history"), &history).unwrap();
        let res = f.apply();
        assert!(ok(&res), "{res:?}");
        assert!(elsewhere.join("dir/precious.txt").exists());
        assert!(elsewhere.join("history/precious.txt").exists());
        assert!(fs::symlink_metadata(&dir).unwrap().file_type().is_symlink());
        assert!(
            fs::symlink_metadata(&history)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        let shown = at(&res, &["legacy_rtk"]);
        assert_eq!(text_at(shown, &["dir"]), "left in place");
        assert_eq!(text_at(shown, &["history"]), "left in place");
        let warnings = dumped(at(&res, &["warnings"]));
        assert!(warnings.contains("legacy"), "{warnings}");
    }

    /// A data folder that is the config folder or above plugins/data never loses a folder.
    #[test]
    fn setup_apply_never_deletes_a_legacy_folder_outside_the_plugin_data_folder() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        let config = f.c.mem.config.clone();
        for data in [config.clone(), config.join("plugins"), f.c.mem.home.clone()] {
            write(&data.join(LEGACY_DIR).join("precious.txt"), "keep", None);
            let env = f.env_without_data();
            state::write(&env, vec![("data_dir".into(), Value::Str(p(&data)))]).unwrap();
            let digest =
                text_at(&f.preview_d(None, &f.deps()), &["settings", "sha256"]).to_string();
            let res = setup::apply(
                &f.c.plugin,
                &digest,
                Some(f.tmp()),
                false,
                &env,
                &[],
                &f.deps(),
            );
            assert!(ok(&res), "{data:?}: {res:?}");
            assert!(
                data.join(LEGACY_DIR).join("precious.txt").exists(),
                "{data:?}"
            );
        }
    }

    /// Remove deletes the ATL folder and history and the legacy ones, shown in its preview.
    #[test]
    fn setup_remove_covers_atl_and_legacy() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        f.apply();
        let atl_dir = f.c.data_dir.join("atl");
        write(&atl_dir.join("1.0/atl"), "bin", None);
        let atl_history = state::atl_data_dir(f.env());
        write(&atl_history.join("history.db"), "commands", None);
        let old = leave_legacy(&f);
        let all = [
            ("atl", &atl_dir),
            ("atl_history", &atl_history),
            ("legacy_rtk", &old.dir),
            ("legacy_rtk_history", &old.history),
        ];
        let preview = f.remove(false, false, true);
        assert!(ok(&preview), "{preview:?}");
        for (key, dir) in all {
            assert_eq!(text_at(&preview, &[key, "dir"]), p(dir), "{key}");
            assert_eq!(text_at(&preview, &[key, "status"]), "delete", "{key}");
            assert!(dir.exists(), "{key}");
        }
        let res = f.remove(false, false, false);
        assert!(ok(&res), "{res:?}");
        for (key, dir) in all {
            assert_eq!(
                at(&res, &[key]),
                &statuses(&[("status", "deleted")]),
                "{key}"
            );
            assert!(!dir.exists(), "{key}");
        }
        assert!(!f.state_exists());
    }

    /// Remove applies the same guards to the legacy folder as to the ATL one.
    #[test]
    fn setup_remove_never_deletes_a_legacy_folder_outside_the_plugin_data_folder() {
        let f = Fx::new();
        f.c.write_settings(SETTINGS);
        f.apply();
        let config = f.c.mem.config.clone();
        for data in [
            config.clone(),
            config.join("plugins/data"),
            f.c.mem.home.clone(),
        ] {
            write(&data.join(LEGACY_DIR).join("precious.txt"), "keep", None);
            state::write(f.env(), vec![("data_dir".into(), Value::Str(p(&data)))]).unwrap();
            let res = f.remove_env(&f.env_without_data(), false, false, false);
            assert!(
                data.join(LEGACY_DIR).join("precious.txt").exists(),
                "{data:?}"
            );
            assert!(
                dumped(at(&res, &["warnings"])).contains("legacy"),
                "{data:?}: {res:?}"
            );
            f.apply();
        }
    }
}
