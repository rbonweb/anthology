//! Port of `tests/test_doctor.py` (RT1b): the checks, MCP-server discovery and approval,
//! duplicate detection and `main`'s output. Modules are the Python classes; every test keeps
//! the Python method name. The compat-level `CompatBase` rows are RT1a's
//! (`tests/compat.rs`); `CliTest` runs the binary (`crates/anthology-cli/tests/doctor.rs`).
//!
//! Seams (GC-19): `shutil.which` → `DoctorDeps.which`, `subprocess.run` → `run`,
//! `platform.system`/`machine` → `uname`, the `atl_install` module the Python tests swap →
//! `atl_installed` (with a real atl.lock), `SYSTEM_APPLICATIONS` / `MANAGED_MCP_PATHS` →
//! `DoctorLimits`. F-5: the first check is `anthology_binary`, not `python3`.
#![forbid(unsafe_code)]

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anthology_core::Env;
use anthology_core::lock::Lock;
use anthology_pycompat::json::{self, Map, Value};
use anthology_pycompat::pyerr::PyExc;
use anthology_pycompat::str::is_printable;
use anthology_runtime::atl_install::{self, AtlDeps};
use anthology_runtime::compat;
use anthology_runtime::doctor::{self, Check, DoctorDeps, DoctorLimits, Report, RunResult};
use anthology_runtime::fetch::Fetch;
use anthology_testkit::release;
use anthology_testkit::rt_b::{ATL_HOST, AtlFakeFetch, FakeAnswer, atl_url_for};
use anthology_testkit::{Plugin, Sandbox, repo_root};

/// `PLUGIN`: the repository's anthology plugin.
fn plugin() -> PathBuf {
    repo_root().join("plugins").join("anthology")
}

/// `_write(path, text)`: parents created.
fn write(path: &Path, text: &str) -> PathBuf {
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, text).expect("write");
    path.to_path_buf()
}

fn write_bytes(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, bytes).expect("write");
}

/// A path as a JSON string literal (`json.dumps(str(path))`).
fn js(p: &Path) -> String {
    json::dumps(
        &Value::Str(p.to_str().expect("utf-8").into()),
        &json::PY_DEFAULT,
    )
}

fn utf16le_bom(text: &str) -> Vec<u8> {
    let mut out = vec![0xff, 0xfe];
    for u in text.encode_utf16() {
        out.extend_from_slice(&u.to_le_bytes());
    }
    out
}

fn printable(s: &str) -> bool {
    s.chars().all(is_printable)
}

const INSTALLED: &str = r#"{
    "version": 2,
    "plugins": {
        "anthology@anthology": [{"scope": "user", "installPath": "/x/a", "version": "0.1.0"}],
        "superpowers@claude-plugins-official": [
            {"scope": "user", "installPath": "/x/s", "version": "7.0.2"}],
        "superpowers@other-market": [{"scope": "project", "installPath": "/x/s2"},
                                     {"scope": "user", "installPath": "/x/s3"}],
        "design-skills@design-skills": [{"scope": "local", "installPath": "/x/d"}],
        "anthology-superpowers@anthology": [{"scope": "user", "installPath": "/x/as"}],
        "superpowers-extras@m": [{"scope": "user", "installPath": "/x/e"}]
    }
}"#;

fn installed() -> Value {
    json::loads(INSTALLED).expect("INSTALLED")
}

fn dups(text: &str) -> Vec<doctor::Duplicate> {
    doctor::find_duplicates_in(Some(&json::loads(text).expect("json")))
}

/// `EnvCase`: an isolated HOME/config/PATH, fake `which`/`docker`/platform, and the injected
/// application and managed-config paths.
struct EnvCase {
    _dir: tempfile::TempDir,
    tmp: PathBuf,
    config: PathBuf,
    env: Env,
    tools: Rc<RefCell<HashSet<String>>>,
    docker: Rc<Cell<RunResult>>,
    system: Rc<RefCell<(String, String)>>,
    apps: PathBuf,
    managed: PathBuf,
    cwd: PathBuf,
}

impl EnvCase {
    fn new() -> EnvCase {
        let dir = tempfile::Builder::new()
            .prefix("doctor-")
            .tempdir()
            .expect("tmp");
        let tmp = fs::canonicalize(dir.path()).expect("realpath");
        let config = tmp.join("config");
        fs::create_dir(&config).expect("config");
        let env = Env::default()
            .with_os("HOME", &tmp)
            .with_os("CLAUDE_CONFIG_DIR", &config)
            .with("PATH", "/bin");
        let cwd = tmp.join("proj");
        fs::create_dir(&cwd).expect("proj");
        EnvCase {
            _dir: dir,
            config,
            env,
            tools: Rc::new(RefCell::new(
                ["python3", "node", "docker"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            )),
            docker: Rc::new(Cell::new(RunResult::Exited(0))),
            system: Rc::new(RefCell::new(("Linux".into(), "x86_64".into()))),
            apps: tmp.join("SysApplications"),
            managed: tmp.join("managed-mcp.json"),
            cwd,
            tmp,
        }
    }

    fn set_tools(&self, tools: &[&str]) {
        *self.tools.borrow_mut() = tools.iter().map(|s| s.to_string()).collect();
    }

    fn deps(&self) -> DoctorDeps {
        let tools = Rc::clone(&self.tools);
        let docker = Rc::clone(&self.docker);
        let system = Rc::clone(&self.system);
        DoctorDeps {
            which: Box::new(move |n, _| {
                tools
                    .borrow()
                    .contains(n)
                    .then(|| PathBuf::from(format!("/usr/bin/{n}")))
            }),
            run: Box::new(move |argv, _, _| {
                assert_eq!(argv.len(), 2, "unexpected command {argv:?}");
                assert_eq!(argv[0], "/usr/bin/docker", "unexpected command {argv:?}");
                assert_eq!(argv[1], "info", "unexpected command {argv:?}");
                docker.get()
            }),
            uname: Box::new(move || system.borrow().clone()),
            limits: DoctorLimits {
                system_applications: vec![self.apps.clone()],
                managed_mcp_paths: vec![self.managed.clone()],
                managed_settings_paths: vec![self.tmp.join("managed-settings.json")],
            },
            ..DoctorDeps::default()
        }
    }

    fn by_name(checks: Vec<Check>) -> HashMap<&'static str, Check> {
        checks.into_iter().map(|c| (c.name, c)).collect()
    }

    /// `checks()`: `check_env(os.environ, cwd=self.cwd)` (the plugin is the repository's).
    fn checks(&self) -> HashMap<&'static str, Check> {
        self.checks_with(&self.env, &plugin(), &self.cwd, &self.deps())
    }

    fn checks_with(
        &self,
        env: &Env,
        root: &Path,
        cwd: &Path,
        deps: &DoctorDeps,
    ) -> HashMap<&'static str, Check> {
        EnvCase::by_name(doctor::check_env(env, root, cwd, deps))
    }

    /// `CompatBase.check(name, env_extra, plugin_root)`.
    fn check(&self, name: &str, extra: &[(&str, &str)], root: &Path) -> Check {
        let mut env = self.env.clone();
        for (k, v) in extra {
            env = env.with(k, v);
        }
        self.check_deps(name, &env, root, &self.deps())
    }

    fn check_deps(&self, name: &str, env: &Env, root: &Path, deps: &DoctorDeps) -> Check {
        let mut all = self.checks_with(env, root, &self.cwd, deps);
        all.remove(name).expect("a check of that name")
    }

    fn user_json(&self, text: &str) -> PathBuf {
        write(&self.config.join(".claude.json"), text)
    }

    fn find(&self, cwd: &Path) -> Vec<doctor::McpServer> {
        doctor::find_mcp_servers(&self.env, cwd, &self.deps().limits)
    }

    fn find_names(&self, cwd: &Path) -> HashMap<String, doctor::McpServer> {
        self.find(cwd)
            .into_iter()
            .map(|s| (s.name.clone(), s))
            .collect()
    }

    /// `ProjectApprovalTest.loaded_from(folder)`: name → approved (other scopes: true).
    fn loaded_from(&self, cwd: &Path) -> HashMap<String, bool> {
        self.find(cwd)
            .into_iter()
            .map(|s| (s.name.clone(), s.approved.unwrap_or(true)))
            .collect()
    }

    fn loaded(&self) -> HashMap<String, bool> {
        self.loaded_from(&self.cwd)
    }

    fn write_state(&self, text: &str) {
        write(&self.config.join("anthology").join("state.json"), text);
    }

    /// `main(argv)` with `os.getcwd()` = `self.cwd`: stdout and the exit code.
    fn main(&self, as_json: bool) -> (String, i32) {
        let report = doctor::report(&self.env, &plugin(), &self.cwd, &self.deps());
        doctor::render(&report, as_json)
    }
}

fn loaded(pairs: &[(&str, bool)]) -> HashMap<String, bool> {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

mod find_duplicates_test {
    use super::*;

    #[test]
    fn test_finds_superpowers_and_design_skills_installs_with_uninstall_commands() {
        let found = doctor::find_duplicates_in(Some(&installed()));
        let mut pairs: Vec<(String, String)> = found
            .iter()
            .map(|d| (d.plugin.clone(), d.scope.clone()))
            .collect();
        pairs.sort();
        assert_eq!(
            pairs,
            [
                ("design-skills@design-skills", "local"),
                ("superpowers@claude-plugins-official", "user"),
                ("superpowers@other-market", "project"),
                ("superpowers@other-market", "user"),
            ]
            .map(|(a, b)| (a.to_string(), b.to_string()))
        );
        let by: HashMap<(String, String), Option<String>> = found
            .into_iter()
            .map(|d| ((d.plugin, d.scope), d.command))
            .collect();
        let get = |p: &str, s: &str| by[&(p.to_string(), s.to_string())].clone();
        assert_eq!(
            get("superpowers@claude-plugins-official", "user").as_deref(),
            Some("claude plugin uninstall superpowers@claude-plugins-official -s user")
        );
        assert_eq!(
            get("design-skills@design-skills", "local").as_deref(),
            Some("claude plugin uninstall design-skills@design-skills -s local")
        );
    }

    #[test]
    fn test_ours_are_not_duplicates() {
        let names: HashSet<String> = doctor::find_duplicates_in(Some(&installed()))
            .into_iter()
            .map(|d| d.plugin)
            .collect();
        assert!(!names.contains("anthology-superpowers@anthology"));
        assert!(!names.contains("superpowers-extras@m"));
    }

    #[test]
    fn test_odd_shapes_give_no_duplicates() {
        assert_eq!(doctor::find_duplicates_in(None), []);
        for text in [
            "null",
            "[]",
            "{}",
            r#"{"plugins": []}"#,
            r#"{"plugins": {"superpowers@x": "bad"}}"#,
            r#"{"plugins": {"superpowers@x": [null, 3]}}"#,
        ] {
            assert_eq!(dups(text), [], "{text}");
        }
    }

    #[test]
    fn test_missing_scope_defaults_to_user() {
        let found = dups(r#"{"plugins": {"superpowers@m": [{"installPath": "/p"}]}}"#);
        assert_eq!(found[0].scope, "user");
        assert!(
            found[0]
                .command
                .as_deref()
                .expect("command")
                .ends_with("-s user")
        );
    }
}

mod check_env_test {
    use super::*;

    #[test]
    fn test_shape_and_order() {
        let c = EnvCase::new();
        let checks = doctor::check_env(&c.env, &plugin(), &c.cwd, &c.deps());
        let names: Vec<&str> = checks.iter().map(|x| x.name).collect();
        // F-5: anthology_binary replaces python3
        assert_eq!(
            names,
            [
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
                "atl_savings"
            ]
        );
        assert_eq!(names, doctor::CHECKS);
        // Check has exactly name, ok (true/false/none) and a str detail, by construction; the
        // JSON form has exactly those keys.
        for x in &checks {
            let v = doctor::check_value(x);
            let keys: Vec<&str> = v.as_object().expect("obj").iter().map(|(k, _)| k).collect();
            assert_eq!(keys, ["name", "ok", "detail"]);
        }
    }

    #[test]
    fn test_present_tools_are_ok() {
        let c = EnvCase::new();
        let all = c.checks();
        for name in ["node", "docker", "platform_supported"] {
            assert_eq!(all[name].ok, Some(true), "{name}");
        }
    }

    #[test]
    fn test_missing_tools_are_not_ok() {
        let c = EnvCase::new();
        c.set_tools(&[]);
        let all = c.checks();
        for name in ["node", "docker"] {
            assert_eq!(all[name].ok, Some(false), "{name}");
        }
        assert!(all["node"].detail.contains("Node"));
    }

    #[test]
    fn test_docker_on_path_but_not_running_says_start_it() {
        let c = EnvCase::new();
        c.docker.set(RunResult::Exited(1));
        let x = &c.checks()["docker"];
        assert_eq!(x.ok, Some(false));
        assert!(x.detail.to_lowercase().contains("start"));
    }

    #[test]
    fn test_docker_timeout_is_not_ok() {
        let c = EnvCase::new();
        c.docker.set(RunResult::Failed); // TimeoutExpired
        assert_eq!(c.checks()["docker"].ok, Some(false));
    }

    #[test]
    fn test_browseros_via_mcp_server_name_without_leaking_urls_or_tokens() {
        let c = EnvCase::new();
        c.user_json(
            r#"{"mcpServers": {"browseros-neo": {"type": "http", "url": "http://127.0.0.1:1/mcp?token=SECRET",
                "headers": {"Authorization": "Bearer SECRET"}}}}"#,
        );
        let x = &c.checks()["browseros"];
        assert_eq!(x.ok, Some(true));
        let blob = json::dumps(&doctor::check_value(x), &json::PY_DEFAULT);
        assert!(!blob.contains("SECRET"));
        assert!(!blob.contains("127.0.0.1"));
    }

    #[test]
    fn test_browseros_via_app_or_path() {
        let c = EnvCase::new();
        assert_eq!(c.checks()["browseros"].ok, Some(false));
        fs::create_dir_all(c.apps.join("BrowserOS.app")).expect("app");
        assert_eq!(c.checks()["browseros"].ok, Some(true));
        fs::remove_dir(c.apps.join("BrowserOS.app")).expect("rmdir");
        c.tools.borrow_mut().insert("browseros".into());
        assert_eq!(c.checks()["browseros"].ok, Some(true));
    }

    #[test]
    fn test_browseros_is_never_offered_as_an_install() {
        let c = EnvCase::new();
        let detail = c.checks()["browseros"].detail.to_lowercase();
        for bad in ["brew", "install chrome", "download"] {
            assert!(!detail.contains(bad), "{detail}");
        }
    }

    #[test]
    fn test_claude_json_in_home_is_used_without_config_dir() {
        let c = EnvCase::new();
        let env = Env::default().with_os("HOME", &c.tmp).with("PATH", "/bin");
        write(
            &c.tmp.join(".claude.json"),
            r#"{"mcpServers": {"my-browseros": {}}}"#,
        );
        let all = c.checks_with(&env, &plugin(), &c.cwd, &c.deps());
        assert_eq!(all["browseros"].ok, Some(true));
    }

    #[test]
    fn test_corrupt_claude_json_is_not_an_error() {
        let c = EnvCase::new();
        c.user_json("{nope");
        assert_eq!(c.checks()["browseros"].ok, Some(false));
    }

    #[test]
    fn test_windows_is_unsupported() {
        let c = EnvCase::new();
        *c.system.borrow_mut() = ("Windows".into(), "AMD64".into());
        let x = &c.checks()["platform_supported"];
        assert_eq!(x.ok, Some(false));
        assert!(x.detail.contains("macOS and Linux"));
    }

    /// A valid atl.lock at `path` for `version` (its digests are of `blob`).
    fn lock_file(path: &Path, version: &str, blob: &[u8]) -> Lock {
        release::write_lock(path, blob, version)
    }

    fn atl_check(c: &EnvCase, lock: &Path, deps: DoctorDeps) -> Check {
        let deps = DoctorDeps {
            atl_lock: Some(lock.to_path_buf()),
            ..deps
        };
        c.check_deps("atl", &c.env, &plugin(), &deps)
    }

    #[test]
    fn test_rtk_is_checked_through_the_lock_and_the_installed_marker() {
        let c = EnvCase::new();
        let data = c.tmp.join("data");
        let binary = data.join("atl").join("1.0.0").join("atl");
        write(&binary, "x");
        c.write_state(&format!(
            r#"{{"atl_path": {}, "data_dir": {}}}"#,
            js(&binary),
            js(&data)
        ));
        let lock_path = c.tmp.join("atl.lock");
        let parsed = lock_file(&lock_path, "1.0.0", b"blob");
        let calls: Rc<RefCell<Vec<(PathBuf, Lock, String)>>> = Rc::default();
        let answer = Rc::new(Cell::new(true));
        let deps = |calls: &Rc<RefCell<Vec<_>>>, answer: &Rc<Cell<bool>>| {
            let (calls, answer) = (Rc::clone(calls), Rc::clone(answer));
            DoctorDeps {
                atl_installed: Box::new(move |d, l, t| {
                    calls
                        .borrow_mut()
                        .push((d.to_path_buf(), l.clone(), t.triple().to_string()));
                    answer.get()
                }),
                ..c.deps()
            }
        };
        let x = atl_check(&c, &lock_path, deps(&calls, &answer));
        assert_eq!(x.ok, Some(true), "{x:?}");
        assert_eq!(
            *calls.borrow(),
            [(
                data.clone(),
                parsed,
                "x86_64-unknown-linux-musl".to_string()
            )]
        );
        answer.set(false);
        let x = atl_check(&c, &lock_path, deps(&calls, &answer));
        assert_eq!(x.ok, Some(false));
        assert!(x.detail.contains("setup"));
    }

    fn always_installed(c: &EnvCase) -> DoctorDeps {
        DoctorDeps {
            atl_installed: Box::new(|_, _, _| true),
            ..c.deps()
        }
    }

    #[test]
    fn doctor_reports_legacy_rtk_leftovers() {
        let c = EnvCase::new();
        let data = c.tmp.join("data");
        let binary = data.join("atl").join("1.0.0").join("atl");
        write(&binary, "x");
        let lock_path = c.tmp.join("atl.lock");
        lock_file(&lock_path, "1.0.0", b"blob");
        c.write_state(&format!(
            r#"{{"atl_path": {}, "data_dir": {}}}"#,
            js(&binary),
            js(&data)
        ));
        // A clean install says nothing about the old one.
        let x = atl_check(&c, &lock_path, always_installed(&c));
        assert_eq!(
            (x.ok, x.detail.as_str()),
            (Some(true), "atl matches atl.lock")
        );
        // The old install's state key, downloaded versions and history folder.
        c.write_state(&format!(
            r#"{{"atl_path": {}, "data_dir": {}, "rtk_path": "/old/rtk"}}"#, // legacy key
            js(&binary),
            js(&data)
        ));
        write(&data.join("rtk/1.2.1-anthology.1/rtk"), "x"); // legacy download
        write(&c.config.join("anthology/rtk/history.db"), "x"); // legacy history
        let x = atl_check(&c, &lock_path, always_installed(&c));
        assert_eq!(x.ok, Some(true), "{x:?}"); // the install itself is fine
        assert!(
            x.detail
                .starts_with("atl matches atl.lock; legacy rtk leftovers: ")
        );
        for part in [
            "the rtk_path key in state.json", // legacy
            "the old rtk download folder",    // legacy
            "the old rtk history folder",     // legacy
            "/anthology:setup",
        ] {
            assert!(x.detail.contains(part), "{part}: {x:?}");
        }
        // Each leftover is reported on its own.
        fs::remove_dir_all(data.join("rtk")).expect("rm download"); // legacy
        let x = atl_check(&c, &lock_path, always_installed(&c));
        assert!(!x.detail.contains("download folder"), "{x:?}");
        assert!(x.detail.contains("history folder"), "{x:?}");
        // A leftover is reported even when atl itself is not installed.
        c.write_state(r#"{"rtk_path": "/old/rtk"}"#); // legacy key
        let x = atl_check(&c, &lock_path, always_installed(&c));
        assert_eq!(x.ok, Some(false), "{x:?}");
        assert!(x.detail.contains("legacy rtk leftovers"), "{x:?}");
    }

    #[test]
    fn test_rtk_outside_the_locked_version_dir_is_not_ok() {
        let c = EnvCase::new();
        let data = c.tmp.join("data");
        let binary = data.join("atl").join("0.9.0").join("atl"); // older than the lock's
        write(&binary, "x");
        c.write_state(&format!(
            r#"{{"atl_path": {}, "data_dir": {}}}"#,
            js(&binary),
            js(&data)
        ));
        let lock_path = c.tmp.join("atl.lock");
        lock_file(&lock_path, "1.0.0", b"blob");
        let x = atl_check(&c, &lock_path, always_installed(&c));
        assert_eq!(x.ok, Some(false));
    }

    #[test]
    fn test_rtk_missing_binary_is_not_ok() {
        let c = EnvCase::new();
        let lock_path = c.tmp.join("atl.lock");
        lock_file(&lock_path, "1.0.0", b"blob");
        assert_eq!(
            atl_check(&c, &lock_path, always_installed(&c)).ok,
            Some(false)
        );
        c.write_state(&format!(
            r#"{{"atl_path": {}}}"#,
            js(&c.tmp.join("gone").join("atl"))
        ));
        assert_eq!(
            atl_check(&c, &lock_path, always_installed(&c)).ok,
            Some(false)
        );
    }

    #[test]
    fn test_rtk_unreadable_lock_is_not_wired_yet() {
        // load_lock raising: a lock that cannot be read (a folder) or does not validate
        let c = EnvCase::new();
        let lock_path = c.tmp.join("atl.lock");
        fs::create_dir(&lock_path).expect("a folder");
        let x = atl_check(&c, &lock_path, always_installed(&c));
        assert_eq!(x.ok, None);
        assert!(x.detail.contains("not wired yet"));
        fs::remove_dir(&lock_path).expect("rmdir");
        write(&lock_path, "{}");
        let x = atl_check(&c, &lock_path, always_installed(&c));
        assert_eq!(x.ok, None);
        assert!(x.detail.contains("not wired yet"));
    }

    struct Fake(AtlFakeFetch);

    impl Fetch for Fake {
        fn fetch(&self, url: &str, dest: &mut dyn Write, timeout: f64) -> Result<(), PyExc> {
            match self.0.call(url, timeout) {
                FakeAnswer::Write(b) => {
                    dest.write_all(b)?;
                    Ok(())
                }
                FakeAnswer::Raise(t, m) => Err(PyExc::Other(t, m.to_string())),
            }
        }
    }

    fn linux() -> (String, String) {
        (release::LINUX.0.into(), release::LINUX.1.into())
    }

    #[test]
    fn test_real_rtk_install_installed_binary_passes_and_tampering_fails() {
        let c = EnvCase::new();
        *c.system.borrow_mut() = linux();
        let blob = release::tarball(release::ATL_BODY);
        let lock_path = c.tmp.join("atl.lock");
        let lock = lock_file(&lock_path, release::VERSION, &blob);
        let data = c.tmp.join("plugin-data");
        let real = |c: &EnvCase| atl_check(c, &lock_path, c.deps());
        assert_eq!(real(&c).ok, Some(false)); // nothing installed yet
        let fetch = Fake(AtlFakeFetch::serving(&atl_url_for(&lock, ATL_HOST), &blob));
        let mut rdeps = AtlDeps::new(&fetch);
        rdeps.uname = linux;
        let result = atl_install::reconcile(&c.env, &data, &lock, 5.0, &rdeps);
        assert_eq!(result.warning, None, "{result:?}");
        let x = real(&c);
        assert_eq!(x.ok, Some(true), "{x:?}");
        let binary = result.atl_path.expect("atl_path");
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o600)).expect("chmod");
        assert_eq!(real(&c).ok, Some(false)); // no longer executable
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).expect("chmod");
        assert_eq!(real(&c).ok, Some(true));
        // a lock bumped to a version that is not installed yet
        lock_file(&lock_path, "1.3.0-anthology.1", &blob);
        assert_eq!(real(&c).ok, Some(false));
    }

    #[test]
    fn test_real_rtk_install_missing_lock_is_not_wired_yet() {
        let c = EnvCase::new();
        let x = atl_check(&c, &c.tmp.join("absent.lock"), c.deps());
        assert_eq!(x.ok, None);
    }

    fn plugin_rule(c: &EnvCase) -> Check {
        c.check("plugin_read_rule", &[], &plugin())
    }

    #[test]
    fn test_plugin_read_rule_is_unknown_before_setup() {
        let c = EnvCase::new();
        assert_eq!(plugin_rule(&c).ok, None);
    }

    #[test]
    fn test_plugin_read_rule_after_setup() {
        let c = EnvCase::new();
        c.write_state(r#"{"setup_done": "0.1.0"}"#);
        assert_eq!(plugin_rule(&c).ok, Some(false)); // no settings.json yet
        let settings = c.config.join("settings.json");
        write(
            &settings,
            r#"{"permissions": {"allow": ["Read(//old/**)"]}}"#,
        );
        let x = plugin_rule(&c);
        assert_eq!(x.ok, Some(false));
        assert!(x.detail.contains("setup"));
        let rule = anthology_settings::plugin_read_rule(&plugin());
        write(
            &settings,
            &format!(
                r#"{{"permissions": {{"allow": [{}]}}}}"#,
                json::dumps(&Value::Str(rule), &json::PY_DEFAULT)
            ),
        );
        assert_eq!(plugin_rule(&c).ok, Some(true));
        write(&settings, "{nope");
        assert!(plugin_rule(&c).detail.contains("cannot be read"));
    }

    #[test]
    fn test_check_env_never_raises() {
        let c = EnvCase::new();
        let deps = DoctorDeps {
            which: Box::new(|_, _| panic!("boom")),
            ..c.deps()
        };
        let checks = doctor::check_env(&c.env, &plugin(), &c.cwd, &deps);
        assert_eq!(checks.len(), 13);
        // F-5: checks[0] (anthology_binary) never calls which; the first that does is node.
        assert_eq!(checks[1].ok, Some(false));
        assert_eq!(checks[1].detail, "check failed: boom");
        assert_eq!(checks[2].detail, "check failed: boom");
    }
}

/// `AtlSavingsTest`: what atl did, from its own history next to state.json.
mod rtk_savings_test {
    use super::*;

    /// `history(decisions, commands, wal)`: a history.db with the atl columns the check reads.
    fn history(
        c: &EnvCase,
        decisions: &[(&str, &str)],
        commands: &[(&str, i64, i64)],
        wal: bool,
    ) -> PathBuf {
        let db = c.config.join("anthology").join("atl").join("history.db");
        fs::create_dir_all(db.parent().expect("parent")).expect("mkdir");
        let con = rusqlite::Connection::open(&db).expect("open");
        if wal {
            // as atl keeps it
            let mode: String = con
                .query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))
                .expect("wal");
            assert_eq!(mode, "wal");
        }
        con.execute_batch(
            "CREATE TABLE hook_decisions (id INTEGER PRIMARY KEY, raw_cmd TEXT, decision TEXT);
             CREATE TABLE commands (id INTEGER PRIMARY KEY, atl_cmd TEXT, input_tokens INTEGER,
                                    saved_tokens INTEGER);",
        )
        .expect("schema");
        for (raw, decision) in decisions {
            con.execute(
                "INSERT INTO hook_decisions (raw_cmd, decision) VALUES (?1, ?2)",
                (raw, decision),
            )
            .expect("insert");
        }
        for (cmd, input, saved) in commands {
            con.execute(
                "INSERT INTO commands (atl_cmd, input_tokens, saved_tokens) VALUES (?1, ?2, ?3)",
                (cmd, input, saved),
            )
            .expect("insert");
        }
        drop(con);
        db
    }

    fn savings(c: &EnvCase) -> Check {
        c.checks().remove("atl_savings").expect("atl_savings")
    }

    fn listing(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .expect("ls")
            .map(|e| e.expect("entry").file_name().into_string().expect("utf-8"))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn test_no_history_yet_is_none() {
        let c = EnvCase::new();
        let none = Check {
            name: "atl_savings",
            ok: None,
            detail: "atl has no history yet".into(),
        };
        assert_eq!(savings(&c), none);
        history(&c, &[], &[], true);
        assert_eq!(savings(&c).detail, "atl has no history yet");
    }

    #[test]
    fn test_commands_and_tokens_from_the_history() {
        let c = EnvCase::new();
        let mut decisions = vec![("git status", "defer"); 2999];
        decisions.push(("cat a", "allow"));
        let db = history(
            &c,
            &decisions,
            &[("atl read a", 1000, 35), ("atl grep x", 233, 0)],
            true,
        );
        let dir = db.parent().expect("parent");
        let before = (listing(dir), fs::read(&db).expect("read"));
        let got = savings(&c);
        assert_eq!(got.ok, Some(true));
        assert_eq!(
            got.detail,
            "atl ran 2 of the 3,000 commands the hook saw (0.1%) and saved 35 of their 1,233 \
             output tokens (2.8%), from atl's own history"
        );
        // read only
        assert_eq!((listing(dir), fs::read(&db).expect("read")), before);
    }

    #[test]
    fn test_percentages_round_half_up_and_never_divide_by_zero() {
        let c = EnvCase::new();
        // 6.25%; a negative saving counts as 0
        history(&c, &[("x", "defer"); 16], &[("atl ls", 0, -5)], true);
        assert_eq!(
            savings(&c).detail,
            "atl ran 1 of the 16 commands the hook saw (6.3%) and saved 0 of their 0 output \
             tokens (0.0%), from atl's own history"
        );
    }

    #[test]
    fn test_unreadable_history_is_none() {
        let c = EnvCase::new();
        let db = history(&c, &[("x", "defer")], &[], false);
        fs::write(&db, b"not a database".repeat(100)).expect("write");
        let unreadable = Check {
            name: "atl_savings",
            ok: None,
            detail: "atl's history cannot be read".into(),
        };
        assert_eq!(savings(&c), unreadable);
        fs::remove_file(&db).expect("rm");
        let con = rusqlite::Connection::open(&db).expect("open");
        con.execute_batch("CREATE TABLE commands (input_tokens INTEGER, saved_tokens INTEGER)")
            .expect("schema");
        drop(con);
        // no hook_decisions table
        assert_eq!(savings(&c).detail, "atl's history cannot be read");
    }
}

mod browseros_app_test {
    use super::*;

    #[test]
    fn test_neo_app_in_system_applications() {
        let c = EnvCase::new();
        fs::create_dir_all(c.apps.join("BrowserOS neo.app")).expect("app");
        let x = &c.checks()["browseros"];
        assert_eq!(x.ok, Some(true), "{x:?}");
    }

    #[test]
    fn test_plain_app_in_user_applications() {
        let c = EnvCase::new();
        fs::create_dir(c.tmp.join("Applications")).expect("mkdir");
        assert_eq!(c.checks()["browseros"].ok, Some(false));
        fs::create_dir(c.tmp.join("Applications").join("BrowserOS.app")).expect("app");
        assert_eq!(c.checks()["browseros"].ok, Some(true));
    }

    #[test]
    fn test_unrelated_app_is_not_found() {
        let c = EnvCase::new();
        fs::create_dir_all(c.apps.join("Foo.app")).expect("app");
        // the prefix is case-sensitive (also on a case-insensitive file system)
        let _ = fs::create_dir(c.apps.join("browseros.app"));
        assert_eq!(c.checks()["browseros"].ok, Some(false));
    }

    #[test]
    fn test_a_file_is_not_an_app() {
        let c = EnvCase::new();
        write(&c.apps.join("BrowserOS neo.app"), "x");
        assert_eq!(c.checks()["browseros"].ok, Some(false));
    }

    #[test]
    fn test_linux_binaries_on_path() {
        let c = EnvCase::new();
        for name in ["browseros", "browseros-neo", "BrowserOS"] {
            c.set_tools(&["python3", name]);
            assert_eq!(c.checks()["browseros"].ok, Some(true), "{name}");
        }
    }

    #[test]
    fn test_linux_appimage_in_user_applications_or_local_bin() {
        let c = EnvCase::new();
        assert_eq!(c.checks()["browseros"].ok, Some(false));
        let local = c
            .tmp
            .join(".local")
            .join("bin")
            .join("BrowserOS-1.2.AppImage");
        write(&local, "x");
        assert_eq!(c.checks()["browseros"].ok, Some(true));
        fs::remove_file(&local).expect("unlink");
        write(&c.tmp.join("Applications").join("BrowserOS.AppImage"), "x");
        assert_eq!(c.checks()["browseros"].ok, Some(true));
    }

    #[test]
    fn test_not_found_message_still_says_install_it_yourself() {
        let c = EnvCase::new();
        let x = &c.checks()["browseros"];
        assert_eq!(x.ok, Some(false));
        assert!(x.detail.contains("agents never install"));
    }
}

mod find_mcp_servers_test {
    use super::*;

    #[test]
    fn test_user_scope() {
        let c = EnvCase::new();
        c.user_json(
            r#"{"mcpServers": {"browseros-neo": {"url": "http://127.0.0.1:9/mcp?token=SECRET"}}}"#,
        );
        let s = &c.find_names(&c.cwd)["browseros-neo"];
        assert_eq!(
            (s.scope, s.tool_prefix.as_str()),
            ("user", "mcp__browseros-neo__")
        );
        assert_eq!(s.approved, None); // {"name", "scope", "tool_prefix"} only
    }

    #[test]
    fn test_user_scope_from_home_without_config_dir() {
        let c = EnvCase::new();
        write(&c.tmp.join(".claude.json"), r#"{"mcpServers": {"a": {}}}"#);
        let env = Env::default().with_os("HOME", &c.tmp).with("PATH", "/bin");
        let names: Vec<String> = doctor::find_mcp_servers(&env, &c.cwd, &c.deps().limits)
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(names, ["a"]);
    }

    #[test]
    fn test_local_scope_for_cwd_and_ancestors() {
        let c = EnvCase::new();
        let deep = c.cwd.join("a").join("b");
        fs::create_dir_all(&deep).expect("deep");
        c.user_json(&format!(
            r#"{{"projects": {{{}: {{"mcpServers": {{"loc": {{"command": "SECRETCMD"}}}}}},
                {}: {{"mcpServers": {{"nope": {{}}}}}}}}}}"#,
            js(&c.cwd),
            js(&c.tmp.join("other"))
        ));
        let found = c.find_names(&deep);
        assert_eq!(found["loc"].scope, "local");
        assert!(!found.contains_key("nope"));
    }

    #[test]
    fn test_project_scope_walks_up_to_the_first_mcp_json() {
        let c = EnvCase::new();
        let deep = c.cwd.join("a").join("b");
        fs::create_dir_all(&deep).expect("deep");
        write(
            &c.cwd.join(".mcp.json"),
            r#"{"mcpServers": {"proj": {"args": ["SECRETARG"]}}}"#,
        );
        let s = &c.find_names(&deep)["proj"];
        assert_eq!(
            (s.scope, s.tool_prefix.as_str()),
            ("project", "mcp__proj__")
        );
    }

    #[test]
    fn test_project_walk_stops_at_the_git_toplevel() {
        let c = EnvCase::new();
        write(
            &c.tmp.join(".mcp.json"),
            r#"{"mcpServers": {"outside": {}}}"#,
        );
        fs::create_dir(c.cwd.join(".git")).expect("git");
        fs::create_dir(c.cwd.join("sub")).expect("sub");
        assert!(!c.find_names(&c.cwd.join("sub")).contains_key("outside"));
    }

    #[test]
    fn test_plugin_scope_from_mcp_json_and_plugin_json() {
        let c = EnvCase::new();
        let (one, two) = (c.tmp.join("p1"), c.tmp.join("p2"));
        write(
            &one.join(".mcp.json"),
            r#"{"mcpServers": {"srv": {"url": "http://SECRETHOST"}}}"#,
        );
        write(&two.join(".mcp.json"), r#"{"flat": {"command": "x"}}"#); // top-level map form
        write(
            &two.join(".claude-plugin").join("plugin.json"),
            r#"{"mcpServers": {"inline": {"command": "y"}}}"#,
        );
        write(
            &c.config.join("plugins").join("installed_plugins.json"),
            &format!(
                r#"{{"version": 2, "plugins": {{
                "one@market": [{{"scope": "user", "installPath": {}}}],
                "two@market": [{{"scope": "user", "installPath": {}}}]}}}}"#,
                js(&one),
                js(&two)
            ),
        );
        let found = c.find_names(&c.cwd);
        assert_eq!(found["srv"].scope, "plugin");
        assert_eq!(found["srv"].tool_prefix, "mcp__plugin_one_srv__");
        assert_eq!(found["flat"].tool_prefix, "mcp__plugin_two_flat__");
        assert_eq!(found["inline"].tool_prefix, "mcp__plugin_two_inline__");
    }

    #[test]
    fn test_managed_scope() {
        let c = EnvCase::new();
        write(
            &c.managed,
            r#"{"mcpServers": {"corp": {"url": "http://SECRETHOST"}}}"#,
        );
        let s = &c.find_names(&c.cwd)["corp"];
        assert_eq!(
            (s.scope, s.tool_prefix.as_str()),
            ("managed", "mcp__corp__")
        );
    }

    #[test]
    fn test_malformed_files_are_skipped() {
        let c = EnvCase::new();
        c.user_json("{nope");
        write(&c.cwd.join(".mcp.json"), "[1, 2");
        write(&c.managed, "");
        write(
            &c.config.join("plugins").join("installed_plugins.json"),
            r#"{"plugins": {"x@m": "bad", "y@m": [null, {"installPath": 3}]}}"#,
        );
        assert_eq!(c.find(&c.cwd), []);
        c.user_json(&format!(
            r#"{{"mcpServers": ["a"], "projects": {{{}: "bad"}}}}"#,
            js(&c.cwd)
        ));
        assert_eq!(c.find(&c.cwd), []);
    }

    #[test]
    fn test_no_value_from_any_config_reaches_any_check() {
        let c = EnvCase::new();
        let secret = "SECRETVALUE123";
        c.user_json(&format!(
            r#"{{"mcpServers": {{"my-browseros": {{"url": "http://{secret}/mcp", "headers": {{"A": "{secret}"}}}}}}}}"#
        ));
        write(
            &c.cwd.join(".mcp.json"),
            &format!(
                r#"{{"mcpServers": {{"p": {{"command": "{secret}", "args": ["{secret}"]}}}}}}"#
            ),
        );
        write(
            &c.managed,
            &format!(r#"{{"mcpServers": {{"m": {{"env": {{"T": "{secret}"}}}}}}}}"#),
        );
        let checks = doctor::check_env(&c.env, &plugin(), &c.cwd, &c.deps());
        let blob = json::dumps(
            &Value::Array(checks.iter().map(doctor::check_value).collect()),
            &json::PY_DEFAULT,
        );
        assert!(!blob.contains(secret));
        assert!(blob.contains("my-browseros"));
    }
}

mod browseros_tools_test {
    use super::*;

    #[test]
    fn test_exact_name_in_user_scope_is_ok() {
        let c = EnvCase::new();
        c.user_json(r#"{"mcpServers": {"browseros-neo": {}}}"#);
        assert_eq!(c.checks()["browseros_tools"].ok, Some(true));
    }

    #[test]
    fn test_exact_name_in_project_and_managed_scope_is_ok() {
        let c = EnvCase::new();
        write(
            &c.cwd.join(".mcp.json"),
            r#"{"mcpServers": {"browseros-neo": {}}}"#,
        );
        write(
            &c.config.join("settings.json"),
            r#"{"enabledMcpjsonServers": ["browseros-neo"]}"#,
        );
        assert_eq!(c.checks()["browseros_tools"].ok, Some(true));
        fs::remove_file(c.cwd.join(".mcp.json")).expect("unlink");
        write(&c.managed, r#"{"mcpServers": {"browseros-neo": {}}}"#);
        assert_eq!(c.checks()["browseros_tools"].ok, Some(true));
    }

    #[test]
    fn test_wrong_name_is_loud() {
        let c = EnvCase::new();
        c.user_json(r#"{"mcpServers": {"browseros": {"url": "http://SECRETHOST"}}}"#);
        let x = &c.checks()["browseros_tools"];
        assert_eq!(x.ok, Some(false));
        for want in ["mcp__browseros-neo__", "browseros", "user"] {
            assert!(x.detail.contains(want), "{want}: {}", x.detail);
        }
        assert!(!x.detail.contains("SECRETHOST"));
    }

    #[test]
    fn test_plugin_provided_server_is_loud() {
        let c = EnvCase::new();
        let plug = c.tmp.join("bp");
        write(
            &plug.join(".mcp.json"),
            r#"{"mcpServers": {"browseros-neo": {}}}"#,
        );
        write(
            &c.config.join("plugins").join("installed_plugins.json"),
            &format!(
                r#"{{"plugins": {{"bp@m": [{{"installPath": {}}}]}}}}"#,
                js(&plug)
            ),
        );
        let x = &c.checks()["browseros_tools"];
        assert_eq!(x.ok, Some(false));
        assert!(x.detail.contains("mcp__browseros-neo__"));
        assert!(x.detail.contains("mcp__plugin_bp_browseros-neo__"));
    }

    #[test]
    fn test_no_server_is_none() {
        let c = EnvCase::new();
        assert_eq!(c.checks()["browseros_tools"].ok, None);
    }

    #[test]
    fn test_the_browseros_check_accepts_any_loaded_scope() {
        let c = EnvCase::new();
        write(
            &c.cwd.join(".mcp.json"),
            r#"{"mcpServers": {"x-browseros": {}}}"#,
        );
        write(
            &c.config.join("settings.json"),
            r#"{"enableAllProjectMcpServers": true}"#,
        );
        assert_eq!(c.checks()["browseros"].ok, Some(true));
        assert!(c.checks()["browseros"].detail.contains("configured"));
    }
}

const HOSTILE: &str = "browseros\x1b[31m\nFAKE  ok   forged line\r\u{202e}";

fn hostile_json() -> String {
    json::dumps(&Value::Str(HOSTILE.into()), &json::PY_DEFAULT)
}

mod project_approval_test {
    use super::*;

    fn project(c: &EnvCase, name: &str) {
        write(
            &c.cwd.join(".mcp.json"),
            &format!(r#"{{"mcpServers": {{"{name}": {{"url": "http://SECRETHOST/mcp"}}}}}}"#),
        );
    }

    /// `trust(folder, value)`: `.claude.json` `projects[<folder>].hasTrustDialogAccepted`.
    fn trust(c: &EnvCase, folder: &Path, value: Value) {
        let path = c.config.join(".claude.json");
        let mut data = fs::read_to_string(&path)
            .ok()
            .and_then(|t| json::loads(&t).ok())
            .unwrap_or_else(|| Value::Object(Map::new()));
        let top = data.as_object_mut().expect("an object");
        if top.get("projects").is_none() {
            top.insert("projects".into(), Value::Object(Map::new()));
        }
        let projects = top
            .get_mut("projects")
            .and_then(Value::as_object_mut)
            .expect("projects");
        let key = folder.to_str().expect("utf-8");
        if projects.get(key).is_none() {
            projects.insert(key.into(), Value::Object(Map::new()));
        }
        projects
            .get_mut(key)
            .and_then(Value::as_object_mut)
            .expect("entry")
            .insert("hasTrustDialogAccepted".into(), value);
        c.user_json(&json::dumps(&data, &json::PY_DEFAULT));
    }

    fn trust_cwd(c: &EnvCase) {
        trust(c, &c.cwd, Value::Bool(true));
    }

    /// `.mcp.json` at the git root; returns a working folder below it.
    fn git_project(c: &EnvCase) -> PathBuf {
        project(c, "browseros");
        fs::create_dir(c.cwd.join(".git")).expect("git");
        let sub = c.cwd.join("pkg").join("app");
        fs::create_dir_all(&sub).expect("sub");
        sub
    }

    #[test]
    fn test_unapproved_project_server_is_not_configured() {
        let c = EnvCase::new();
        project(&c, "browseros");
        assert_eq!(c.loaded(), loaded(&[("browseros", false)]));
        let x = &c.checks()["browseros"];
        assert_eq!(x.ok, Some(false));
        for want in ["unapproved project", "browseros", "install it"] {
            assert!(x.detail.contains(want), "{want}");
        }
        assert!(!x.detail.contains("SECRETHOST"));
    }

    #[test]
    fn test_unapproved_project_server_does_not_pass_browseros_tools() {
        let c = EnvCase::new();
        project(&c, "browseros-neo");
        let x = &c.checks()["browseros_tools"];
        assert_eq!(x.ok, None);
        assert!(x.detail.contains("unapproved project"));
    }

    #[test]
    fn test_unapproved_server_next_to_an_installed_app_says_so() {
        let c = EnvCase::new();
        project(&c, "browseros");
        fs::create_dir_all(c.apps.join("BrowserOS neo.app")).expect("app");
        let x = &c.checks()["browseros"];
        assert_eq!(x.ok, Some(true));
        assert!(x.detail.contains("installed"));
        assert!(x.detail.contains("unapproved project"));
    }

    #[test]
    fn test_approval_in_user_settings_by_name_or_wholesale() {
        let c = EnvCase::new();
        project(&c, "browseros");
        let settings = c.config.join("settings.json");
        write(&settings, r#"{"enabledMcpjsonServers": ["browseros"]}"#);
        assert_eq!(c.loaded(), loaded(&[("browseros", true)]));
        assert_eq!(c.checks()["browseros"].ok, Some(true));
        assert!(c.checks()["browseros"].detail.contains("configured"));
        write(&settings, r#"{"enabledMcpjsonServers": ["other"]}"#);
        assert_eq!(c.loaded(), loaded(&[("browseros", false)]));
        write(&settings, r#"{"enableAllProjectMcpServers": true}"#);
        assert_eq!(c.loaded(), loaded(&[("browseros", true)]));
    }

    #[test]
    fn test_approval_in_the_users_claude_json_needs_no_trust() {
        let c = EnvCase::new();
        project(&c, "browseros");
        c.user_json(&format!(
            r#"{{"projects": {{{}: {{"enabledMcpjsonServers": ["browseros"]}}}}}}"#,
            js(&c.cwd)
        ));
        assert_eq!(c.loaded(), loaded(&[("browseros", true)]));
        c.user_json(&format!(
            r#"{{"projects": {{{}: {{"enableAllProjectMcpServers": true}}}}}}"#,
            js(&c.cwd)
        ));
        assert_eq!(c.loaded(), loaded(&[("browseros", true)]));
    }

    #[test]
    fn test_local_settings_approve_only_in_a_trusted_workspace() {
        let c = EnvCase::new();
        project(&c, "browseros");
        write(
            &c.cwd.join(".claude").join("settings.local.json"),
            r#"{"enabledMcpjsonServers": ["browseros"]}"#,
        );
        assert_eq!(c.loaded(), loaded(&[("browseros", false)])); // a clone may commit this file
        trust_cwd(&c);
        assert_eq!(c.loaded(), loaded(&[("browseros", true)]));
    }

    #[test]
    fn test_shared_settings_approve_only_in_a_trusted_workspace() {
        let c = EnvCase::new();
        project(&c, "browseros");
        let shared = c.cwd.join(".claude").join("settings.json");
        write(
            &shared,
            r#"{"enableAllProjectMcpServers": true, "enabledMcpjsonServers": ["browseros"]}"#,
        );
        assert_eq!(c.loaded(), loaded(&[("browseros", false)]));
        trust_cwd(&c);
        assert_eq!(c.loaded(), loaded(&[("browseros", true)]));
        write(&shared, r#"{"enabledMcpjsonServers": ["other"]}"#);
        assert_eq!(c.loaded(), loaded(&[("browseros", false)]));
        write(&shared, r#"{"enableAllProjectMcpServers": true}"#);
        assert_eq!(c.loaded(), loaded(&[("browseros", true)]));
    }

    #[test]
    fn test_trust_must_be_recorded_for_this_project_and_be_true() {
        let c = EnvCase::new();
        project(&c, "browseros");
        write(
            &c.cwd.join(".claude").join("settings.json"),
            r#"{"enableAllProjectMcpServers": true}"#,
        );
        trust(&c, &c.tmp.join("elsewhere"), Value::Bool(true));
        assert_eq!(c.loaded(), loaded(&[("browseros", false)]));
        trust(&c, &c.tmp, Value::Bool(true)); // an ancestor's trust is not enough
        assert_eq!(c.loaded(), loaded(&[("browseros", false)]));
        for odd in [r#""true""#, "1", r#""yes""#, "null", "[true]"] {
            trust(&c, &c.cwd, json::loads(odd).expect("odd"));
            assert_eq!(c.loaded(), loaded(&[("browseros", false)]), "{odd}");
        }
        c.user_json("[]");
        assert_eq!(c.loaded(), loaded(&[("browseros", false)]));
    }

    #[test]
    fn test_repo_settings_are_read_from_the_working_folder_not_the_mcp_json_folder() {
        let c = EnvCase::new();
        let sub = git_project(&c);
        trust_cwd(&c); // the git root
        write(
            &c.config.join("settings.json"),
            r#"{"enableAllProjectMcpServers": true}"#,
        );
        assert_eq!(c.loaded_from(&sub), loaded(&[("browseros", true)]));
        let shared = sub.join(".claude").join("settings.json");
        write(&shared, r#"{"disabledMcpjsonServers": ["browseros"]}"#);
        assert_eq!(c.loaded_from(&sub), loaded(&[("browseros", false)]));
        fs::remove_file(&shared).expect("unlink");
        write(
            &sub.join(".claude").join("settings.local.json"),
            r#"{"disabledMcpjsonServers": ["browseros"]}"#,
        );
        assert_eq!(c.loaded_from(&sub), loaded(&[("browseros", false)]));
    }

    #[test]
    fn test_approval_in_the_working_folders_settings_not_the_mcp_json_folders() {
        let c = EnvCase::new();
        let sub = git_project(&c);
        trust_cwd(&c);
        write(
            &c.cwd.join(".claude").join("settings.local.json"),
            r#"{"enabledMcpjsonServers": ["browseros"]}"#,
        );
        assert_eq!(c.loaded_from(&sub), loaded(&[("browseros", false)])); // <cwd>/.claude
        write(
            &sub.join(".claude").join("settings.local.json"),
            r#"{"enabledMcpjsonServers": ["browseros"]}"#,
        );
        assert_eq!(c.loaded_from(&sub), loaded(&[("browseros", true)]));
    }

    #[test]
    fn test_trust_is_recorded_for_the_git_root_of_the_working_folder() {
        let c = EnvCase::new();
        let sub = git_project(&c);
        write(
            &sub.join(".claude").join("settings.json"),
            r#"{"enableAllProjectMcpServers": true}"#,
        );
        trust(&c, &sub, Value::Bool(true)); // a subfolder key is not how Claude Code records it
        assert_eq!(c.loaded_from(&sub), loaded(&[("browseros", false)]));
        trust_cwd(&c); // the git root is
        assert_eq!(c.loaded_from(&sub), loaded(&[("browseros", true)]));
    }

    #[test]
    fn test_trust_at_the_mcp_json_folder_below_the_git_root_does_not_count() {
        let c = EnvCase::new();
        fs::create_dir(c.cwd.join(".git")).expect("git");
        let mid = c.cwd.join("pkg");
        let sub = mid.join("app");
        fs::create_dir_all(&sub).expect("sub");
        write(
            &mid.join(".mcp.json"),
            r#"{"mcpServers": {"browseros": {}}}"#,
        );
        write(
            &sub.join(".claude").join("settings.json"),
            r#"{"enableAllProjectMcpServers": true}"#,
        );
        trust(&c, &mid, Value::Bool(true));
        assert_eq!(c.loaded_from(&sub), loaded(&[("browseros", false)]));
        trust_cwd(&c);
        assert_eq!(c.loaded_from(&sub), loaded(&[("browseros", true)]));
    }

    #[test]
    fn test_without_a_git_root_the_working_folder_is_the_trust_key() {
        let c = EnvCase::new();
        let sub = c.cwd.join("a");
        fs::create_dir(&sub).expect("sub");
        project(&c, "browseros"); // at cwd, no .git anywhere below tmp
        write(
            &sub.join(".claude").join("settings.json"),
            r#"{"enableAllProjectMcpServers": true}"#,
        );
        trust_cwd(&c); // the folder holding .mcp.json is not the working folder
        assert_eq!(c.loaded_from(&sub), loaded(&[("browseros", false)]));
        trust(&c, &sub, Value::Bool(true));
        assert_eq!(c.loaded_from(&sub), loaded(&[("browseros", true)]));
    }

    #[test]
    fn test_repo_settings_in_utf16_are_read() {
        let c = EnvCase::new();
        project(&c, "browseros");
        trust_cwd(&c);
        write_bytes(
            &c.cwd.join(".claude").join("settings.json"),
            &utf16le_bom(r#"{"disabledMcpjsonServers": ["browseros"]}"#),
        );
        write(
            &c.config.join("settings.json"),
            r#"{"enableAllProjectMcpServers": true}"#,
        );
        assert_eq!(c.loaded(), loaded(&[("browseros", false)]));
    }

    #[test]
    fn test_the_users_own_files_are_read_like_claude_code_does() {
        // doctor's reader must be the lenient one: UTF-16 BOM and bad bytes in the user's files
        let c = EnvCase::new();
        project(&c, "browseros");
        let path = c.config.join("settings.json");
        write_bytes(
            &path,
            &utf16le_bom(r#"{"enableAllProjectMcpServers": true}"#),
        );
        assert_eq!(c.loaded(), loaded(&[("browseros", true)]));
        write_bytes(
            &path,
            b"{\"x\": \"a\xff\", \"enableAllProjectMcpServers\": true}",
        );
        assert_eq!(c.loaded(), loaded(&[("browseros", true)]));
        fs::remove_file(&path).expect("unlink");
        write_bytes(
            &c.config.join(".claude.json"),
            &utf16le_bom(r#"{"mcpServers": {"browseros-neo": {}}}"#),
        );
        assert_eq!(
            c.loaded(),
            loaded(&[("browseros-neo", true), ("browseros", false)])
        );
    }

    #[test]
    fn test_user_settings_approve_without_trust() {
        let c = EnvCase::new();
        project(&c, "browseros");
        write(
            &c.config.join("settings.json"),
            r#"{"enableAllProjectMcpServers": true}"#,
        );
        assert_eq!(c.loaded(), loaded(&[("browseros", true)]));
    }

    #[test]
    fn test_a_server_disabled_in_any_source_is_never_approved() {
        let c = EnvCase::new();
        project(&c, "browseros");
        trust_cwd(&c);
        write(
            &c.config.join("settings.json"),
            r#"{"enableAllProjectMcpServers": true}"#,
        );
        for name in ["settings.json", "settings.local.json"] {
            let where_ = c.cwd.join(".claude").join(name);
            assert_eq!(c.loaded(), loaded(&[("browseros", true)]));
            write(&where_, r#"{"disabledMcpjsonServers": ["browseros"]}"#);
            assert_eq!(c.loaded(), loaded(&[("browseros", false)]), "{name}");
            fs::remove_file(&where_).expect("unlink");
        }
        c.user_json(&format!(
            r#"{{"projects": {{{}: {{"hasTrustDialogAccepted": true,
                "disabledMcpjsonServers": ["browseros"]}}}}}}"#,
            js(&c.cwd)
        ));
        assert_eq!(c.loaded(), loaded(&[("browseros", false)]));
    }

    #[test]
    fn test_disabled_in_shared_settings_wins_even_without_trust() {
        let c = EnvCase::new();
        project(&c, "browseros");
        write(
            &c.config.join("settings.json"),
            r#"{"enabledMcpjsonServers": ["browseros"]}"#,
        );
        write(
            &c.cwd.join(".claude").join("settings.json"),
            r#"{"disabledMcpjsonServers": ["browseros"]}"#,
        );
        assert_eq!(c.loaded(), loaded(&[("browseros", false)]));
    }

    #[test]
    fn test_oversized_repo_settings_approve_and_disable_nothing() {
        let c = EnvCase::new();
        project(&c, "browseros");
        trust_cwd(&c);
        let pad = "A".repeat(compat::REPO_READ_LIMIT as usize + 10);
        write(
            &c.cwd.join(".claude").join("settings.json"),
            &format!(r#"{{"enableAllProjectMcpServers": true, "pad": "{pad}"}}"#),
        );
        assert_eq!(c.loaded(), loaded(&[("browseros", false)]));
    }

    #[test]
    fn test_disabled_wins_over_enabled() {
        let c = EnvCase::new();
        project(&c, "browseros");
        write(
            &c.config.join("settings.json"),
            r#"{"enableAllProjectMcpServers": true, "disabledMcpjsonServers": ["browseros"]}"#,
        );
        assert_eq!(c.loaded(), loaded(&[("browseros", false)]));
    }

    #[test]
    fn test_non_project_scopes_are_not_gated() {
        let c = EnvCase::new();
        c.user_json(r#"{"mcpServers": {"browseros-neo": {}}}"#);
        write(&c.managed, r#"{"mcpServers": {"corp-browseros": {}}}"#);
        assert_eq!(
            c.loaded(),
            loaded(&[("browseros-neo", true), ("corp-browseros", true)])
        );
        assert_eq!(c.checks()["browseros"].ok, Some(true));
    }

    #[test]
    fn test_garbage_approval_lists_are_unapproved() {
        let c = EnvCase::new();
        project(&c, "browseros");
        for bad in [r#""browseros""#, r#"{"browseros": true}"#, "[3, null]", "7"] {
            write(
                &c.config.join("settings.json"),
                &format!(r#"{{"enabledMcpjsonServers": {bad}}}"#),
            );
            assert_eq!(c.loaded(), loaded(&[("browseros", false)]), "{bad}");
        }
    }
}

mod hostile_names_test {
    use super::*;

    #[test]
    fn test_wrong_name_detail_is_escaped() {
        let c = EnvCase::new();
        c.user_json(&format!(
            r#"{{"mcpServers": {{{}: {{"url": "http://SECRETHOST"}}}}}}"#,
            hostile_json()
        ));
        let x = &c.checks()["browseros_tools"];
        assert_eq!(x.ok, Some(false));
        for bad in ["\x1b", "\n", "\r", "\u{202e}"] {
            assert!(!x.detail.contains(bad), "{bad:?}");
        }
        assert!(x.detail.contains("\\x1b[31m"));
        assert!(x.detail.contains("\\n"));
        assert!(printable(&x.detail));
        assert!(!x.detail.contains("SECRETHOST"));
    }

    #[test]
    fn test_unapproved_name_is_escaped_too() {
        let c = EnvCase::new();
        write(
            &c.cwd.join(".mcp.json"),
            &format!(r#"{{"mcpServers": {{{}: {{}}}}}}"#, hostile_json()),
        );
        let all = c.checks();
        for name in ["browseros", "browseros_tools"] {
            assert!(printable(&all[name].detail), "{name}");
        }
    }

    #[test]
    fn test_only_the_first_few_servers_are_listed() {
        let c = EnvCase::new();
        let servers: Vec<String> = (0..9)
            .map(|i| format!(r#""browseros-{i}": {{}}"#))
            .collect();
        c.user_json(&format!(r#"{{"mcpServers": {{{}}}}}"#, servers.join(", ")));
        let x = &c.checks()["browseros_tools"];
        assert!(x.detail.contains("and 4 more"));
        assert_eq!(
            x.detail.matches("scope, tools").count(),
            doctor::SHOWN_SERVERS
        );
    }

    #[test]
    fn test_only_the_first_few_unapproved_servers_are_listed() {
        let c = EnvCase::new();
        let servers: Vec<String> = (0..9)
            .map(|i| format!(r#""browseros-{i}": {{}}"#))
            .collect();
        write(
            &c.cwd.join(".mcp.json"),
            &format!(r#"{{"mcpServers": {{{}}}}}"#, servers.join(", ")),
        );
        let x = &c.checks()["browseros_tools"];
        assert!(x.detail.contains("and 4 more"));
        assert!(!x.detail.contains("browseros-8"));
    }

    #[test]
    fn test_a_very_long_name_is_cut() {
        let c = EnvCase::new();
        c.user_json(&format!(
            r#"{{"mcpServers": {{"browseros{}": {{}}}}}}"#,
            "x".repeat(5000)
        ));
        assert!(c.checks()["browseros_tools"].detail.chars().count() < 600);
    }

    #[test]
    fn test_text_mode_prints_one_line_per_check_and_no_control_characters() {
        let c = EnvCase::new();
        c.user_json(&format!(
            r#"{{"mcpServers": {{{}: {{}}}}}}"#,
            hostile_json()
        ));
        let hostile2 = json::dumps(&Value::Str(format!("{HOSTILE}2")), &json::PY_DEFAULT);
        write(
            &c.cwd.join(".mcp.json"),
            &format!(r#"{{"mcpServers": {{{hostile2}: {{}}}}}}"#),
        );
        let (out, _) = c.main(false);
        assert!(!out.contains('\x1b'));
        assert!(!out.contains('\u{202e}'));
        // splitlines: \n, \r and the other line boundaries all end a line
        let lines: Vec<&str> = out
            .split(|ch: char| {
                matches!(
                    ch,
                    '\n' | '\r' | '\x0b' | '\x0c' | '\x1c'
                        ..='\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}'
                )
            })
            .filter(|l| !l.is_empty())
            .collect();
        let names: Vec<&str> = lines
            .iter()
            .filter(|l| !l.starts_with("duplicate:"))
            .map(|l| l.split_whitespace().next().unwrap_or(""))
            .collect();
        assert_eq!(names, doctor::CHECKS);
    }
}

mod percent_in_names_test {
    use super::*;

    #[test]
    fn test_plugin_key_with_a_percent_sign() {
        let c = EnvCase::new();
        let plug = c.tmp.join("bp");
        write(
            &plug.join(".mcp.json"),
            r#"{"mcpServers": {"browseros-neo": {}}}"#,
        );
        write(
            &c.config.join("plugins").join("installed_plugins.json"),
            &format!(
                r#"{{"plugins": {{"b%s%d@m": [{{"installPath": {}}}]}}}}"#,
                js(&plug)
            ),
        );
        let found: Vec<String> = c
            .find(&c.cwd)
            .into_iter()
            .filter(|s| s.scope == "plugin")
            .map(|s| s.tool_prefix)
            .collect();
        assert_eq!(found, ["mcp__plugin_b%s%d_browseros-neo__"]);
        let all = c.checks();
        assert!(!all["browseros_tools"].detail.starts_with("check failed"));
        assert!(!all["browseros"].detail.starts_with("check failed"));
    }

    #[test]
    fn test_server_name_with_a_percent_sign() {
        let c = EnvCase::new();
        c.user_json(r#"{"mcpServers": {"browseros%s%": {}}}"#);
        assert_eq!(c.find(&c.cwd)[0].tool_prefix, "mcp__browseros%s%__");
        assert!(
            !c.checks()["browseros_tools"]
                .detail
                .starts_with("check failed")
        );
    }
}

mod nested_installed_plugins_test {
    use super::*;

    fn nest(c: &EnvCase) {
        write(
            &c.config.join("plugins").join("installed_plugins.json"),
            &format!("{}{}", "[".repeat(200_000), "]".repeat(200_000)),
        );
    }

    fn data(c: &EnvCase) -> Value {
        json::loads(&c.main(true).0).expect("json output")
    }

    fn field<'a>(v: &'a Value, k: &str) -> &'a Value {
        v.as_object().and_then(|o| o.get(k)).expect("field")
    }

    #[test]
    fn test_text_mode_reports_unreadable() {
        let c = EnvCase::new();
        nest(&c);
        let (out, code) = c.main(false);
        assert_eq!(code, 0);
        assert!(out.contains("installed_plugins.json is unreadable"));
    }

    #[test]
    fn test_json_mode_reports_unreadable_and_no_duplicates() {
        let c = EnvCase::new();
        nest(&c);
        let v = data(&c);
        assert_eq!(field(&v, "duplicates"), &Value::Array(Vec::new()));
        assert_eq!(field(&v, "installed_plugins").as_str(), Some("unreadable"));
    }

    #[test]
    fn test_malformed_and_missing_files() {
        let c = EnvCase::new();
        let path = write(
            &c.config.join("plugins").join("installed_plugins.json"),
            "{nope",
        );
        assert_eq!(
            field(&data(&c), "installed_plugins").as_str(),
            Some("unreadable")
        );
        fs::remove_file(&path).expect("unlink");
        let v = data(&c);
        assert_eq!(field(&v, "installed_plugins").as_str(), Some("missing"));
        assert_eq!(field(&v, "duplicates"), &Value::Array(Vec::new()));
        assert!(!c.main(false).0.contains("unreadable"));
    }

    #[test]
    fn test_readable_file_is_ok() {
        let c = EnvCase::new();
        write(
            &c.config.join("plugins").join("installed_plugins.json"),
            INSTALLED,
        );
        let v = data(&c);
        assert_eq!(field(&v, "installed_plugins").as_str(), Some("ok"));
        assert_eq!(field(&v, "duplicates").as_array().map(Vec::len), Some(4));
    }

    #[test]
    fn test_a_nested_user_settings_file_does_not_crash_the_approval_check() {
        // a valid project .mcp.json makes doctor read the (nested) user settings.json
        let c = EnvCase::new();
        write(
            &c.cwd.join(".mcp.json"),
            r#"{"mcpServers": {"browseros": {}}}"#,
        );
        write(
            &c.config.join("settings.json"),
            &format!("{}{}", "[".repeat(200_000), "]".repeat(200_000)),
        );
        let all = c.checks();
        assert!(!all["browseros"].detail.starts_with("check failed"));
        assert!(!all["browseros_tools"].detail.starts_with("check failed"));
        assert!(all["browseros"].detail.contains("unapproved"));
    }

    #[test]
    fn test_nested_mcp_and_settings_files_do_not_crash_the_checks() {
        let c = EnvCase::new();
        nest(&c);
        let deep = "[".repeat(200_000);
        write(&c.cwd.join(".mcp.json"), &deep);
        write(&c.cwd.join(".claude").join("settings.local.json"), &deep);
        write(&c.config.join("settings.json"), &deep);
        let failed: Vec<Check> = c
            .checks()
            .into_values()
            .filter(|x| x.detail.starts_with("check failed"))
            .collect();
        assert_eq!(failed, []);
    }
}

mod duplicate_scope_test {
    use super::*;

    fn found(scope: &str, name: &str) -> Vec<doctor::Duplicate> {
        dups(&format!(
            r#"{{"plugins": {{{}: [{{"scope": {scope}, "installPath": "/p"}}]}}}}"#,
            json::dumps(&Value::Str(name.into()), &json::PY_DEFAULT)
        ))
    }

    fn s(text: &str) -> String {
        json::dumps(&Value::Str(text.into()), &json::PY_DEFAULT)
    }

    #[test]
    fn test_known_scopes_get_the_command() {
        for scope in ["user", "project", "local"] {
            assert_eq!(
                found(&s(scope), "superpowers@m")[0].command.as_deref(),
                Some(format!("claude plugin uninstall superpowers@m -s {scope}").as_str())
            );
        }
    }

    #[test]
    fn test_unknown_scope_gets_no_command_and_is_escaped() {
        let mut scopes: Vec<String> = [
            "user; touch x",
            "x\x1b[31m\nfake",
            "",
            " user",
            "USER",
            "managed",
        ]
        .iter()
        .map(|t| s(t))
        .collect();
        scopes.extend(["3".to_string(), r#"["user"]"#.to_string()]);
        for scope in &scopes {
            let d = found(scope, "superpowers@m");
            assert_eq!(d.len(), 1);
            assert_eq!(d[0].command, None, "{scope}");
            assert!(printable(&d[0].scope), "{scope}");
        }
    }

    #[test]
    fn test_the_escaped_scope_shows_the_odd_value() {
        let d = found(&s("x\x1b\nz"), "superpowers@m");
        assert!(d[0].scope.contains("\\x1b"));
        assert!(d[0].scope.contains("\\n"));
    }

    #[test]
    fn test_a_plugin_name_with_shell_characters_gets_no_command() {
        for name in [
            "superpowers@m; touch x",
            "superpowers@m$(id)",
            "superpowers@m x",
            "superpowers@m\nz",
        ] {
            let d = found(&s("user"), name);
            assert_eq!(d.len(), 1);
            assert_eq!(d[0].command, None, "{name:?}");
            assert!(printable(&d[0].plugin), "{name:?}");
        }
    }

    #[test]
    fn test_text_mode_prints_no_runnable_command_for_an_odd_scope() {
        let report = Report {
            // check_env patched to one check
            checks: vec![Check {
                name: "platform_supported",
                ok: Some(true),
                detail: "x".into(),
            }],
            duplicates: dups(
                r#"{"plugins": {"superpowers@m": [{"scope": "user; touch x"}, {"scope": "user"}]}}"#,
            ),
            installed_plugins: "ok",
        };
        let (out, code) = doctor::render(&report, false);
        assert_eq!(code, 0);
        assert_eq!(out.matches("claude plugin uninstall").count(), 1);
        assert!(out.contains("touch x")); // shown, escaped, with no command
    }
}

mod plugin_data_test {
    use super::*;

    fn state(c: &EnvCase, text: &str) {
        c.write_state(text);
    }

    #[test]
    fn test_not_recorded_is_none() {
        let c = EnvCase::new();
        assert_eq!(c.checks()["plugin_data"].ok, None);
    }

    #[test]
    fn test_recorded_and_present_is_ok() {
        let c = EnvCase::new();
        fs::create_dir(c.tmp.join("d")).expect("d");
        state(&c, &format!(r#"{{"data_dir": {}}}"#, js(&c.tmp.join("d"))));
        assert_eq!(c.checks()["plugin_data"].ok, Some(true));
    }

    #[test]
    fn test_recorded_but_gone_is_loud() {
        let c = EnvCase::new();
        state(
            &c,
            &format!(r#"{{"data_dir": {}}}"#, js(&c.tmp.join("gone"))),
        );
        let x = &c.checks()["plugin_data"];
        assert_eq!(x.ok, Some(false));
        assert!(x.detail.contains("gone"));
    }

    #[test]
    fn test_a_file_is_not_a_folder() {
        let c = EnvCase::new();
        write(&c.tmp.join("f"), "x");
        state(&c, &format!(r#"{{"data_dir": {}}}"#, js(&c.tmp.join("f"))));
        assert_eq!(c.checks()["plugin_data"].ok, Some(false));
    }

    #[test]
    fn test_env_wins_over_state() {
        let c = EnvCase::new();
        fs::create_dir(c.tmp.join("d")).expect("d");
        state(
            &c,
            &format!(r#"{{"data_dir": {}}}"#, js(&c.tmp.join("gone"))),
        );
        let d = c.tmp.join("d");
        let x = c.check(
            "plugin_data",
            &[("CLAUDE_PLUGIN_DATA", d.to_str().expect("utf-8"))],
            &plugin(),
        );
        assert_eq!(x.ok, Some(true));
    }
}

mod path_wrappers_test {
    use super::*;

    /// `fake_plugin()`: executable `bin/` wrappers (F-9: `anthology` too).
    fn fake_plugin(c: &EnvCase) -> PathBuf {
        let root = c.tmp.join("plug");
        for name in ["anthology", "anthology-mem", "atl", "atl-hook"] {
            let p = write(&root.join("bin").join(name), "#!/bin/sh\n");
            fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        root
    }

    #[test]
    fn test_the_real_plugin_has_its_wrappers() {
        let c = EnvCase::new();
        let sandbox = Sandbox::new();
        // the plugin as it ships after cutover: plugins/anthology with port/staging over it
        let root = sandbox.plugin_tree(Plugin::Anthology);
        let x = c.check("path_wrappers", &[], &root);
        assert_eq!(x.ok, Some(true), "{x:?}");
    }

    #[test]
    fn test_inside_claude_code_the_bin_must_be_on_path() {
        let c = EnvCase::new();
        let root = fake_plugin(&c);
        let bad = c.check("path_wrappers", &[("CLAUDECODE", "1")], &root);
        assert_eq!(bad.ok, Some(false));
        assert!(bad.detail.contains("bin/ is not on PATH"));
        let bin = root.join("bin");
        let deps = DoctorDeps {
            which: Box::new(move |n, _| Some(bin.join(n))),
            ..c.deps()
        };
        let env = c.env.clone().with("CLAUDECODE", "1");
        assert_eq!(
            c.check_deps("path_wrappers", &env, &root, &deps).ok,
            Some(true)
        );
    }

    #[test]
    fn doctor_warns_when_atl_on_path_is_not_the_plugin_shim() {
        // Another `atl` earlier on PATH would run instead of the plugin's bin/atl.
        let c = EnvCase::new();
        let root = fake_plugin(&c);
        let bin = root.join("bin");
        let other = write(&c.tmp.join("other").join("atl"), "x");
        let deps = DoctorDeps {
            which: Box::new(move |n, _| {
                Some(if n == "atl" {
                    other.clone()
                } else {
                    bin.join(n)
                })
            }),
            ..c.deps()
        };
        let env = c.env.clone().with("CLAUDECODE", "1");
        let x = c.check_deps("path_wrappers", &env, &root, &deps);
        assert_eq!(x.ok, Some(false), "{x:?}");
        assert!(
            x.detail.contains("atl on PATH is not the plugin's"),
            "{x:?}"
        );
    }

    #[test]
    fn test_inside_claude_code_another_anthology_mem_on_path_is_not_ours() {
        let c = EnvCase::new();
        let root = fake_plugin(&c);
        let other = write(&c.tmp.join("other").join("anthology-mem"), "x");
        let deps = DoctorDeps {
            which: Box::new(move |_, _| Some(other.clone())),
            ..c.deps()
        };
        let env = c.env.clone().with("CLAUDECODE", "1");
        assert_eq!(
            c.check_deps("path_wrappers", &env, &root, &deps).ok,
            Some(false)
        );
    }
}

mod subagent_model_test {
    use super::*;

    /// The `doctor.find_mcp_servers` half (the compat half is RT1a's): a `.mcp.json` and a
    /// `.claude/settings.json` linked to `/dev/zero` are skipped without reading them.
    #[test]
    fn test_a_device_symlink_is_skipped_without_reading_it() {
        if !Path::new("/dev/zero").exists() {
            return; // needs /dev/zero
        }
        let c = EnvCase::new();
        fs::create_dir(c.cwd.join(".claude")).expect("mkdir");
        std::os::unix::fs::symlink("/dev/zero", c.cwd.join(".claude").join("settings.json"))
            .expect("link");
        std::os::unix::fs::symlink("/dev/zero", c.cwd.join(".mcp.json")).expect("link");
        let env = Env::default().with_os("HOME", &c.tmp);
        assert_eq!(compat::subagent_model_source(&env, &c.cwd), None);
        assert_eq!(doctor::find_mcp_servers(&env, &c.cwd, &c.deps().limits), []);
    }
}

/// Regression tests from the RT1b review (not ledger targets): the `anthology_binary` wiring
/// (F-5), the unknown plugin root, and Python's reading of project `.mcp.json` files.
mod review_test {
    use super::*;
    use anthology_core::lock;
    use anthology_core::platform::Target;
    use anthology_runtime::bootstrap;

    const VERSION: &str = "0.1.0";
    const BINARY: &[u8] = b"\x7fELF fake anthology binary";
    const TARBALL: &[u8] = b"anthology tarball";

    /// `plugins/anthology` and its guardrails sibling with a valid anthology.lock; with
    /// `install`, a current `<ah>/bin/<version>/` too. Returns the anthology plugin root.
    fn guardrails(c: &EnvCase, install: bool) -> PathBuf {
        let root = c.tmp.join("plugins").join("anthology");
        fs::create_dir_all(&root).expect("plugin");
        let guard = c.tmp.join("plugins").join("anthology-guardrails");
        write(&guard.join("hooks").join("guard.sh"), "#!/bin/sh\n");
        let sha256 = Target::ALL
            .into_iter()
            .map(|t| (t, release::sha256_hex(TARBALL)))
            .collect();
        let lock = Lock {
            version: VERSION.into(),
            tag: format!("anthology-bin--v{VERSION}"),
            repo: "example/anthology".into(),
            sha256,
        };
        write(
            &guard.join("anthology.lock"),
            &lock::render_anthology(&lock),
        );
        if install {
            let vdir = c.config.join("anthology").join("bin").join(VERSION);
            write_bytes(&vdir.join("anthology"), BINARY);
            fs::set_permissions(vdir.join("anthology"), fs::Permissions::from_mode(0o755))
                .expect("chmod");
            write(
                &vdir.join(".bin-sha256"),
                &format!("{}\n", release::sha256_hex(BINARY)),
            );
            write(
                &vdir.join(".sha256"),
                &format!("{}\n", release::sha256_hex(TARBALL)),
            );
        }
        root
    }

    fn binary(c: &EnvCase, root: &Path) -> Check {
        c.check("anthology_binary", &[], root)
    }

    #[test]
    fn anthology_binary_matches_the_lock_through_the_live_guardrails() {
        let c = EnvCase::new();
        let root = guardrails(&c, true);
        let x = binary(&c, &root);
        assert_eq!((x.ok, x.detail.as_str()), (Some(true), bootstrap::CHECK_OK));
    }

    #[test]
    fn anthology_binary_not_installed_and_tampered() {
        let c = EnvCase::new();
        let root = guardrails(&c, false);
        let x = binary(&c, &root);
        assert_eq!(x.ok, Some(false));
        let want = format!("the guardrails' anthology binary ({VERSION}) is not installed");
        assert!(x.detail.starts_with(&want), "{x:?}");
        let root = guardrails(&c, true);
        let bin = c
            .config
            .join("anthology/bin")
            .join(VERSION)
            .join("anthology");
        fs::write(&bin, b"\x7fELF swapped").expect("tamper");
        let x = binary(&c, &root);
        assert_eq!(x.ok, Some(false));
        assert!(
            x.detail.contains("does not match its install marker"),
            "{x:?}"
        );
    }

    /// Early whole-branch review M6 and M7: doctor reads `<ah>` and the markers as `lib.sh`
    /// does.
    #[test]
    fn anthology_binary_reads_the_install_as_the_shims_do() {
        let c = EnvCase::new();
        let root = guardrails(&c, true);
        let vdir = c.config.join("anthology/bin").join(VERSION);
        // the first line of a marker counts, as `anth_first_line` reads it
        let tarball = release::sha256_hex(TARBALL);
        write(&vdir.join(".sha256"), &format!("{tarball}\njunk\n"));
        let x = binary(&c, &root);
        assert_eq!((x.ok, x.detail.as_str()), (Some(true), bootstrap::CHECK_OK));
        write(&vdir.join(".sha256"), &format!("{tarball} \n"));
        assert_eq!(binary(&c, &root).ok, Some(false));
        write(&vdir.join(".sha256"), &tarball);
        assert_eq!(binary(&c, &root).ok, Some(true));
        // a relative, empty or unset <ah> base is refused
        for env in [
            c.env.clone().with("CLAUDE_CONFIG_DIR", "config"),
            c.env.clone().with("CLAUDE_CONFIG_DIR", "").with("HOME", ""),
            c.env.clone().without("CLAUDE_CONFIG_DIR").without("HOME"),
            c.env
                .clone()
                .with("CLAUDE_CONFIG_DIR", "")
                .with("HOME", "home"),
        ] {
            let all = c.checks_with(&env, &root, &c.cwd, &c.deps());
            let x = &all["anthology_binary"];
            assert_eq!(
                (x.ok, x.detail.as_str()),
                (Some(false), bootstrap::CHECK_NO_HOME),
                "{env:?}"
            );
        }
        // a symlinked <ah>/bin is refused, as `anth_installed` refuses it
        let bin = c.config.join("anthology/bin");
        let real = c.tmp.join("real-bin");
        fs::rename(&bin, &real).expect("move");
        std::os::unix::fs::symlink(&real, &bin).expect("link");
        let x = binary(&c, &root);
        assert_eq!(x.ok, Some(false));
        assert!(x.detail.contains("is not installed"), "{x:?}");
    }

    #[test]
    fn anthology_binary_still_checks_a_disabled_guardrails_and_names_a_missing_one() {
        let c = EnvCase::new();
        let root = guardrails(&c, true);
        write(
            &c.config.join("settings.json"),
            r#"{"enabledPlugins": {"anthology-guardrails@anthology": false}}"#,
        );
        assert_eq!(binary(&c, &root).ok, Some(true));
        let lone = c.tmp.join("lone").join("anthology");
        fs::create_dir_all(&lone).expect("lone");
        let x = binary(&c, &lone);
        assert_eq!(
            (x.ok, x.detail.as_str()),
            (None, bootstrap::CHECK_NOT_FOUND)
        );
    }

    #[test]
    fn an_unknown_plugin_root_is_never_guessed() {
        // a guardrails-like folder in the working folder must not be picked up
        let c = EnvCase::new();
        let fake = c.cwd.join("anthology-guardrails");
        write(&fake.join("hooks").join("guard.sh"), "#!/bin/sh\n");
        write(&fake.join("anthology.lock"), "{}");
        let all = c.checks_with(&c.env, Path::new(""), &c.cwd, &c.deps());
        for name in ["anthology_binary", "atl", "path_wrappers"] {
            let got = (all[name].ok, all[name].detail.as_str());
            assert_eq!(got, (None, doctor::UNKNOWN_ROOT), "{name}");
        }
        c.write_state(r#"{"setup_done": "0.1.0"}"#);
        let all = c.checks_with(&c.env, Path::new(""), &c.cwd, &c.deps());
        assert_eq!(all["plugin_read_rule"].detail, doctor::UNKNOWN_ROOT);
        // CLAUDE_PLUGIN_ROOT stands in for the argument
        let env = c.env.clone().with_os("CLAUDE_PLUGIN_ROOT", plugin());
        let all = c.checks_with(&env, Path::new(""), &c.cwd, &c.deps());
        assert_ne!(all["path_wrappers"].detail, doctor::UNKNOWN_ROOT);
    }

    #[test]
    fn a_null_project_mcp_json_counts_as_absent() {
        let c = EnvCase::new();
        write(&c.cwd.join(".mcp.json"), "null");
        write(&c.tmp.join(".mcp.json"), r#"{"mcpServers": {"outer": {}}}"#);
        let names: Vec<String> = c.find(&c.cwd).into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["outer"]);
    }

    #[test]
    fn repo_files_nest_as_deep_as_python_reads_them() {
        let c = EnvCase::new();
        let deep = format!("{}{}", "[".repeat(600), "]".repeat(600));
        write(
            &c.cwd.join(".mcp.json"),
            &format!(r#"{{"mcpServers": {{"inner": {{}}}}, "x": {deep}}}"#),
        );
        write(&c.tmp.join(".mcp.json"), r#"{"mcpServers": {"outer": {}}}"#);
        // the 600-deep file is read, so the walk stops there
        assert_eq!(c.loaded(), loaded(&[("inner", false)]));
        write(
            &c.config.join("settings.json"),
            r#"{"enableAllProjectMcpServers": true}"#,
        );
        assert_eq!(c.loaded(), loaded(&[("inner", true)]));
        c.user_json(&format!(
            r#"{{"projects": {{{}: {{"hasTrustDialogAccepted": true}}}}}}"#,
            js(&c.cwd)
        ));
        write(
            &c.cwd.join(".claude").join("settings.json"),
            &format!(r#"{{"disabledMcpjsonServers": ["inner"], "x": {deep}}}"#),
        );
        // the deep repo settings still disable it
        assert_eq!(c.loaded(), loaded(&[("inner", false)]));
    }
}
