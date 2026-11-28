//! The `doctor` difftest surface (RT1b): `anthology_runtime::doctor` against
//! `plugins/anthology/scripts/doctor.py` on generated environments.
//!
//! Oracle: `port/difftest/oracle/doctor.py`. Its module docs give the case format and the
//! fixed seams: the application folder and managed MCP file are under the root, the real
//! `shutil.which` and `subprocess.run` run fake tools from the tree on the case's `PATH`, and
//! two forced changes are modeled on the Python side or masked on both:
//! - F-5: the first check (`python3` there, `anthology_binary` here) is masked on both sides
//!   (`{"name": "<F-5>", ...}`, or the text line `<F-5>`). `anthology_binary` is covered by
//!   RT2's `bootstrap` tests and `tests/doctor.rs`.
//! - F-9: the oracle's `compat.wrapper_problems` also checks `bin/anthology`, as Rust does.
//!
//! The layout is built by `anthology_testkit::rt_a::materialize` here and by its twin there,
//! each under its own temporary root; every root prefix in the output is `@ROOT@`. Ops:
//! `check_env`, `main` (text and `--json`, exit code included), `find_mcp_servers` and
//! `duplicates` (`find_duplicates` on a generated `installed_plugins.json` value).
//!
//! Seeds are the layouts of `tests/test_doctor.py`. Generated cases draw from a vocabulary aimed
//! at the checks' branches: the config folder set, unset or empty; `PATH` with fake `node`,
//! `docker` (exit 0, 1 or 3, not executable, a folder, not a program), BrowserOS binaries and
//! `anthology-mem`; apps and AppImages (folders, files, near-miss names) in the three roots;
//! `.claude.json` (user servers, `projects` entries for the working folder, its ancestors and
//! elsewhere, with every approval and trust key and odd value types), a project `.mcp.json` at
//! three heights around a git root (or none), user and repo settings (approval lists, the
//! plugin-read rule, the subagent model; UTF-16, garbage, oversized), installed plugins with
//! their own `.mcp.json` and manifests (flat and nested forms), managed servers, hostile, long,
//! `%` and many server names; state.json (`setup_done`, `atl_path`, `data_dir` of every shape);
//! an atl.lock that is valid, invalid or missing with an install that matches, is tampered or
//! not executable; the plugin's `bin/` wrappers; agent-memory folders; duplicate installs with
//! odd scopes and keys; atl's history.db (absent, not a database, a folder, a table missing, or
//! built here with token values of every SQLite type and size).
//!
//! The generator never builds a layout the two sides cannot agree on by design: no lone
//! surrogates (D-6), no FIFOs or devices read unbounded, no `docker` that hangs (the 10 s
//! timeout is unit-tested through the `run` seam), no `PATH` entry that is empty (it would look
//! in each process's own working folder), no file whose mode leaves its owner without access
//! (`os.access` is approximated by "some execute bit", RT1a), no deep JSON in a container
//! `atl_path` or `data_dir` (Python's `str()` raises `RecursionError` there: D-1), and no repo
//! settings file 513 to 9,997 levels deep that sets the subagent model (compat's read refuses
//! that band on purpose, RT1a's ruling; doctor's own repo reads parse it, as Python does, and
//! the generator does build such files with approval keys, and project `.mcp.json` files that
//! deep, `null`, or with a second one higher up).

use std::path::{Path, PathBuf};

use anthology_core::Env;
use anthology_pycompat::json::{self, Int, Map, Value};
use anthology_runtime::doctor::{self, DoctorDeps, DoctorLimits};
use anthology_testkit::release::sha256_hex;
use anthology_testkit::rt_a::{self, RealTemp};

use crate::case::{Case, Rng, Surface};
use crate::observe::{Observed, from_oracle};
use crate::oracle::Oracle;

/// The surface name (plan §1.4).
pub const NAME: &str = "doctor";

/// Whether `difftest --all` runs this surface on macOS too (`ci/jobs/difftest-macos.sh`): yes,
/// every layout and fake tool exists there too (it is a CLI surface, D§13.2).
pub const ON_MACOS: bool = true;

/// The registered implementation.
pub fn surface() -> Option<Box<dyn Surface>> {
    Some(Box::new(Doctor))
}

struct Doctor;

impl Surface for Doctor {
    fn name(&self) -> &'static str {
        NAME
    }

    fn seeds(&self) -> Vec<Case> {
        seeds()
            .into_iter()
            .enumerate()
            .map(|(i, data)| Case::new(format!("seed:{}", i + 1), data))
            .collect()
    }

    fn generate(&self, rng: &mut Rng, n: usize) -> Vec<Case> {
        (0..n)
            .map(|i| Case::new(format!("gen:{}", i + 1), gen_case(rng)))
            .collect()
    }

    fn run_python(&self, c: &Case, oracle: &mut Oracle) -> Observed {
        from_oracle(&oracle.call("doctor", &c.data))
    }

    fn run_rust(&self, c: &Case) -> Observed {
        run(&c.data)
    }
}

// ---------------------------------------------------------------------------------------------
// The Rust side

fn field<'a>(case: &'a Value, k: &str) -> Option<&'a Value> {
    case.as_object().and_then(|o| o.get(k))
}

fn arg(case: &Value, k: &str, root: &Path) -> PathBuf {
    let s = field(case, k)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("doctor: no {k:?}"));
    PathBuf::from(rt_a::expand(s, root, None))
}

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    let mut m = Map::new();
    for (k, v) in pairs {
        m.insert(k.to_string(), v);
    }
    Value::Object(m)
}

fn s(x: &str) -> Value {
    Value::Str(x.to_string())
}

fn mask() -> Value {
    obj(vec![
        ("name", s("<F-5>")),
        ("ok", Value::Null),
        ("detail", s("")),
    ])
}

fn masked(checks: &[doctor::Check]) -> Value {
    let mut out = vec![mask()];
    out.extend(checks.iter().skip(1).map(doctor::check_value));
    Value::Array(out)
}

fn dump(v: &Value) -> String {
    json::dumps(v, &json::INDENT2)
}

fn run(case: &Value) -> Observed {
    let tmp = RealTemp::new();
    let root = tmp.path.clone();
    let tree = field(case, "tree")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    rt_a::materialize(&root, &tree);
    let mut env = Env::default();
    if let Some(vars) = field(case, "env").and_then(Value::as_object) {
        for (k, v) in vars.iter() {
            env = env.with(
                k,
                &rt_a::expand(v.as_str().expect("env value"), &root, None),
            );
        }
    }
    let limits = DoctorLimits {
        system_applications: vec![root.join("SysApps")],
        managed_mcp_paths: vec![root.join("managed-mcp.json")],
        managed_settings_paths: vec![root.join("managed-settings.json")],
    };
    let lock = field(case, "lock")
        .and_then(Value::as_str)
        .map(|l| PathBuf::from(rt_a::expand(l, &root, None)));
    let deps = DoctorDeps {
        atl_lock: lock,
        limits: limits.clone(),
        ..DoctorDeps::default()
    };
    let op = field(case, "op").and_then(Value::as_str).expect("op");
    let (exit, stdout) = match op {
        "check_env" => {
            let checks = doctor::check_env(
                &env,
                &arg(case, "plugin_root", &root),
                &arg(case, "cwd", &root),
                &deps,
            );
            (0, dump(&masked(&checks)))
        }
        "find_mcp_servers" => {
            let servers = doctor::find_mcp_servers(&env, &arg(case, "cwd", &root), &limits);
            let list = servers
                .iter()
                .map(|x| {
                    let mut pairs = vec![
                        ("name", s(&x.name)),
                        ("scope", s(x.scope)),
                        ("tool_prefix", s(&x.tool_prefix)),
                    ];
                    if let Some(a) = x.approved {
                        pairs.push(("approved", Value::Bool(a)));
                    }
                    obj(pairs)
                })
                .collect();
            (0, dump(&Value::Array(list)))
        }
        "duplicates" => {
            let found = doctor::find_duplicates_in(field(case, "installed"));
            let list = found.iter().map(doctor::duplicate_value).collect();
            (0, dump(&Value::Array(list)))
        }
        "main" => {
            let as_json = field(case, "json").and_then(Value::as_bool) == Some(true);
            let report = doctor::report(&env, Path::new(""), &arg(case, "cwd", &root), &deps);
            let (out, code) = doctor::render(&report, as_json);
            let shown = if as_json {
                let mut data = json::loads(&out).expect("main's JSON");
                if out != dump(&data) + "\n" {
                    // never: render prints exactly this dump
                    format!("main's JSON is not json.dumps(indent=2):\n{out}")
                } else {
                    if let Some(checks) = data
                        .as_object_mut()
                        .and_then(|o| o.get_mut("checks"))
                        .and_then(Value::as_array_mut)
                        && !checks.is_empty()
                    {
                        checks[0] = mask();
                    }
                    dump(&data) + "\n"
                }
            } else {
                let mut lines: Vec<&str> = out.split('\n').collect();
                if lines.first().is_some_and(|l| !l.is_empty()) {
                    lines[0] = "<F-5>";
                }
                lines.join("\n")
            };
            (code, shown)
        }
        other => panic!("doctor: unknown op {other:?}"),
    };
    Observed {
        exit,
        stdout: stdout
            .replace(&*root.to_string_lossy(), "@ROOT@")
            .into_bytes(),
        stderr: Vec::new(),
        files: None,
    }
}

// ---------------------------------------------------------------------------------------------
// Building cases

const PLUG: &str = "plug";
const PROJ: &str = "w/proj";
const SUB: &str = "w/proj/sub";
const CONFIG: &str = "config";
const HOME: &str = "home";
const ATL_VERSION: &str = "1.2.1-anthology.1";
/// What the fake atl tarball hashes to; every target of the generated lock pins it.
const ATL_BLOB: &[u8] = b"atl tarball";
const HOSTILE: &str = "browseros\x1b[31m\nFAKE  ok   forged line\r\u{202e}";
const TOOL_EXIT: &str = "#!/bin/sh\nexit ";

fn text(v: &Value) -> String {
    json::dumps(v, &json::PY_DEFAULT)
}

fn int(i: i64) -> Value {
    Value::Int(Int::Small(i))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// A case under construction.
struct B {
    tree: Vec<Value>,
    env: Vec<(String, String)>,
    /// Where `.claude.json` and settings go: the config folder, and the base of `.claude.json`.
    config: String,
    base: String,
}

impl B {
    fn new() -> B {
        B {
            tree: Vec::new(),
            env: vec![
                ("HOME".into(), format!("@ROOT@/{HOME}")),
                ("CLAUDE_CONFIG_DIR".into(), format!("@ROOT@/{CONFIG}")),
                ("PATH".into(), "@ROOT@/bin".into()),
                ("CLAUDE_PLUGIN_ROOT".into(), format!("@ROOT@/{PLUG}")),
            ],
            config: CONFIG.into(),
            base: CONFIG.into(),
        }
    }
    fn push(&mut self, pairs: Vec<(&str, Value)>) -> &mut B {
        self.tree.push(obj(pairs));
        self
    }
    fn dir(&mut self, p: &str) -> &mut B {
        self.push(vec![("t", s("dir")), ("p", s(p))])
    }
    fn file(&mut self, p: &str, t: &str) -> &mut B {
        self.push(vec![("t", s("file")), ("p", s(p)), ("text", s(t))])
    }
    fn file_enc(&mut self, p: &str, t: &str, enc: &str) -> &mut B {
        self.push(vec![
            ("t", s("file")),
            ("p", s(p)),
            ("text", s(t)),
            ("enc", s(enc)),
        ])
    }
    fn file_hex(&mut self, p: &str, b: &[u8]) -> &mut B {
        self.push(vec![("t", s("file")), ("p", s(p)), ("hex", s(&hex(b)))])
    }
    fn file_pad(&mut self, p: &str, t: &str, key: &str, n: i64) -> &mut B {
        self.push(vec![
            ("t", s("file")),
            ("p", s(p)),
            ("text", s(t)),
            (key, int(n)),
        ])
    }
    fn chmod(&mut self, p: &str, mode: i64) -> &mut B {
        self.push(vec![("t", s("chmod")), ("p", s(p)), ("mode", int(mode))])
    }
    fn link(&mut self, p: &str, to: &str) -> &mut B {
        self.push(vec![("t", s("link")), ("p", s(p)), ("to", s(to))])
    }
    fn exe(&mut self, p: &str, body: &str) -> &mut B {
        self.file(p, body).chmod(p, 0o755)
    }
    fn env(&mut self, k: &str, v: &str) -> &mut B {
        self.env.retain(|(x, _)| x != k);
        self.env.push((k.into(), v.into()));
        self
    }
    fn unset(&mut self, k: &str) -> &mut B {
        self.env.retain(|(x, _)| x != k);
        self
    }
    /// No `CLAUDE_CONFIG_DIR`: the config is `~/.claude`, `.claude.json` is `~/.claude.json`.
    fn home_config(&mut self) -> &mut B {
        self.unset("CLAUDE_CONFIG_DIR");
        self.config = format!("{HOME}/.claude");
        self.base = HOME.into();
        self
    }
    fn claude_json(&mut self, t: &str) -> &mut B {
        let p = format!("{}/.claude.json", self.base);
        self.file(&p, t)
    }
    fn settings(&mut self, t: &str) -> &mut B {
        let p = format!("{}/settings.json", self.config);
        self.file(&p, t)
    }
    fn state(&mut self, t: &str) -> &mut B {
        let p = format!("{}/anthology/state.json", self.config);
        self.file(&p, t)
    }
    fn installed(&mut self, t: &str) -> &mut B {
        let p = format!("{}/plugins/installed_plugins.json", self.config);
        self.file(&p, t)
    }
    /// The plugin's four `bin/` wrappers, executable.
    fn wrappers(&mut self) -> &mut B {
        for n in ["anthology", "anthology-mem", "atl", "atl-hook"] {
            self.exe(&format!("{PLUG}/bin/{n}"), "#!/bin/sh\n");
        }
        self
    }
    fn tools(&mut self, names: &[&str]) -> &mut B {
        for n in names {
            let body = if *n == "docker" {
                format!("{TOOL_EXIT}0\n")
            } else {
                "#!/bin/sh\n".to_string()
            };
            self.exe(&format!("bin/{n}"), &body);
        }
        self
    }
    fn case(&self, op: &str, args: Vec<(&str, Value)>) -> Value {
        let mut env = Map::new();
        for (k, v) in &self.env {
            env.insert(k.clone(), s(v));
        }
        let mut pairs = vec![
            ("op", s(op)),
            ("tree", Value::Array(self.tree.clone())),
            ("env", Value::Object(env)),
        ];
        pairs.extend(args);
        obj(pairs)
    }
    fn check_env(&self, cwd: &str) -> Value {
        self.case(
            "check_env",
            vec![
                ("plugin_root", s(&format!("@ROOT@/{PLUG}"))),
                ("cwd", s(&format!("@ROOT@/{cwd}"))),
                ("lock", s(&format!("@ROOT@/{PLUG}/atl.lock"))),
            ],
        )
    }
    fn main(&self, as_json: bool) -> Value {
        self.case(
            "main",
            vec![
                ("cwd", s(&format!("@ROOT@/{PROJ}"))),
                ("json", Value::Bool(as_json)),
            ],
        )
    }
    fn find(&self, cwd: &str) -> Value {
        self.case(
            "find_mcp_servers",
            vec![("cwd", s(&format!("@ROOT@/{cwd}")))],
        )
    }
}

/// The base of every case: home, config, the working folder and `PATH` with node and docker.
fn base() -> B {
    let mut b = B::new();
    b.dir(HOME).dir(CONFIG).dir(PROJ).dir(PLUG);
    b.tools(&["python3", "node", "docker"]);
    b
}

fn dup(installed: &str) -> Value {
    obj(vec![
        ("op", s("duplicates")),
        ("tree", Value::Array(Vec::new())),
        ("env", Value::Object(Map::new())),
        ("installed", json::loads(installed).expect("installed json")),
    ])
}

/// The atl.lock text every target of which pins [`ATL_BLOB`].
fn atl_lock_text(version: &str) -> String {
    let h = sha256_hex(ATL_BLOB);
    let mut sha = Map::new();
    for t in [
        "aarch64-apple-darwin",
        "x86_64-apple-darwin",
        "x86_64-unknown-linux-musl",
        "aarch64-unknown-linux-musl",
    ] {
        sha.insert(t.into(), s(&h));
    }
    let v = obj(vec![
        ("version", s(version)),
        ("tag", s(&format!("atl--v{version}"))),
        ("repo", s("example/anthology")),
        ("sha256", Value::Object(sha)),
    ]);
    json::dumps(&v, &json::INDENT2) + "\n"
}

/// An atl install at `data/atl/<version>/`: `exec` binary or not, marker matching or not.
fn atl_install(b: &mut B, data: &str, version: &str, exec: bool, marker_ok: bool) {
    let bin = format!("{data}/atl/{version}/atl");
    b.file(&bin, "#!/bin/sh\n");
    b.chmod(&bin, if exec { 0o755 } else { 0o644 });
    let marker = if marker_ok {
        sha256_hex(ATL_BLOB)
    } else {
        "0".repeat(64)
    };
    b.file(
        &format!("{data}/atl/{version}/.sha256"),
        &format!("{marker}\n"),
    );
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

fn servers(names: &[&str]) -> String {
    let parts: Vec<String> = names
        .iter()
        .map(|n| format!("{}: {{\"url\": \"http://SECRETHOST\"}}", text(&s(n))))
        .collect();
    format!("{{\"mcpServers\": {{{}}}}}", parts.join(", "))
}

/// The Python tests' layouts (`tests/test_doctor.py`), each as one case.
fn seeds() -> Vec<Value> {
    let mut out = Vec::new();
    let proj = PROJ;
    // FindDuplicatesTest / DuplicateScopeTest
    out.push(dup(INSTALLED));
    for odd in [
        "null",
        "[]",
        "{}",
        r#"{"plugins": []}"#,
        r#"{"plugins": {"superpowers@x": "bad"}}"#,
        r#"{"plugins": {"superpowers@x": [null, 3]}}"#,
        r#"{"plugins": {"superpowers@m": [{"installPath": "/p"}]}}"#,
    ] {
        out.push(dup(odd));
    }
    for scope in [
        r#""user""#,
        r#""project""#,
        r#""local""#,
        r#""user; touch x""#,
        r#""x\u001b[31m\nfake""#,
        r#""""#,
        r#"" user""#,
        r#""USER""#,
        r#""managed""#,
        "3",
        r#"["user"]"#,
        "null",
        "true",
        "1.5",
        r#"{"a": [1, "b"]}"#,
    ] {
        out.push(dup(&format!(
            r#"{{"plugins": {{"superpowers@m": [{{"scope": {scope}, "installPath": "/p"}}]}}}}"#
        )));
    }
    for name in [
        "superpowers@m; touch x",
        "superpowers@m$(id)",
        "superpowers@m x",
        "superpowers@m\nz",
        "superpowers",
        "design-skills@a@b",
        "superpowers@",
        "superpowers@-m",
    ] {
        out.push(dup(&format!(
            r#"{{"plugins": {{{}: [{{"scope": "user"}}]}}}}"#,
            text(&s(name))
        )));
    }
    // CheckEnvTest
    let b = base();
    out.push(b.check_env(proj));
    out.push(b.main(false));
    out.push(b.main(true));
    let mut b = B::new();
    b.dir(HOME).dir(CONFIG).dir(PROJ).dir(PLUG); // no tools at all
    out.push(b.check_env(proj));
    for code in ["1", "3"] {
        let mut b = base();
        b.exe("bin/docker", &format!("{TOOL_EXIT}{code}\n"));
        out.push(b.check_env(proj));
    }
    let mut b = base();
    b.claude_json(r#"{"mcpServers": {"browseros-neo": {"type": "http", "url": "http://127.0.0.1:1/mcp?token=SECRET", "headers": {"Authorization": "Bearer SECRET"}}}}"#);
    out.push(b.check_env(proj));
    let mut b = base();
    b.dir("SysApps/BrowserOS.app");
    out.push(b.check_env(proj));
    let mut b = base();
    b.tools(&["browseros"]);
    out.push(b.check_env(proj));
    let mut b = base();
    b.home_config()
        .claude_json(r#"{"mcpServers": {"my-browseros": {}}}"#);
    out.push(b.check_env(proj));
    let mut b = base();
    b.claude_json("{nope");
    out.push(b.check_env(proj));
    // atl: through the lock and the marker
    for (exec, marker_ok, version, state_ver) in [
        (true, true, ATL_VERSION, ATL_VERSION),
        (false, true, ATL_VERSION, ATL_VERSION),
        (true, false, ATL_VERSION, ATL_VERSION),
        (true, true, ATL_VERSION, "0.9.0"),
        (true, true, "1.3.0-anthology.1", ATL_VERSION),
    ] {
        let mut b = base();
        b.file(&format!("{PLUG}/atl.lock"), &atl_lock_text(version));
        atl_install(&mut b, "data", state_ver, exec, marker_ok);
        b.state(&format!(
            r#"{{"atl_path": "@ROOT@/data/atl/{state_ver}/atl", "data_dir": "@ROOT@/data"}}"#
        ));
        out.push(b.check_env(proj));
        b.state(&format!(
            r#"{{"atl_path": "@ROOT@/data/atl/{state_ver}/atl"}}"#
        ));
        out.push(b.check_env(proj));
    }
    let mut b = base();
    b.file(&format!("{PLUG}/atl.lock"), &atl_lock_text(ATL_VERSION))
        .state(r#"{"atl_path": "@ROOT@/gone/atl"}"#);
    out.push(b.check_env(proj));
    let mut b = base();
    b.file(&format!("{PLUG}/atl.lock"), "{}");
    out.push(b.check_env(proj));
    let mut b = base();
    b.dir(&format!("{PLUG}/atl.lock"));
    out.push(b.check_env(proj));
    // plugin_read_rule
    let mut b = base();
    b.state(r#"{"setup_done": "0.1.0"}"#);
    out.push(b.check_env(proj));
    for settings in [
        r#"{"permissions": {"allow": ["Read(//old/**)"]}}"#,
        r#"{"permissions": {"allow": ["Read(/@ROOT@/**)"]}}"#,
        "{nope",
        "",
        r#"{"permissions": {"allow": "Read(/@ROOT@/**)"}}"#,
    ] {
        let mut b = base();
        b.state(r#"{"setup_done": "0.1.0"}"#).settings(settings);
        out.push(b.check_env(proj));
    }
    // BrowserosAppTest
    for (p, dir) in [
        ("SysApps/BrowserOS neo.app", true),
        ("home/Applications/BrowserOS.app", true),
        ("SysApps/Foo.app", true),
        ("SysApps/browseros.app", true),
        ("SysApps/BrowserOS neo.app", false),
        ("home/.local/bin/BrowserOS-1.2.AppImage", false),
        ("home/Applications/BrowserOS.AppImage", false),
        ("home/Applications/BrowserOS.AppImage", true),
        ("SysApps/BrowserOS.App", true),
        ("SysApps/BrowserOS", true),
    ] {
        let mut b = base();
        if dir {
            b.dir(p);
        } else {
            b.file(p, "x");
        }
        out.push(b.check_env(proj));
    }
    for name in ["browseros", "browseros-neo", "BrowserOS"] {
        let mut b = B::new();
        b.dir(HOME)
            .dir(CONFIG)
            .dir(PROJ)
            .dir(PLUG)
            .tools(&["python3", name]);
        out.push(b.check_env(proj));
    }
    // FindMcpServersTest
    let mut b = base();
    b.claude_json(
        r#"{"mcpServers": {"browseros-neo": {"url": "http://127.0.0.1:9/mcp?token=SECRET"}}}"#,
    );
    out.push(b.find(proj));
    let mut b = base();
    b.home_config().claude_json(r#"{"mcpServers": {"a": {}}}"#);
    out.push(b.find(proj));
    let mut b = base();
    b.dir(&format!("{proj}/a/b")).claude_json(&format!(
        r#"{{"projects": {{"@ROOT@/{proj}": {{"mcpServers": {{"loc": {{"command": "SECRETCMD"}}}}}}, "@ROOT@/other": {{"mcpServers": {{"nope": {{}}}}}}}}}}"#
    ));
    out.push(b.find(&format!("{proj}/a/b")));
    let mut b = base();
    b.dir(&format!("{proj}/a/b")).file(
        &format!("{proj}/.mcp.json"),
        r#"{"mcpServers": {"proj": {"args": ["SECRETARG"]}}}"#,
    );
    out.push(b.find(&format!("{proj}/a/b")));
    let mut b = base();
    b.file("w/.mcp.json", r#"{"mcpServers": {"outside": {}}}"#)
        .dir(&format!("{proj}/.git"))
        .dir(SUB);
    out.push(b.find(SUB));
    let mut b = base();
    b.file("p1/.mcp.json", r#"{"mcpServers": {"srv": {"url": "http://SECRETHOST"}}}"#)
        .file("p2/.mcp.json", r#"{"flat": {"command": "x"}}"#)
        .file("p2/.claude-plugin/plugin.json", r#"{"mcpServers": {"inline": {"command": "y"}}}"#)
        .file("p2/plugin.json", r#"{"mcpServers": {"inline": {}, "root-level": {}}}"#)
        .installed(r#"{"version": 2, "plugins": {"two@market": [{"scope": "user", "installPath": "@ROOT@/p2"}], "one@market": [{"scope": "user", "installPath": "@ROOT@/p1"}]}}"#);
    out.push(b.find(proj));
    out.push(b.check_env(proj));
    let mut b = base();
    b.file(
        "managed-mcp.json",
        r#"{"mcpServers": {"corp": {"url": "http://SECRETHOST"}}}"#,
    );
    out.push(b.find(proj));
    let mut b = base();
    b.claude_json("{nope")
        .file(&format!("{proj}/.mcp.json"), "[1, 2")
        .file("managed-mcp.json", "")
        .installed(r#"{"plugins": {"x@m": "bad", "y@m": [null, {"installPath": 3}]}}"#);
    out.push(b.find(proj));
    let mut b = base();
    b.claude_json(&format!(
        r#"{{"mcpServers": ["a"], "projects": {{"@ROOT@/{proj}": "bad"}}}}"#
    ));
    out.push(b.find(proj));
    let mut b = base();
    b.claude_json(r#"{"mcpServers": {"my-browseros": {"url": "http://SECRETVALUE123/mcp"}}}"#)
        .file(
            &format!("{proj}/.mcp.json"),
            r#"{"mcpServers": {"p": {"command": "SECRETVALUE123"}}}"#,
        )
        .file(
            "managed-mcp.json",
            r#"{"mcpServers": {"m": {"env": {"T": "SECRETVALUE123"}}}}"#,
        );
    out.push(b.check_env(proj));
    // BrowserosToolsTest
    let mut b = base();
    b.claude_json(r#"{"mcpServers": {"browseros-neo": {}}}"#);
    out.push(b.check_env(proj));
    let mut b = base();
    b.file(
        &format!("{proj}/.mcp.json"),
        r#"{"mcpServers": {"browseros-neo": {}}}"#,
    )
    .settings(r#"{"enabledMcpjsonServers": ["browseros-neo"]}"#);
    out.push(b.check_env(proj));
    let mut b = base();
    b.claude_json(r#"{"mcpServers": {"browseros": {"url": "http://SECRETHOST"}}}"#);
    out.push(b.check_env(proj));
    let mut b = base();
    b.file("bp/.mcp.json", r#"{"mcpServers": {"browseros-neo": {}}}"#)
        .installed(r#"{"plugins": {"bp@m": [{"installPath": "@ROOT@/bp"}]}}"#);
    out.push(b.check_env(proj));
    let mut b = base();
    b.file(
        &format!("{proj}/.mcp.json"),
        r#"{"mcpServers": {"x-browseros": {}}}"#,
    )
    .settings(r#"{"enableAllProjectMcpServers": true}"#);
    out.push(b.check_env(proj));
    // ProjectApprovalTest (a matrix over the approval sources)
    let trust = |b: &mut B, folder: &str, value: &str, extra: &str| {
        b.claude_json(&format!(
            r#"{{"projects": {{"@ROOT@/{folder}": {{"hasTrustDialogAccepted": {value}{extra}}}}}}}"#
        ));
    };
    for (user, repo, repo_local, trusted) in [
        ("", "", "", ""),
        (r#"{"enabledMcpjsonServers": ["browseros"]}"#, "", "", ""),
        (r#"{"enabledMcpjsonServers": ["other"]}"#, "", "", ""),
        (r#"{"enableAllProjectMcpServers": true}"#, "", "", ""),
        ("", "", r#"{"enabledMcpjsonServers": ["browseros"]}"#, ""),
        (
            "",
            "",
            r#"{"enabledMcpjsonServers": ["browseros"]}"#,
            "true",
        ),
        ("", r#"{"enableAllProjectMcpServers": true}"#, "", "true"),
        (
            "",
            r#"{"enableAllProjectMcpServers": true}"#,
            "",
            r#""true""#,
        ),
        ("", r#"{"enableAllProjectMcpServers": true}"#, "", "1"),
        ("", r#"{"enableAllProjectMcpServers": true}"#, "", "[true]"),
        (
            r#"{"enabledMcpjsonServers": ["browseros"]}"#,
            r#"{"disabledMcpjsonServers": ["browseros"]}"#,
            "",
            "",
        ),
        (
            r#"{"enableAllProjectMcpServers": true, "disabledMcpjsonServers": ["browseros"]}"#,
            "",
            "",
            "",
        ),
        (
            r#"{"enableAllProjectMcpServers": true}"#,
            "",
            r#"{"disabledMcpjsonServers": ["browseros"]}"#,
            "true",
        ),
        (r#"{"enabledMcpjsonServers": "browseros"}"#, "", "", ""),
        (
            r#"{"enabledMcpjsonServers": {"browseros": true}}"#,
            "",
            "",
            "",
        ),
        (r#"{"enabledMcpjsonServers": [3, null]}"#, "", "", ""),
        (r#"{"enabledMcpjsonServers": 7}"#, "", "", ""),
    ] {
        let mut b = base();
        b.file(&format!("{proj}/.mcp.json"), &servers(&["browseros"]));
        if !user.is_empty() {
            b.settings(user);
        }
        if !repo.is_empty() {
            b.file(&format!("{proj}/.claude/settings.json"), repo);
        }
        if !repo_local.is_empty() {
            b.file(&format!("{proj}/.claude/settings.local.json"), repo_local);
        }
        if !trusted.is_empty() {
            trust(&mut b, proj, trusted, "");
        }
        out.push(b.find(proj));
        out.push(b.check_env(proj));
    }
    // trust keys: the git root, a subfolder, elsewhere, an ancestor
    for (git, key, cwd) in [
        (true, PROJ, "w/proj/pkg/app"),
        (true, "w/proj/pkg/app", "w/proj/pkg/app"),
        (false, PROJ, "w/proj/pkg/app"),
        (false, "w/proj/pkg/app", "w/proj/pkg/app"),
        (false, "w", PROJ),
        (false, "elsewhere", PROJ),
    ] {
        let mut b = base();
        b.file(&format!("{proj}/.mcp.json"), &servers(&["browseros"]))
            .file(
                &format!("{cwd}/.claude/settings.json"),
                r#"{"enableAllProjectMcpServers": true}"#,
            );
        if git {
            b.dir(&format!("{proj}/.git"));
        }
        trust(&mut b, key, "true", "");
        out.push(b.find(cwd));
    }
    let mut b = base();
    b.file(&format!("{proj}/.mcp.json"), &servers(&["browseros"]))
        .file_enc(
            &format!("{proj}/.claude/settings.json"),
            r#"{"disabledMcpjsonServers": ["browseros"]}"#,
            "utf16le-bom",
        )
        .settings(r#"{"enableAllProjectMcpServers": true}"#);
    trust(&mut b, proj, "true", "");
    out.push(b.find(proj));
    let mut b = base();
    b.file(&format!("{proj}/.mcp.json"), &servers(&["browseros"]))
        .file_enc(
            &format!("{CONFIG}/settings.json"),
            r#"{"enableAllProjectMcpServers": true}"#,
            "utf16le-bom",
        );
    out.push(b.find(proj));
    let mut b = base();
    b.file(&format!("{proj}/.mcp.json"), &servers(&["browseros"]))
        .file_hex(
            &format!("{CONFIG}/settings.json"),
            b"{\"x\": \"a\xff\", \"enableAllProjectMcpServers\": true}",
        )
        .file_enc(
            &format!("{CONFIG}/.claude.json"),
            r#"{"mcpServers": {"browseros-neo": {}}}"#,
            "utf16le-bom",
        );
    out.push(b.find(proj));
    let mut b = base();
    b.file(&format!("{proj}/.mcp.json"), &servers(&["browseros"]));
    trust(
        &mut b,
        proj,
        "true",
        r#", "disabledMcpjsonServers": ["browseros"]"#,
    );
    b.settings(r#"{"enableAllProjectMcpServers": true}"#);
    out.push(b.find(proj));
    let mut b = base();
    b.file(&format!("{proj}/.mcp.json"), &servers(&["browseros"]))
        .file_pad(
            &format!("{proj}/.claude/settings.json"),
            r#"{"enableAllProjectMcpServers": true, "pad": "@PAD@"}"#,
            "pad",
            2 * 1024 * 1024 + 10,
        );
    trust(&mut b, proj, "true", "");
    out.push(b.find(proj));
    let mut b = base();
    b.claude_json(r#"{"mcpServers": {"browseros-neo": {}}}"#)
        .file(
            "managed-mcp.json",
            r#"{"mcpServers": {"corp-browseros": {}}}"#,
        );
    out.push(b.find(proj));
    out.push(b.check_env(proj));
    let mut b = base();
    b.file(&format!("{proj}/.mcp.json"), &servers(&["browseros"]))
        .dir("SysApps/BrowserOS neo.app");
    out.push(b.check_env(proj));
    // HostileNamesTest
    let mut b = base();
    b.claude_json(&servers(&[HOSTILE]));
    out.push(b.check_env(proj));
    let mut b = base();
    b.file(&format!("{proj}/.mcp.json"), &servers(&[HOSTILE]));
    out.push(b.check_env(proj));
    let many: Vec<String> = (0..9).map(|i| format!("browseros-{i}")).collect();
    let many: Vec<&str> = many.iter().map(String::as_str).collect();
    let mut b = base();
    b.claude_json(&servers(&many));
    out.push(b.check_env(proj));
    let mut b = base();
    b.file(&format!("{proj}/.mcp.json"), &servers(&many));
    out.push(b.check_env(proj));
    let long = format!("browseros{}", "x".repeat(5000));
    let mut b = base();
    b.claude_json(&servers(&[&long]));
    out.push(b.check_env(proj));
    let mut b = base();
    b.claude_json(&servers(&[HOSTILE])).file(
        &format!("{proj}/.mcp.json"),
        &servers(&[&format!("{HOSTILE}2")]),
    );
    out.push(b.main(false));
    out.push(b.main(true));
    // PercentInNamesTest
    let mut b = base();
    b.file("bp/.mcp.json", r#"{"mcpServers": {"browseros-neo": {}}}"#)
        .installed(r#"{"plugins": {"b%s%d@m": [{"installPath": "@ROOT@/bp"}]}}"#);
    out.push(b.find(proj));
    out.push(b.check_env(proj));
    let mut b = base();
    b.claude_json(r#"{"mcpServers": {"browseros%s%": {}}}"#);
    out.push(b.find(proj));
    out.push(b.check_env(proj));
    // NestedInstalledPluginsTest
    let deep = format!("{}{}", "[".repeat(200_000), "]".repeat(200_000));
    let mut b = base();
    b.installed(&deep);
    out.push(b.main(false));
    out.push(b.main(true));
    let mut b = base();
    b.installed("{nope");
    out.push(b.main(true));
    out.push(b.main(false));
    let mut b = base();
    b.installed(INSTALLED);
    out.push(b.main(true));
    out.push(b.main(false));
    let mut b = base();
    b.file(
        &format!("{proj}/.mcp.json"),
        r#"{"mcpServers": {"browseros": {}}}"#,
    )
    .settings(&deep);
    out.push(b.check_env(proj));
    let mut b = base();
    let open = "[".repeat(200_000);
    b.installed(&deep)
        .file(&format!("{proj}/.mcp.json"), &open)
        .file(&format!("{proj}/.claude/settings.local.json"), &open)
        .settings(&open);
    out.push(b.check_env(proj));
    // text mode with an odd scope
    let mut b = base();
    b.installed(
        r#"{"plugins": {"superpowers@m": [{"scope": "user; touch x"}, {"scope": "user"}]}}"#,
    );
    out.push(b.main(false));
    // PluginDataTest
    for (state, env) in [
        ("", ""),
        (r#"{"data_dir": "@ROOT@/d"}"#, ""),
        (r#"{"data_dir": "@ROOT@/gone"}"#, ""),
        (r#"{"data_dir": "@ROOT@/f"}"#, ""),
        (r#"{"data_dir": "@ROOT@/gone"}"#, "@ROOT@/d"),
        (r#"{"data_dir": ""}"#, ""),
        (r#"{"data_dir": 0}"#, ""),
        (r#"{"data_dir": "@ROOT@/d"}"#, "@ROOT@/gone"),
    ] {
        let mut b = base();
        b.dir("d").file("f", "x");
        if !state.is_empty() {
            b.state(state);
        }
        if !env.is_empty() {
            b.env("CLAUDE_PLUGIN_DATA", env);
        }
        out.push(b.check_env(proj));
    }
    // PathWrappersTest
    let mut b = base();
    b.wrappers();
    out.push(b.check_env(proj));
    let mut b = base();
    b.wrappers()
        .file(&format!("{PLUG}/bin/atl"), "")
        .chmod(&format!("{PLUG}/bin/atl"), 0o644)
        .chmod(&format!("{PLUG}/bin/atl-hook"), 0o644);
    out.push(b.check_env(proj));
    let mut b = base();
    b.wrappers().env("CLAUDECODE", "1");
    out.push(b.check_env(proj));
    let mut b = base();
    b.wrappers()
        .env("CLAUDECODE", "1")
        .env("PATH", "@ROOT@/bin:@ROOT@/plug/bin");
    out.push(b.check_env(proj));
    let mut b = base();
    b.wrappers()
        .env("CLAUDECODE", "1")
        .exe("other/anthology-mem", "x")
        .env("PATH", "@ROOT@/other:@ROOT@/bin:@ROOT@/plug/bin");
    out.push(b.check_env(proj));
    let mut b = base();
    b.wrappers()
        .env("CLAUDECODE", "1")
        .link("linkbin", "@ROOT@/plug/bin")
        .env("PATH", "@ROOT@/bin:@ROOT@/linkbin");
    out.push(b.check_env(proj));
    // AgentMemoryPathTest
    for names in [
        &[][..],
        &["anthology-implementer"],
        &["implementer"],
        &["reviewer", "anthology-implementer"],
        &["anthology:implementer"],
        &["anthology_debugger"],
        &["notes", "anthology-implementer"],
    ] {
        let mut b = base();
        b.dir(&format!("{proj}/.git"))
            .dir(&format!("{proj}/.claude/agent-memory"));
        for n in names {
            b.dir(&format!("{proj}/.claude/agent-memory/{n}"));
        }
        out.push(b.check_env(proj));
    }
    // SubagentModelTest (doctor's wording)
    let mut b = base();
    b.env("CLAUDE_CODE_SUBAGENT_MODEL", "SECRETMODEL");
    out.push(b.check_env(proj));
    let mut b = base();
    b.settings(r#"{"env": {"CLAUDE_CODE_SUBAGENT_MODEL": "SECRETMODEL"}}"#);
    out.push(b.check_env(proj));
    let mut b = base();
    b.file(
        &format!("{proj}/.claude/settings.local.json"),
        r#"{"env": {"CLAUDE_CODE_SUBAGENT_MODEL": "m"}}"#,
    );
    out.push(b.check_env(proj));
    // AtlSavingsTest: atl's history read, and one that is not a database
    let mut b = base();
    let bytes = history_bytes(
        true,
        "CREATE TABLE hook_decisions (id INTEGER PRIMARY KEY, decision TEXT);
         CREATE TABLE commands (id INTEGER PRIMARY KEY, input_tokens, saved_tokens);",
        &[
            "INSERT INTO hook_decisions (decision) VALUES ('defer'), ('allow'), ('defer');".into(),
            "INSERT INTO commands (input_tokens, saved_tokens) VALUES (1000, 35), (233, 0);".into(),
        ],
    );
    b.file_hex(&format!("{CONFIG}/anthology/atl/history.db"), &bytes);
    out.push(b.check_env(proj));
    out.push(b.main(false));
    let mut b = base();
    b.file(
        &format!("{CONFIG}/anthology/atl/history.db"),
        "not a database",
    );
    out.push(b.check_env(proj));
    // platform exit code: unsupported only on another OS, so main covers the supported one
    out
}

// ---------------------------------------------------------------------------------------------
// The generator

const NAMES: &[&str] = &[
    "browseros-neo",
    "browseros",
    "x-browseros",
    "BrowserOS-Neo",
    "BROWSEROS",
    "my-browseros",
    "browser-os",
    "corp",
    "a",
    "b%s%d",
    "browseros%s%",
    "browseros-neo ",
    "",
    "\u{130}browseros",
    "browseros\u{1b}[0m",
    HOSTILE,
];

fn rand_names(rng: &mut Rng) -> Vec<String> {
    let n = match rng.below(10) {
        0 => 0,
        1 => 6 + rng.index(4),
        _ => 1 + rng.index(3),
    };
    let mut out: Vec<String> = Vec::new();
    for _ in 0..n {
        let name = if rng.chance(0.05) {
            format!("browseros{}", "y".repeat(70 + rng.index(30)))
        } else if rng.chance(0.15) {
            format!("browseros-{}", rng.index(12))
        } else {
            rng.pick(NAMES).to_string()
        };
        if !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

fn names_obj(names: &[String]) -> String {
    let parts: Vec<String> = names
        .iter()
        .map(|n| format!("{}: {{\"url\": \"http://SECRETHOST\"}}", text(&s(n))))
        .collect();
    format!("{{{}}}", parts.join(", "))
}

fn names_list(rng: &mut Rng, names: &[String]) -> String {
    match rng.below(8) {
        0 => r#""browseros""#.into(),
        1 => "7".into(),
        2 => r#"[3, null, "browseros"]"#.into(),
        _ => {
            let picked: Vec<String> = names
                .iter()
                .filter(|_| rng.chance(0.6))
                .map(|n| text(&s(n)))
                .collect();
            format!("[{}]", picked.join(", "))
        }
    }
}

/// A settings-like object with approval keys for `names` (and sometimes the model and rule).
fn rand_approvals(rng: &mut Rng, names: &[String]) -> String {
    let mut parts = Vec::new();
    if rng.chance(0.35) {
        let v = *rng.pick(&["true", "false", r#""true""#, "1", "null"]);
        parts.push(format!(r#""enableAllProjectMcpServers": {v}"#));
    }
    if rng.chance(0.5) {
        parts.push(format!(
            r#""enabledMcpjsonServers": {}"#,
            names_list(rng, names)
        ));
    }
    if rng.chance(0.3) {
        parts.push(format!(
            r#""disabledMcpjsonServers": {}"#,
            names_list(rng, names)
        ));
    }
    if rng.chance(0.15) {
        let v = *rng.pick(&[r#""m""#, r#""""#, r#"" ""#, "3"]);
        parts.push(format!(r#""env": {{"CLAUDE_CODE_SUBAGENT_MODEL": {v}}}"#));
    }
    if rng.chance(0.2) {
        let rule = *rng.pick(&[
            r#"["Read(/@ROOT@/**)"]"#,
            r#"["Read(//old/**)"]"#,
            r#""Read(/@ROOT@/**)""#,
            "[]",
        ]);
        parts.push(format!(r#""permissions": {{"allow": {rule}}}"#));
    }
    format!("{{{}}}", parts.join(", "))
}

/// A settings-like file at `p`: the object, or an odd form of it.
fn rand_settings_file(rng: &mut Rng, b: &mut B, p: &str, body: &str) {
    match rng.below(20) {
        0 => {
            b.file(p, "{nope");
        }
        1 => {
            b.file(p, "[]");
        }
        2 => {
            b.file_enc(p, body, "utf16le-bom");
        }
        3 => {
            b.file_enc(p, body, "utf8-bom");
        }
        4 => {
            b.file(p, "");
        }
        5 => {
            b.file(p, &format!("{}{}", "[".repeat(30_000), "]".repeat(30_000)));
        }
        6 => {
            b.dir(p);
        }
        _ => {
            b.file(p, body);
        }
    }
}

fn rand_projects(rng: &mut Rng, names: &[String]) -> String {
    let mut parts = Vec::new();
    for folder in [PROJ, SUB, "w", "", "other", "w/proj/pkg"] {
        if !rng.chance(0.3) {
            continue;
        }
        let key = if folder.is_empty() {
            "@ROOT@".to_string()
        } else {
            format!("@ROOT@/{folder}")
        };
        let mut entry = Vec::new();
        if rng.chance(0.4) {
            entry.push(format!(r#""mcpServers": {}"#, names_obj(&rand_names(rng))));
        }
        if rng.chance(0.5) {
            let v = *rng.pick(&["true", "true", "false", r#""true""#, "1", "null", "[true]"]);
            entry.push(format!(r#""hasTrustDialogAccepted": {v}"#));
        }
        if rng.chance(0.3) {
            entry.push(format!(
                r#""enabledMcpjsonServers": {}"#,
                names_list(rng, names)
            ));
        }
        if rng.chance(0.2) {
            entry.push(format!(
                r#""disabledMcpjsonServers": {}"#,
                names_list(rng, names)
            ));
        }
        if rng.chance(0.2) {
            entry.push(r#""enableAllProjectMcpServers": true"#.into());
        }
        let value = if rng.chance(0.05) {
            r#""bad""#.to_string()
        } else {
            format!("{{{}}}", entry.join(", "))
        };
        parts.push(format!("{}: {value}", text(&s(&key))));
    }
    format!("{{{}}}", parts.join(", "))
}

fn rand_mcp(rng: &mut Rng, b: &mut B) {
    let names = rand_names(rng);
    // .claude.json
    if rng.chance(0.75) {
        let mut parts = Vec::new();
        if rng.chance(0.6) {
            let v = if rng.chance(0.05) {
                r#"["a"]"#.to_string()
            } else {
                names_obj(&rand_names(rng))
            };
            parts.push(format!(r#""mcpServers": {v}"#));
        }
        if rng.chance(0.5) {
            parts.push(format!(r#""projects": {}"#, rand_projects(rng, &names)));
        }
        let body = format!("{{{}}}", parts.join(", "));
        let p = format!("{}/.claude.json", b.base);
        rand_settings_file(rng, b, &p, &body);
    }
    // the project .mcp.json, at one of three heights, and the git root
    if rng.chance(0.7) {
        let at = *rng.pick(&[PROJ, PROJ, SUB, "w", "w/proj/pkg"]);
        let body = match rng.below(14) {
            0 => r#"{"mcpServers": ["a"]}"#.to_string(),
            1 => "[1, 2".to_string(),
            2 => "[]".to_string(),
            3 => names_obj(&names),
            4 => "null".to_string(),
            _ => format!(r#"{{"mcpServers": {}}}"#, names_obj(&names)),
        };
        // a second one higher up: the walk stops at the first readable one
        if rng.chance(0.25) {
            b.file(
                ".mcp.json",
                &format!(r#"{{"mcpServers": {}}}"#, names_obj(&rand_names(rng))),
            );
        }
        if rng.chance(0.05) {
            // nested deeper than compat's 512 guard-lookup cap, within CPython's limit
            let deep = *rng.pick(&[513, 600, 9_997, 9_998]);
            b.file_pad(
                &format!("{at}/.mcp.json"),
                &format!(r#"{{"mcpServers": {}, "x": @DEEP@}}"#, names_obj(&names)),
                "deep",
                deep - 1,
            );
        } else if rng.chance(0.01) {
            b.file_pad(
                &format!("{at}/.mcp.json"),
                &format!(r#"{{"mcpServers": {}, "pad": "@PAD@"}}"#, names_obj(&names)),
                "pad",
                2 * 1024 * 1024 + 1,
            );
        } else {
            b.file(&format!("{at}/.mcp.json"), &body);
        }
    }
    if rng.chance(0.4) {
        let at = *rng.pick(&[PROJ, PROJ, "w"]);
        b.dir(&format!("{at}/.git"));
    }
    // user settings and the working folders' settings
    if rng.chance(0.6) {
        let body = rand_approvals(rng, &names);
        let p = format!("{}/settings.json", b.config);
        rand_settings_file(rng, b, &p, &body);
    }
    for folder in [PROJ, SUB] {
        for file in ["settings.json", "settings.local.json"] {
            if rng.chance(0.25) {
                let body = rand_approvals(rng, &names);
                let p = format!("{folder}/.claude/{file}");
                if rng.chance(0.01) {
                    b.file_pad(
                        &p,
                        r#"{"enableAllProjectMcpServers": true, "pad": "@PAD@"}"#,
                        "pad",
                        2 * 1024 * 1024 + 10,
                    );
                } else if rng.chance(0.04) {
                    // approval keys only: compat's subagent-model read refuses this band on
                    // purpose (RT1a's 512-level cap), so the model is never in it
                    let deep = *rng.pick(&[600, 9_997, 9_998]);
                    let text = format!(
                        r#"{{"enableAllProjectMcpServers": true, "disabledMcpjsonServers": {}, "x": @DEEP@}}"#,
                        names_list(rng, &names)
                    );
                    b.file_pad(&p, &text, "deep", deep - 1);
                } else {
                    rand_settings_file(rng, b, &p, &body);
                }
            }
        }
    }
    // installed plugins with servers
    if rng.chance(0.4) {
        let mut entries = Vec::new();
        for (i, key) in [
            "one@market",
            "two@m",
            "b%s%d@m",
            "three",
            "anthology@anthology",
        ]
        .iter()
        .enumerate()
        {
            if !rng.chance(0.4) {
                continue;
            }
            let dir = format!("p{i}");
            let install = match rng.below(10) {
                0 => "3".to_string(),
                1 => r#""""#.to_string(),
                2 => r#""@ROOT@/missing""#.to_string(),
                _ => text(&s(&format!("@ROOT@/{dir}"))),
            };
            let entry = if rng.chance(0.05) {
                "null".to_string()
            } else {
                format!(r#"{{"scope": "user", "installPath": {install}}}"#)
            };
            entries.push(format!("{}: [{entry}]", text(&s(key))));
            let ns = rand_names(rng);
            match rng.below(4) {
                0 => {
                    b.file(&format!("{dir}/.mcp.json"), &names_obj(&ns));
                }
                1 => {
                    b.file(
                        &format!("{dir}/.mcp.json"),
                        &format!(r#"{{"mcpServers": {}}}"#, names_obj(&ns)),
                    );
                }
                2 => {
                    b.file(
                        &format!("{dir}/.mcp.json"),
                        r#"{"mcpServers": "flat", "x": {}}"#,
                    );
                }
                _ => {}
            }
            if rng.chance(0.4) {
                b.file(
                    &format!("{dir}/.claude-plugin/plugin.json"),
                    &format!(r#"{{"mcpServers": {}}}"#, names_obj(&rand_names(rng))),
                );
            }
            if rng.chance(0.2) {
                b.file(
                    &format!("{dir}/plugin.json"),
                    &format!(r#"{{"mcpServers": {}}}"#, names_obj(&rand_names(rng))),
                );
            }
        }
        b.installed(&format!(
            r#"{{"version": 2, "plugins": {{{}}}}}"#,
            entries.join(", ")
        ));
    }
    if rng.chance(0.25) {
        let body = match rng.below(6) {
            0 => String::new(),
            1 => r#"{"mcpServers": []}"#.to_string(),
            _ => format!(r#"{{"mcpServers": {}}}"#, names_obj(&rand_names(rng))),
        };
        b.file("managed-mcp.json", &body);
    }
}

fn rand_tools(rng: &mut Rng, b: &mut B) {
    if rng.chance(0.85) {
        b.tools(&["node"]);
    }
    match rng.below(10) {
        0 => {}
        1 => {
            b.file("bin/docker", "#!/bin/sh\n")
                .chmod("bin/docker", 0o644);
        }
        2 => {
            b.dir("bin/docker");
        }
        3 => {
            b.exe("bin/docker", "not a program\n");
        }
        4 | 5 => {
            let code = *rng.pick(&["1", "3", "127"]);
            b.exe("bin/docker", &format!("{TOOL_EXIT}{code}\n"));
        }
        _ => {
            b.tools(&["docker"]);
        }
    }
    for name in ["browseros", "browseros-neo", "BrowserOS", "Browseros"] {
        if rng.chance(0.05) {
            b.tools(&[name]);
        }
    }
    // apps and AppImages
    for (root, want_dir) in [
        ("SysApps", true),
        ("home/Applications", true),
        ("home/Applications", false),
        ("home/.local/bin", false),
    ] {
        if !rng.chance(0.12) {
            continue;
        }
        let name = if want_dir {
            *rng.pick(&[
                "BrowserOS.app",
                "BrowserOS neo.app",
                "browseros.app",
                "Foo.app",
                "BrowserOS",
                "BrowserOS.app.bak",
            ])
        } else {
            *rng.pick(&[
                "BrowserOS.AppImage",
                "BrowserOS-1.2.AppImage",
                "browseros.AppImage",
                "BrowserOS.appimage",
            ])
        };
        let p = format!("{root}/{name}");
        if rng.chance(0.75) == want_dir {
            b.dir(&p);
        } else {
            b.file(&p, "x");
        }
    }
    match rng.below(20) {
        0 => {
            b.env("PATH", "");
        }
        1 => {
            b.env("PATH", "@ROOT@/nobin:@ROOT@/bin");
        }
        2 => {
            b.env("PATH", "@ROOT@/bin:@ROOT@/bin");
        }
        _ => {}
    }
}

fn rand_state(rng: &mut Rng, b: &mut B) {
    // the plugin's atl.lock
    let lock_version = *rng.pick(&[ATL_VERSION, ATL_VERSION, "1.3.0-anthology.1"]);
    match rng.below(10) {
        0 => {}
        1 => {
            b.file(&format!("{PLUG}/atl.lock"), "{}");
        }
        2 => {
            b.file(&format!("{PLUG}/atl.lock"), "not json");
        }
        _ => {
            b.file(&format!("{PLUG}/atl.lock"), &atl_lock_text(lock_version));
        }
    }
    let installed_version = *rng.pick(&[ATL_VERSION, ATL_VERSION, "0.9.0"]);
    let data = *rng.pick(&["data", "data", "elsewhere"]);
    if rng.chance(0.5) {
        atl_install(
            b,
            data,
            installed_version,
            rng.chance(0.85),
            rng.chance(0.8),
        );
    }
    if rng.chance(0.15) {
        b.dir("d");
    }
    if rng.chance(0.1) {
        b.file("afile", "x");
    }
    match rng.below(6) {
        0 => {}
        1 => {
            b.env("CLAUDE_PLUGIN_DATA", "@ROOT@/data");
        }
        2 => {
            b.env("CLAUDE_PLUGIN_DATA", "@ROOT@/gone");
        }
        3 => {
            b.env("CLAUDE_PLUGIN_DATA", "");
        }
        _ => {
            b.unset("CLAUDE_PLUGIN_DATA");
        }
    }
    if !rng.chance(0.8) {
        return;
    }
    let mut parts = Vec::new();
    if rng.chance(0.5) {
        let v = *rng.pick(&[r#""0.1.0""#, "null", r#""unknown""#]);
        parts.push(format!(r#""setup_done": {v}"#));
    }
    if rng.chance(0.7) {
        let v = match rng.below(10) {
            0 => r#""""#.to_string(),
            1 => "0".to_string(),
            2 => r#""@ROOT@/gone/atl""#.to_string(),
            3 => r#""@ROOT@/data/atl/../atl/1.2.1-anthology.1/atl""#.to_string(),
            4 => r#""@ROOT@//data/atl/1.2.1-anthology.1/atl/""#.to_string(),
            5 => r#"["x"]"#.to_string(),
            _ => format!(r#""@ROOT@/{data}/atl/{installed_version}/atl""#),
        };
        parts.push(format!(r#""atl_path": {v}"#));
    }
    if rng.chance(0.6) {
        let v = *rng.pick(&[
            r#""@ROOT@/data""#,
            r#""@ROOT@/data""#,
            r#""@ROOT@/elsewhere""#,
            r#""@ROOT@/d""#,
            r#""@ROOT@/afile""#,
            r#""@ROOT@/gone""#,
            r#""""#,
            "0",
            "5",
            "true",
            r#""@ROOT@/data/""#,
        ]);
        parts.push(format!(r#""data_dir": {v}"#));
    }
    let body = format!("{{{}}}", parts.join(", "));
    if rng.chance(0.05) {
        b.state(if rng.chance(0.5) { "[]" } else { "{nope" });
    } else {
        b.state(&body);
    }
}

/// The bytes of an atl history.db built here (closed, so a WAL file is folded back in).
fn history_bytes(wal: bool, tables: &str, inserts: &[String]) -> Vec<u8> {
    let dir = tempfile::tempdir().expect("tmp");
    let db = dir.path().join("history.db");
    let con = rusqlite::Connection::open(&db).expect("open");
    if wal {
        let _: String = con
            .query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))
            .expect("wal");
    }
    con.execute_batch(tables).expect("schema");
    for sql in inserts {
        con.execute_batch(sql).expect("insert");
    }
    drop(con);
    std::fs::read(&db).expect("read")
}

/// atl's history next to state.json (`atl_savings`): none, not a database, a folder, a table
/// missing, or atl's two tables with token values of every SQLite type and size.
fn rand_atl_history(rng: &mut Rng, b: &mut B) {
    let p = format!("{}/anthology/atl/history.db", b.config);
    const TABLES: &str = "CREATE TABLE hook_decisions (id INTEGER PRIMARY KEY, decision TEXT);
         CREATE TABLE commands (id INTEGER PRIMARY KEY, input_tokens, saved_tokens);";
    match rng.below(10) {
        0..=3 => {}
        4 => {
            b.file(
                &p,
                rng.pick(&["", "not a database", "SQLite format 3\u{0}"]),
            );
        }
        5 => {
            b.dir(&p);
        }
        6 => {
            let bytes = history_bytes(
                rng.chance(0.5),
                "CREATE TABLE commands (input_tokens, saved_tokens);",
                &[],
            );
            b.file_hex(&p, &bytes);
        }
        _ => {
            const VALUES: &[&str] = &[
                "0",
                "1",
                "35",
                "1000",
                "123456789",
                "-5",
                "NULL",
                "2.5",
                "'12'",
                "'abc'",
                "9000000000000000000",
                "X'00'",
            ];
            let mut inserts = Vec::new();
            for _ in 0..rng.below(40) {
                inserts.push("INSERT INTO hook_decisions (decision) VALUES ('defer');".into());
            }
            for _ in 0..rng.below(6) {
                inserts.push(format!(
                    "INSERT INTO commands (input_tokens, saved_tokens) VALUES ({}, {});",
                    rng.pick(VALUES),
                    rng.pick(VALUES)
                ));
            }
            let bytes = history_bytes(rng.chance(0.5), TABLES, &inserts);
            b.file_hex(&p, &bytes);
        }
    }
}

fn rand_plugin(rng: &mut Rng, b: &mut B) {
    for n in ["anthology", "anthology-mem", "atl", "atl-hook"] {
        let p = format!("{PLUG}/bin/{n}");
        match rng.below(14) {
            0 => {}
            1 => {
                b.file(&p, "#!/bin/sh\n").chmod(&p, 0o644);
            }
            2 => {
                b.dir(&p);
            }
            _ => {
                b.exe(&p, "#!/bin/sh\n");
            }
        }
    }
    match rng.below(10) {
        0 => {
            b.env("CLAUDECODE", "1")
                .env("PATH", "@ROOT@/bin:@ROOT@/plug/bin");
        }
        1 => {
            b.env("CLAUDECODE", "1");
        }
        2 => {
            b.env("CLAUDECODE", "");
        }
        3 => {
            b.env("CLAUDECODE", "1")
                .exe("other/anthology-mem", "x")
                .env("PATH", "@ROOT@/other:@ROOT@/plug/bin");
        }
        4 => {
            b.env("CLAUDECODE", "1")
                .link("linkbin", "@ROOT@/plug/bin")
                .env("PATH", "@ROOT@/bin:@ROOT@/linkbin");
        }
        _ => {}
    }
    // agent memory
    if rng.chance(0.2) {
        if rng.chance(0.7) {
            b.dir(&format!("{PROJ}/.git"));
        }
        for n in [
            "anthology-implementer",
            "implementer",
            "anthology:debugger",
            "anthology_scout",
            "notes",
            "reviewer",
        ] {
            if rng.chance(0.3) {
                b.dir(&format!("{PROJ}/.claude/agent-memory/{n}"));
            }
        }
    }
    if rng.chance(0.1) {
        let v = *rng.pick(&["m", "", " ", "x"]);
        b.env("CLAUDE_CODE_SUBAGENT_MODEL", v);
    }
    // managed settings, and the project file that is the user's own (run in the home folder)
    if rng.chance(0.1) {
        let v = *rng.pick(&[r#""m""#, r#""x""#, r#"" ""#, "1"]);
        b.file(
            "managed-settings.json",
            &format!(r#"{{"env": {{"CLAUDE_CODE_SUBAGENT_MODEL": {v}}}}}"#),
        );
    }
    if rng.chance(0.05) {
        b.env("CLAUDE_CONFIG_DIR", &format!("@ROOT@/{PROJ}/.claude"));
    }
}

fn rand_installed(rng: &mut Rng) -> String {
    let mut entries = Vec::new();
    let keys = [
        "superpowers@claude-plugins-official",
        "superpowers@other-market",
        "design-skills@design-skills",
        "superpowers",
        "superpowers-extras@m",
        "anthology-superpowers@anthology",
        "superpowers@m; touch x",
        "superpowers@m\nz",
        "design-skills@a@b",
        "superpowers@",
    ];
    for key in keys {
        if !rng.chance(0.3) {
            continue;
        }
        let mut list = Vec::new();
        for _ in 0..1 + rng.index(3) {
            let scope = *rng.pick(&[
                r#""user""#,
                r#""project""#,
                r#""local""#,
                r#""managed""#,
                r#""USER""#,
                r#""""#,
                r#""user; touch x""#,
                r#""x\u001b[31m\nz""#,
                "3",
                "null",
                r#"["user"]"#,
                "1.5",
                "true",
                r#"{"a": "b"}"#,
            ]);
            list.push(match rng.below(12) {
                0 => "null".to_string(),
                1 => "3".to_string(),
                2 => r#"{"installPath": "/p"}"#.to_string(),
                _ => format!(r#"{{"scope": {scope}, "installPath": "/p"}}"#),
            });
        }
        let value = if rng.chance(0.05) {
            r#""bad""#.to_string()
        } else {
            format!("[{}]", list.join(", "))
        };
        entries.push(format!("{}: {value}", text(&s(key))));
    }
    format!(r#"{{"version": 2, "plugins": {{{}}}}}"#, entries.join(", "))
}

fn gen_case(rng: &mut Rng) -> Value {
    let op = rng.below(20);
    if op < 2 {
        let text = match rng.below(10) {
            0 => r#"{"plugins": []}"#.to_string(),
            1 => "[]".to_string(),
            _ => rand_installed(rng),
        };
        return dup(&text);
    }
    // rand_tools decides node and docker
    let mut b = B::new();
    b.dir(HOME)
        .dir(CONFIG)
        .dir(SUB)
        .dir(PLUG)
        .tools(&["python3"]);
    if rng.chance(0.15) {
        b.home_config();
    } else if rng.chance(0.05) {
        b.env("CLAUDE_CONFIG_DIR", "");
        b.config = format!("{HOME}/.claude");
        b.base = HOME.into();
    }
    rand_tools(rng, &mut b);
    rand_mcp(rng, &mut b);
    rand_state(rng, &mut b);
    rand_atl_history(rng, &mut b);
    rand_plugin(rng, &mut b);
    if rng.chance(0.1) {
        let text = rand_installed(rng);
        b.installed(&text);
    } else if rng.chance(0.03) {
        b.installed(&format!("{}{}", "[".repeat(12_000), "]".repeat(12_000)));
    }
    let cwd = *rng.pick(&[PROJ, PROJ, PROJ, SUB, "w"]);
    match op {
        2..=4 => b.find(cwd),
        5..=9 => b.main(rng.chance(0.5)),
        _ => b.check_env(cwd),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The generator reaches every op and every check outcome (run on the Rust side only).
    #[test]
    fn the_generator_reaches_every_outcome() {
        let mut rng = Rng::new(20_261_121);
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..600 {
            let case = gen_case(&mut rng);
            let op = field(&case, "op")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            seen.insert(op.clone());
            if op != "check_env" {
                continue;
            }
            let out = run(&case);
            let v = json::loads(&String::from_utf8_lossy(&out.stdout)).expect("json");
            for c in v.as_array().expect("list").iter().skip(1) {
                let o = c.as_object().expect("check");
                let name = o
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let ok = match o.get("ok") {
                    Some(Value::Bool(true)) => "ok",
                    Some(Value::Bool(false)) => "fail",
                    _ => "none",
                };
                seen.insert(format!("{name}:{ok}"));
            }
        }
        for want in [
            "check_env",
            "main",
            "find_mcp_servers",
            "duplicates",
            "node:ok",
            "node:fail",
            "docker:ok",
            "docker:fail",
            "browseros:ok",
            "browseros:fail",
            "atl:ok",
            "atl:fail",
            "atl:none",
            "plugin_read_rule:ok",
            "plugin_read_rule:fail",
            "plugin_read_rule:none",
            "browseros_tools:ok",
            "browseros_tools:fail",
            "browseros_tools:none",
            "plugin_data:ok",
            "plugin_data:fail",
            "plugin_data:none",
            "path_wrappers:ok",
            "path_wrappers:fail",
            "agent_memory_path:ok",
            "agent_memory_path:none",
            "subagent_model:ok",
            "subagent_model:fail",
            "atl_savings:ok",
            "atl_savings:none",
        ] {
            assert!(seen.contains(want), "never reached {want}: {seen:?}");
        }
    }
}
