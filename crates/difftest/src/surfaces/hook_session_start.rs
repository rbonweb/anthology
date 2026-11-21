//! The `hook.session-start` difftest surface (RT3): `session_start::run` against
//! `plugins/anthology/scripts/session_start.py` on generated layouts.
//!
//! Oracle: `port/difftest/oracle/session_start.py`; its module docs give the case format and
//! the forced changes it applies to Python's answer (F-4 with m-R14, F-6, F-9). Both sides build
//! the same tree under their own temporary root: RT1a's layout entries, plus `sguard` entries (a
//! guardrails folder whose guard is written as Python's `scripts/guard.py` and Rust's
//! `hooks/guard.sh`, with the same exit code and stderr), and files whose text holds `@RULE@` or
//! `@OPSV@`, built last. Each side reads the same seams from the case: `memory_sync.run` (the
//! memory surface is MS2's), the rtk module (a fake with the same interface; the real one is
//! `rtk_install.reconcile`'s surface), `_elapsed`, `os.getcwd` and `platform.system`.
//!
//! Compared: the hook's stdout JSON and the `state.json` text it leaves, with the root written
//! as `@ROOT@`. The new step (ensuring the live guard's binary) never runs here:
//! `ANTHOLOGY_BOOTSTRAP_DOWNLOADED` is never set, so Rust defers it, which is its rule; the
//! prune finds no `<ah>/bin`.
//!
//! The generator draws: config folder forms (`CLAUDE_CONFIG_DIR` set, unset, with a trailing or
//! doubled slash); the plugin's manifest, guide (missing, not UTF-8, CRLF and CR newlines, a BOM,
//! trailing newlines, oversized) and its four wrappers (present, missing, not executable); the
//! guardrails (a sibling, none, registry entries with scopes and projects, a directory
//! marketplace whose live folder lacks the guard, disabled and enabled flags in all three
//! settings files) and every guard behaviour; settings.json (missing, the rule, a stale rule, a
//! string allow, unparseable, a BOM, deep unterminated nesting, a subagent model); state.json
//! (missing, not JSON, `setup_done`, every `settings_ops_version` form, `data_dir` of every JSON
//! type, `agent_memory_warned`); `CLAUDE_PLUGIN_DATA` and the subagent variable; a repo with
//! agent-memory folders; stdin forms (with and without `cwd`, not JSON, an array); memory text
//! around the 9,000 budget with non-ASCII and astral characters, messages around the 2,000 cap
//! with Python whitespace at the cut, and a raising memory sync; the fake rtk's warning, lock and
//! reconcile errors; elapsed times on both sides of the rtk cutoff; Windows.
//!
//! The generator avoids what the two sides cannot agree on by design: a guard that sleeps
//! (timeouts are unit-tested), stderr that is not UTF-8, deep but valid JSON (ST1's D-1), and
//! RT1a's `guard` entries (their `scripts/guard.py` is not Python).

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anthology_core::Env;
use anthology_core::lock::Lock;
use anthology_pycompat::json::{Int, Map, Value};
use anthology_pycompat::pyerr::PyExc;
use anthology_runtime::bootstrap::GuardBinaryStatus;
use anthology_runtime::guardrails_loc;
use anthology_runtime::rtk_install::Reconciled;
use anthology_runtime::session_start::{self, RtkWiring, SessionDeps};
use anthology_settings as settings;
use anthology_testkit::rt_a;

use crate::case::{Case, Rng, Surface};
use crate::observe::{Observed, from_oracle};
use crate::oracle::Oracle;

/// The surface name (plan §1.4).
pub const NAME: &str = "hook.session-start";

/// Whether `difftest --all` runs this surface on macOS too: yes, every layout exists there.
pub const ON_MACOS: bool = true;

/// The registered implementation.
pub fn surface() -> Option<Box<dyn Surface>> {
    Some(Box::new(SessionStart))
}

struct SessionStart;

impl Surface for SessionStart {
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
        from_oracle(&oracle.call("session_start", &c.data))
    }

    fn run_rust(&self, c: &Case) -> Observed {
        run(&c.data)
    }
}

// --- the guard twin -----------------------------------------------------------------------

/// `(exit code, stderr line)` of guard behaviour `b` (the oracle's `GUARDS`).
fn guard_behaviour(b: &str, reason: &str) -> (i32, Option<String>) {
    let line = |s: &str| Some(s.to_string());
    match b {
        "g1" => (
            2,
            line(
                "Blocked by anthology-guardrails (G1): run it inside Docker (OrbStack); for browser work use BrowserOS Neo",
            ),
        ),
        "g2" => (
            2,
            line(
                "Blocked by anthology-guardrails (G2): never launch Chrome on the host; use BrowserOS Neo",
            ),
        ),
        "plain" => (2, line("Blocked by anthology-guardrails: x")),
        "internal" => (
            2,
            line("Blocked by anthology-guardrails (internal): the guard failed on this input"),
        ),
        "setup" => (
            2,
            Some(format!("Blocked by anthology-guardrails (setup): {reason}")),
        ),
        "exit0" => (0, None),
        "exit1" => (1, None),
        "exit2" => (2, None),
        "g1exit0" => (0, line("Blocked by anthology-guardrails (G1): x")),
        "g1exit3" => (3, line("Blocked by anthology-guardrails (G1): x")),
        "other" => (2, line("something else went wrong")),
        other => panic!("hook.session-start: unknown guard {other:?}"),
    }
}

fn write_file(path: &Path, bytes: &[u8]) {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).expect("mkdir");
    }
    fs::write(path, bytes).expect("write");
}

/// The Rust half of an `sguard` entry (the oracle writes `scripts/guard.py` too).
fn write_guard(base: &Path, b: &str, reason: &str) {
    let (code, line) = guard_behaviour(b, reason);
    let mut sh = String::from("#!/bin/sh\ncat >/dev/null\n");
    if let Some(l) = line {
        sh.push_str(&format!("printf '%s\\n' '{l}' >&2\n"));
    }
    sh.push_str(&format!("exit {code}\n"));
    let path = base.join("hooks/guard.sh");
    write_file(&path, sh.as_bytes());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
}

fn field<'a>(v: &'a Value, k: &str) -> Option<&'a Value> {
    v.as_object().and_then(|o| o.get(k))
}

fn str_of<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    field(v, k).and_then(Value::as_str)
}

fn has_late(entry: &Value) -> bool {
    str_of(entry, "text").is_some_and(|t| t.contains("@RULE@") || t.contains("@OPSV@"))
}

/// Builds the case's tree (module docs).
fn build(root: &Path, case: &Value, env: &Env, cwd: &Path) {
    let tree = field(case, "tree")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let (mut early, mut late, mut guards) = (Vec::new(), Vec::new(), Vec::new());
    for e in tree {
        if str_of(&e, "t") == Some("sguard") {
            guards.push(e);
        } else if has_late(&e) {
            late.push(e);
        } else {
            early.push(e);
        }
    }
    rt_a::materialize(root, &early);
    for g in &guards {
        write_guard(
            &root.join(str_of(g, "p").expect("p")),
            str_of(g, "b").expect("b"),
            str_of(g, "reason").unwrap_or(""),
        );
    }
    let plugin = env.get("CLAUDE_PLUGIN_ROOT").unwrap_or("");
    let rule = if plugin.is_empty() {
        "Read(//none/**)".to_string()
    } else {
        settings::plugin_read_rule(Path::new(plugin))
    };
    // Python's setup_ops raises when it locates no guardrails (from os.getcwd(), the case cwd).
    let located = guardrails_loc::inspect(Path::new(plugin), env, cwd)
        .root
        .is_some();
    let opsv = settings::setup_ops(Path::new(plugin), env)
        .ok()
        .filter(|_| located)
        .map(|ops| settings::ops_version(&ops, &Default::default()))
        .unwrap_or_else(|| "1-ffffffffffffffff".into());
    for e in late {
        let mut m = e.as_object().cloned().expect("entry");
        let text = m
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .replace("@RULE@", &rule)
            .replace("@OPSV@", &opsv);
        m.insert("text".into(), Value::Str(text));
        rt_a::materialize(root, &[Value::Object(m)]);
    }
}

// --- the Rust side ------------------------------------------------------------------------

/// The fake rtk module of the case.
struct CaseRtk {
    warning: Option<String>,
    lock_exc: Option<String>,
    exc: Option<String>,
}

fn opt_str(v: &Value, k: &str) -> Option<String> {
    str_of(v, k).map(str::to_string)
}

impl RtkWiring for CaseRtk {
    fn load_lock(&self, _path: &Path) -> Result<Lock, PyExc> {
        match &self.lock_exc {
            Some(m) => Err(PyExc::Other("RuntimeError", m.clone())),
            None => Ok(Lock {
                version: "9.9.9".into(),
                tag: "rtk--v9.9.9".into(),
                repo: "example/x".into(),
                sha256: Default::default(),
            }),
        }
    }

    fn reconcile(&self, _: &Env, _: &Path, _: &Lock, _: f64) -> Result<Reconciled, PyExc> {
        match &self.exc {
            Some(m) => Err(PyExc::Other("RuntimeError", m.clone())),
            None => Ok(Reconciled {
                rtk_path: None,
                warning: self.warning.clone(),
                installed: false,
            }),
        }
    }
}

fn linux() -> (String, String) {
    ("Linux".into(), "x86_64".into())
}

fn windows() -> (String, String) {
    ("Windows".into(), "AMD64".into())
}

fn never_ensure(_: &Path, _: u64, _: &Env) -> GuardBinaryStatus {
    GuardBinaryStatus::Present
}

fn run(case: &Value) -> Observed {
    // The same root form as the oracle's `mkdtemp(prefix="dt-rt3-", dir="/tmp")`: the memory
    // pointer names a path under the root, so both roots must be as long.
    let tmp = tempfile::Builder::new()
        .prefix("dt-rt3-")
        .rand_bytes(8)
        .tempdir_in("/tmp")
        .expect("tempdir");
    let root = fs::canonicalize(tmp.path()).expect("realpath");
    let x = |s: &str| rt_a::expand(s, &root, None);
    let mut env = Env::default();
    if let Some(vars) = field(case, "env").and_then(Value::as_object) {
        for (k, v) in vars.iter() {
            env = env.with(k, &x(v.as_str().expect("env value")));
        }
    }
    env = env.with("PATH", "/usr/bin:/bin");
    let cwd_v = PathBuf::from(x(str_of(case, "cwd").unwrap_or("@ROOT@")));
    build(&root, case, &env, &cwd_v);

    let memory = field(case, "memory").cloned().unwrap_or(Value::Null);
    let memory_run = |_: &Path, _: &Env| -> Result<(String, Vec<String>), PyExc> {
        if let Some(e) = str_of(&memory, "error") {
            return Err(PyExc::Other("OSError", e.to_string()));
        }
        let text = x(str_of(&memory, "text").unwrap_or(""));
        let messages = field(&memory, "messages")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(x).collect())
            .unwrap_or_default();
        Ok((text, messages))
    };
    let rtk = field(case, "rtk")
        .filter(|v| !matches!(v, Value::Null))
        .map(|v| CaseRtk {
            warning: opt_str(v, "warning"),
            lock_exc: opt_str(v, "lock_exc"),
            exc: opt_str(v, "exc"),
        });
    let elapsed_v = match field(case, "elapsed") {
        Some(Value::Float(f)) => *f,
        Some(Value::Int(Int::Small(n))) => *n as f64,
        _ => 0.0,
    };
    let elapsed = || elapsed_v;
    let cwd = || -> io::Result<PathBuf> { Ok(cwd_v.clone()) };
    let deps = SessionDeps {
        uname: if field(case, "windows").and_then(Value::as_bool) == Some(true) {
            windows
        } else {
            linux
        },
        cwd: &cwd,
        elapsed: Some(&elapsed),
        rtk: rtk.as_ref().map(|r| r as &dyn RtkWiring),
        memory_run: &memory_run,
        ensure_guard_binary: &never_ensure,
        ..SessionDeps::default()
    };
    let out = session_start::run(&env, &x(str_of(case, "stdin").unwrap_or("")), &deps);
    let state_text = fs::read(anthology_core::state::path(&env))
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_else(|_| "<none>".into());
    let stdout = format!("{out}\n{state_text}").replace(&*root.to_string_lossy(), "@ROOT@");
    rt_a::unlock(&root);
    Observed::text(0, &stdout, "")
}

// --- cases --------------------------------------------------------------------------------

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    let mut m = Map::new();
    for (k, v) in pairs {
        m.insert(k.to_string(), v);
    }
    Value::Object(m)
}

fn s(v: &str) -> Value {
    Value::Str(v.into())
}

fn file(p: &str, text: &str) -> Value {
    obj(vec![("t", s("file")), ("p", s(p)), ("text", s(text))])
}

fn file_hex(p: &str, bytes: &[u8]) -> Value {
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    obj(vec![("t", s("file")), ("p", s(p)), ("hex", s(&hex))])
}

fn dir(p: &str) -> Value {
    obj(vec![("t", s("dir")), ("p", s(p))])
}

fn chmod(p: &str, mode: i64) -> Value {
    obj(vec![
        ("t", s("chmod")),
        ("p", s(p)),
        ("mode", Value::Int(Int::Small(mode))),
    ])
}

fn sguard(p: &str, b: &str, reason: &str) -> Value {
    obj(vec![
        ("t", s("sguard")),
        ("p", s(p)),
        ("b", s(b)),
        ("reason", s(reason)),
    ])
}

const PLUGIN: &str = "p/anthology";
const GUARD: &str = "p/anthology-guardrails";
const WRAPPERS: [&str; 4] = ["anthology", "anthology-mem", "rtk", "rtk-hook"];
const MANIFEST: &str = r#"{"name": "anthology", "version": "0.1.0"}"#;
const GUIDE: &str = "# Anthology routing guide\n\nA short guide for the difftest.\n";

/// A case builder.
struct B {
    tree: Vec<Value>,
    env: Map,
    stdin: String,
    cwd: String,
    memory: Value,
    rtk: Value,
    elapsed: f64,
    windows: bool,
}

impl B {
    /// A healthy layout: the plugin with its manifest, guide, rtk.lock and wrappers, a blocking
    /// sibling guard, the config in `@ROOT@/cfg`, the data folder, a repo at `@ROOT@/w`.
    fn new() -> B {
        let mut tree = vec![
            file(&format!("{PLUGIN}/.claude-plugin/plugin.json"), MANIFEST),
            file(&format!("{PLUGIN}/templates/routing-guide.md"), GUIDE),
            file(&format!("{PLUGIN}/rtk.lock"), "{}"),
            dir("home"),
            dir("data"),
            dir("w/.git"),
            sguard(GUARD, "g1", ""),
        ];
        for w in WRAPPERS {
            let p = format!("{PLUGIN}/bin/{w}");
            tree.push(file(&p, "#!/bin/sh\nexit 0\n"));
            tree.push(chmod(&p, 0o755));
        }
        let mut env = Map::new();
        env.insert("HOME".into(), s("@ROOT@/home"));
        env.insert("CLAUDE_CONFIG_DIR".into(), s("@ROOT@/cfg"));
        env.insert("CLAUDE_PLUGIN_ROOT".into(), s(&format!("@ROOT@/{PLUGIN}")));
        env.insert("CLAUDE_PLUGIN_DATA".into(), s("@ROOT@/data"));
        B {
            tree,
            env,
            stdin: r#"{"hook_event_name": "SessionStart", "source": "startup", "cwd": "@ROOT@/w"}"#
                .into(),
            cwd: "@ROOT@/w".into(),
            memory: obj(vec![
                ("text", s("")),
                ("messages", Value::Array(Vec::new())),
            ]),
            rtk: obj(vec![
                ("warning", Value::Null),
                ("lock_exc", Value::Null),
                ("exc", Value::Null),
            ]),
            elapsed: 0.0,
            windows: false,
        }
    }

    fn config(&self) -> String {
        match self.env.get("CLAUDE_CONFIG_DIR").and_then(Value::as_str) {
            Some(c) if !c.is_empty() => {
                c.trim_start_matches("@ROOT@").trim_matches('/').to_string()
            }
            _ => "home/.claude".into(),
        }
    }

    /// state.json, with setup done and the current op set.
    fn setup_done(mut self) -> B {
        let p = format!("{}/anthology/state.json", self.config());
        self.tree.push(file(
            &p,
            r#"{"settings_ops_version": "@OPSV@", "setup_done": "0.1.0"}"#,
        ));
        let p = format!("{}/settings.json", self.config());
        self.tree
            .push(file(&p, r#"{"permissions": {"allow": ["@RULE@"]}}"#));
        self
    }

    fn state(mut self, text: &str) -> B {
        let p = format!("{}/anthology/state.json", self.config());
        self.tree.retain(|e| str_of(e, "p") != Some(p.as_str()));
        self.tree.push(file(&p, text));
        self
    }

    fn settings(mut self, text: &str) -> B {
        let p = format!("{}/settings.json", self.config());
        self.tree.retain(|e| str_of(e, "p") != Some(p.as_str()));
        self.tree.push(file(&p, text));
        self
    }

    fn guard(mut self, b: &str, reason: &str) -> B {
        self.tree.retain(|e| str_of(e, "t") != Some("sguard"));
        self.tree.push(sguard(GUARD, b, reason));
        self
    }

    fn no_guard(mut self) -> B {
        self.tree.retain(|e| str_of(e, "t") != Some("sguard"));
        self
    }

    fn entry(mut self, e: Value) -> B {
        self.tree.push(e);
        self
    }

    fn env(mut self, k: &str, v: Option<&str>) -> B {
        match v {
            Some(v) => self.env.insert(k.into(), s(v)),
            None => {
                self.env.remove(k);
            }
        }
        self
    }

    fn memory(mut self, text: &str, messages: &[&str]) -> B {
        self.memory = obj(vec![
            ("text", s(text)),
            (
                "messages",
                Value::Array(messages.iter().map(|m| s(m)).collect()),
            ),
        ]);
        self
    }

    fn rtk(mut self, warning: Option<&str>, lock_exc: Option<&str>, exc: Option<&str>) -> B {
        let v = |o: Option<&str>| o.map_or(Value::Null, s);
        self.rtk = obj(vec![
            ("warning", v(warning)),
            ("lock_exc", v(lock_exc)),
            ("exc", v(exc)),
        ]);
        self
    }

    fn build(self) -> Value {
        obj(vec![
            ("tree", Value::Array(self.tree)),
            ("env", Value::Object(self.env)),
            ("stdin", s(&self.stdin)),
            ("cwd", s(&self.cwd)),
            ("memory", self.memory),
            ("rtk", self.rtk),
            ("elapsed", Value::Float(self.elapsed)),
            ("windows", Value::Bool(self.windows)),
        ])
    }
}

fn lines(n: usize, width: usize, tag: &str) -> String {
    (0..n)
        .map(|i| format!("- {tag} {i:04} {}\n", "w".repeat(width)))
        .collect()
}

const SETUP_REASON: &str = "the guard binary is not installed (not attempted), so shell commands, file edits and MCP tools are blocked; run /anthology:setup (or start a new session while online) to install it; if this keeps happening, disable anthology-guardrails with /plugin";

fn seeds() -> Vec<Value> {
    let mut out = Vec::new();
    // OutputTest
    out.push(B::new().setup_done().build());
    out.push(B::new().build()); // the NAG
    out.push(
        B::new()
            .state(r#"{"setup_done": "0.1.0"}"#)
            .settings(r#"{"permissions": {"allow": ["Read(//old/place/plugins/**)"]}}"#)
            .build(),
    );
    out.push(B::new().state(r#"{"setup_done": "0.1.0"}"#).build());
    out.push(
        B::new()
            .state(r#"{"setup_done": "0.1.0"}"#)
            .settings("{nope")
            .build(),
    );
    out.push(
        B::new()
            .setup_done()
            .settings("\u{feff}{\"permissions\": {\"allow\": [\"@RULE@\"]}}")
            .build(),
    );
    out.push(
        B::new()
            .state(r#"{"setup_done": "0.1.0"}"#)
            .settings(r#"{"permissions": {"allow": "x@RULE@y"}}"#)
            .build(),
    );
    out.push(
        B::new()
            .setup_done()
            .memory(
                &format!("## Personal memory (x)\n\n{}", lines(400, 30, "note")),
                &[],
            )
            .build(),
    );
    let mut big = B::new().setup_done();
    big.tree.push(file(
        &format!("{PLUGIN}/templates/routing-guide.md"),
        &format!("# Anthology routing guide\n{}", "x".repeat(9500)),
    ));
    out.push(big.memory("m\n", &[]).build());
    out.push(
        B::new()
            .setup_done()
            .memory("", &["memory said hi"])
            .build(),
    );
    let msgs: Vec<String> = (0..100)
        .map(|i| format!("m{i:03} {}", "y".repeat(80)))
        .collect();
    let msgs: Vec<&str> = msgs.iter().map(String::as_str).collect();
    out.push(B::new().setup_done().memory("", &msgs).build());
    let mut missing = B::new().setup_done();
    missing
        .tree
        .retain(|e| str_of(e, "p") != Some("p/anthology/templates/routing-guide.md"));
    out.push(missing.build());
    let mut garbage = B::new().setup_done();
    garbage.stdin = "not json".into();
    out.push(garbage.build());
    // OpsVersionTest
    out.push(
        B::new()
            .setup_done()
            .state(r#"{"settings_ops_version": "1-0000000000000000", "setup_done": "0.1.0"}"#)
            .build(),
    );
    out.push(
        B::new()
            .setup_done()
            .state(r#"{"settings_ops_version": null, "setup_done": "0.1.0"}"#)
            .build(),
    );
    // CompatibilityLinesTest
    out.push(
        B::new()
            .setup_done()
            .env("CLAUDE_PLUGIN_DATA", None)
            .build(),
    );
    let mut wr = B::new().setup_done();
    wr.tree
        .retain(|e| str_of(e, "p") != Some("p/anthology/bin/rtk"));
    wr.tree.push(chmod("p/anthology/bin/anthology-mem", 0o644));
    out.push(wr.build());
    out.push(
        B::new()
            .setup_done()
            .entry(dir("w/.claude/agent-memory/anthology-implementer"))
            .entry(dir("w/.claude/agent-memory/reviewer"))
            .build(),
    );
    out.push(
        B::new()
            .setup_done()
            .entry(dir("w/.claude/agent-memory/reviewer"))
            .build(),
    );
    out.push(
        B::new()
            .setup_done()
            .entry(dir("w/.claude/agent-memory/reviewer"))
            .state(r#"{"agent_memory_warned": {"@ROOT@/w": true}, "settings_ops_version": "@OPSV@", "setup_done": "0.1.0"}"#)
            .build(),
    );
    out.push(
        B::new()
            .setup_done()
            .env("CLAUDE_CODE_SUBAGENT_MODEL", Some("SECRETMODEL"))
            .build(),
    );
    out.push(
        B::new()
            .setup_done()
            .settings(r#"{"env": {"CLAUDE_CODE_SUBAGENT_MODEL": "m"}}"#)
            .build(),
    );
    // FailureTest
    out.push(B::new().setup_done().settings(&"[".repeat(200_000)).build());
    let mut nested = B::new().setup_done();
    nested.stdin = "[".repeat(200_000);
    out.push(nested.build());
    let mut bad_guide = B::new().setup_done();
    bad_guide.tree.push(file_hex(
        "p/anthology/templates/routing-guide.md",
        b"\xff\xfe\x00bad",
    ));
    out.push(bad_guide.build());
    out.push(B::new().setup_done().build_memory_error("disk"));
    // PlatformTest
    let mut win = B::new().setup_done();
    win.windows = true;
    out.push(win.build());
    // StateTest
    out.push(
        B::new()
            .setup_done()
            .state(r#"{"data_dir": "@ROOT@/data", "settings_ops_version": "@OPSV@", "setup_done": "0.1.0"}"#)
            .build(),
    );
    out.push(
        B::new()
            .setup_done()
            .state(r#"{"data_dir": "@ROOT@/remembered", "settings_ops_version": "@OPSV@", "setup_done": "0.1.0"}"#)
            .env("CLAUDE_PLUGIN_DATA", None)
            .build(),
    );
    // RtkTest
    out.push(
        B::new()
            .setup_done()
            .rtk(Some("rtk download failed: offline"), None, None)
            .build(),
    );
    out.push(
        B::new()
            .setup_done()
            .rtk(None, Some("rtk.lock is not valid JSON"), None)
            .build(),
    );
    out.push(
        B::new()
            .setup_done()
            .rtk(None, None, Some("kaboom"))
            .build(),
    );
    out.push(B::new().setup_done().rtk(None, Some(""), None).build());
    out.push(B::new().setup_done().rtk(None, None, Some("")).build());
    let mut absent = B::new().setup_done();
    absent.rtk = Value::Null;
    out.push(absent.build());
    let mut no_lock = B::new().setup_done().rtk(None, Some("cannot read"), None);
    no_lock
        .tree
        .retain(|e| str_of(e, "p") != Some("p/anthology/rtk.lock"));
    out.push(no_lock.build());
    for dd in ["5", "true", "[]", "\"\"", "[1]", "{\"a\": 1}", "1.5"] {
        out.push(
            B::new()
                .setup_done()
                .env("CLAUDE_PLUGIN_DATA", None)
                .state(&format!(
                    r#"{{"data_dir": {dd}, "settings_ops_version": "@OPSV@", "setup_done": "0.1.0"}}"#
                ))
                .build(),
        );
    }
    // GuardSelfTestTest, GuardLocationTest and F-4
    for b in [
        "g1", "g2", "plain", "internal", "exit0", "exit1", "exit2", "g1exit0", "g1exit3", "other",
    ] {
        out.push(B::new().setup_done().guard(b, "").build());
    }
    out.push(B::new().setup_done().guard("setup", SETUP_REASON).build());
    out.push(
        B::new()
            .setup_done()
            .guard("setup", &"é".repeat(400))
            .build(),
    );
    out.push(B::new().setup_done().no_guard().build());
    out.push(
        B::new()
            .setup_done()
            .settings(r#"{"permissions": {"allow": ["@RULE@"]}, "enabledPlugins": {"anthology-guardrails@anthology": false}}"#)
            .build(),
    );
    out.push(
        B::new()
            .setup_done()
            .entry(file(
                "w/.claude/settings.local.json",
                r#"{"enabledPlugins": {"anthology-guardrails@x": false}}"#,
            ))
            .build(),
    );
    out.push(
        B::new()
            .setup_done()
            .entry(file(
                "cfg/plugins/installed_plugins.json",
                r#"{"version": 2, "plugins": {"anthology@anthology": [{"scope": "user", "installPath": "@ROOT@/p/anthology"}]}}"#,
            ))
            .build(),
    );
    out.push(
        B::new()
            .setup_done()
            .entry(sguard("g/new", "g1", ""))
            .entry(sguard("g/old", "exit0", ""))
            .entry(file(
                "cfg/plugins/installed_plugins.json",
                r#"{"version": 2, "plugins": {"anthology-guardrails@anthology": [
                {"scope": "user", "installPath": "@ROOT@/g/old", "lastUpdated": "2026-01-01T00:00:00Z"},
                {"scope": "user", "installPath": "@ROOT@/g/new", "lastUpdated": "2026-06-01T00:00:00Z"},
                {"scope": "user", "installPath": "@ROOT@/gone", "lastUpdated": "2027-01-01T00:00:00Z"}]}}"#,
            ))
            .build(),
    );
    // a directory marketplace whose live folder has no guard (F-6)
    out.push(
        B::new()
            .setup_done()
            .entry(sguard("cfg/plugins/cache/anthology/anthology-guardrails/0.1.0", "g1", ""))
            .entry(dir("market/plugins/anthology-guardrails/scripts"))
            .entry(file(
                "market/.claude-plugin/marketplace.json",
                r#"{"name": "anthology", "plugins": [{"name": "anthology-guardrails", "source": "./plugins/anthology-guardrails"}]}"#,
            ))
            .entry(file(
                "cfg/plugins/known_marketplaces.json",
                r#"{"anthology": {"source": {"source": "directory", "path": "@ROOT@/market"}, "installLocation": "@ROOT@/market", "lastUpdated": "x"}}"#,
            ))
            .entry(file(
                "cfg/plugins/installed_plugins.json",
                r#"{"version": 2, "plugins": {"anthology-guardrails@anthology": [{"scope": "user", "installPath": "@ROOT@/cfg/plugins/cache/anthology/anthology-guardrails/0.1.0", "version": "0.1.0", "lastUpdated": "2026-01-01T00:00:00Z"}]}}"#,
            ))
            .build(),
    );
    // DeadlineTest
    for e in [10.0, 10.5, 30.0] {
        let mut late = B::new().setup_done();
        late.elapsed = e;
        out.push(late.build());
    }
    // config forms
    for c in ["@ROOT@/cfg/", "@ROOT@//cfg", ""] {
        out.push(
            B::new()
                .env("CLAUDE_CONFIG_DIR", Some(c))
                .setup_done()
                .memory(&lines(400, 30, "x"), &[])
                .build(),
        );
    }
    // a config folder that is a file: state.json cannot be written
    out.push(B::new().entry(file("cfg", "a file")).build());
    out
}

impl B {
    fn build_memory_error(mut self, e: &str) -> Value {
        self.memory = obj(vec![("error", s(e))]);
        self.build()
    }
}

// --- the generator ------------------------------------------------------------------------

const ODD_CHARS: &[&str] = &[
    "é", "ß", "\u{3000}", "\u{1c}", "\u{85}", "\u{2028}", "𝄞", "\t", " ", "\u{a0}", "ı", "İ",
];

fn odd_text(rng: &mut Rng, n: usize) -> String {
    (0..n)
        .map(|_| {
            if rng.chance(0.15) {
                (*rng.pick(ODD_CHARS)).to_string()
            } else {
                rng.ascii_char().to_string()
            }
        })
        .filter(|c| c != "'" && c != "\\" && c != "\r")
        .collect()
}

fn gen_guide(rng: &mut Rng) -> Option<Value> {
    let p = format!("{PLUGIN}/templates/routing-guide.md");
    Some(match rng.below(9) {
        0 => return None,
        1 => file_hex(&p, b"\xff\xfe\x00bad"),
        2 => file(&p, "# Anthology routing guide\r\nCRLF line\r\n\r\n"),
        3 => file(&p, "# Anthology routing guide\rCR only\r"),
        4 => file(&p, "\u{feff}# Anthology routing guide\n"),
        5 => file(
            &p,
            &format!("# Anthology routing guide\n{}\n\n\n", odd_text(rng, 200)),
        ),
        6 => file(
            &p,
            &format!(
                "# Anthology routing guide\n{}",
                "g\n".repeat(rng.index(5000))
            ),
        ),
        _ => file(&p, GUIDE),
    })
}

fn gen_memory(rng: &mut Rng) -> Value {
    if rng.chance(0.04) {
        let n = rng.index(200);
        return obj(vec![("error", s(&odd_text(rng, n)))]);
    }
    let mut text = String::new();
    match rng.below(4) {
        0 => {}
        1 => {
            let n = rng.index(300);
            text = odd_text(rng, n);
        }
        _ => {
            let n = rng.index(400);
            let w = rng.index(40);
            for i in 0..n {
                text.push_str(&format!("- l{i} {}\n", odd_text(rng, w)));
            }
            if rng.chance(0.3) {
                text.push_str(&"\n".repeat(rng.index(4)));
            }
        }
    }
    let n = if rng.chance(0.5) { 0 } else { rng.index(40) };
    let messages: Vec<Value> = (0..n)
        .map(|_| {
            let w = rng.index(120);
            s(&odd_text(rng, w))
        })
        .collect();
    obj(vec![
        ("text", s(&text)),
        ("messages", Value::Array(messages)),
    ])
}

const GUARDS: &[&str] = &[
    "g1", "g1", "g1", "g1", "g2", "plain", "internal", "setup", "exit0", "exit1", "exit2",
    "g1exit0", "g1exit3", "other",
];

fn gen_case(rng: &mut Rng) -> Value {
    let mut b = B::new();
    // config folder forms
    match rng.below(10) {
        0 => b = b.env("CLAUDE_CONFIG_DIR", Some("")),
        1 => b = b.env("CLAUDE_CONFIG_DIR", Some("@ROOT@/cfg/")),
        2 => b = b.env("CLAUDE_CONFIG_DIR", Some("@ROOT@//cfg")),
        _ => {}
    }
    let config = b.config();
    // setup state
    if rng.chance(0.65) {
        b = b.setup_done();
    }
    if rng.chance(0.4) {
        let mut st = Vec::new();
        if rng.chance(0.7) {
            st.push(r#""setup_done": "0.1.0""#.to_string());
        }
        match rng.below(5) {
            0 => st.push(r#""settings_ops_version": "@OPSV@""#.into()),
            1 => st.push(r#""settings_ops_version": "1-0000000000000000""#.into()),
            2 => st.push(r#""settings_ops_version": null"#.into()),
            3 => st.push(r#""settings_ops_version": 5"#.into()),
            _ => {}
        }
        match rng.below(8) {
            0 => st.push(r#""data_dir": "@ROOT@/data""#.into()),
            1 => st.push(r#""data_dir": "@ROOT@/elsewhere""#.into()),
            2 => st.push(r#""data_dir": 5"#.into()),
            3 => st.push(r#""data_dir": """#.into()),
            4 => st.push(r#""data_dir": ["x"]"#.into()),
            _ => {}
        }
        match rng.below(6) {
            0 => st.push(r#""agent_memory_warned": {"@ROOT@/w": true}"#.into()),
            1 => st.push(r#""agent_memory_warned": {"@ROOT@/w": 0}"#.into()),
            2 => st.push(r#""agent_memory_warned": []"#.into()),
            _ => {}
        }
        let text = if rng.chance(0.05) {
            "not json".to_string()
        } else {
            format!("{{{}}}", st.join(", "))
        };
        b = b.state(&text);
    }
    // settings.json
    let rule_allow = r#""permissions": {"allow": ["@RULE@"]}"#;
    let mut parts: Vec<String> = Vec::new();
    match rng.below(10) {
        0 => {
            let p = format!("{config}/settings.json");
            b.tree.retain(|e| str_of(e, "p") != Some(p.as_str()));
        }
        1 => b = b.settings(r#"{"permissions": {"allow": ["Read(//old/place/plugins/**)"]}}"#),
        2 => b = b.settings(r#"{"permissions": {"allow": "x@RULE@y"}}"#),
        3 => b = b.settings("{nope"),
        4 => b = b.settings(&"[".repeat(rng.index(30_000))),
        5 => b = b.settings("\u{feff}{\"permissions\": {\"allow\": [\"@RULE@\"]}}"),
        _ => {
            parts.push(rule_allow.into());
            match rng.below(6) {
                0 => parts
                    .push(r#""enabledPlugins": {"anthology-guardrails@anthology": false}"#.into()),
                1 => parts
                    .push(r#""enabledPlugins": {"anthology-guardrails@anthology": true}"#.into()),
                2 => parts.push(r#""enabledPlugins": {"anthology-guardrails@x": 0}"#.into()),
                _ => {}
            }
            match rng.below(6) {
                0 => parts.push(r#""env": {"CLAUDE_CODE_SUBAGENT_MODEL": "m"}"#.into()),
                1 => parts.push(r#""env": {"CLAUDE_CODE_SUBAGENT_MODEL": "  "}"#.into()),
                _ => {}
            }
            if rng.chance(0.1) {
                parts.push(
                    r#""permissions": {"allow": ["Bash(rtk *)", "Bash(anthology-mem save *)", "@RULE@"], "deny": []}, "worktree": {"baseRef": "head"}"#
                        .into(),
                );
            }
            b = b.settings(&format!("{{{}}}", parts.join(", ")));
        }
    }
    // the plugin
    if rng.chance(0.05) {
        let p = format!("{PLUGIN}/.claude-plugin/plugin.json");
        b.tree.retain(|e| str_of(e, "p") != Some(p.as_str()));
        if rng.chance(0.5) {
            b.tree.push(file(&p, r#"{"name": "other"}"#));
        }
    }
    let gp = format!("{PLUGIN}/templates/routing-guide.md");
    b.tree.retain(|e| str_of(e, "p") != Some(gp.as_str()));
    if let Some(g) = gen_guide(rng) {
        b.tree.push(g);
    }
    for w in WRAPPERS {
        let p = format!("{PLUGIN}/bin/{w}");
        match rng.below(16) {
            0 => {
                b.tree.retain(|e| str_of(e, "p") != Some(p.as_str()));
            }
            1 => b.tree.push(chmod(&p, 0o644)),
            _ => {}
        }
    }
    if rng.chance(0.15) {
        let p = format!("{PLUGIN}/rtk.lock");
        b.tree.retain(|e| str_of(e, "p") != Some(p.as_str()));
    }
    // the guardrails
    match rng.below(12) {
        0 => b = b.no_guard(),
        1 => {
            b = b
                .entry(file(
                    &format!("{config}/plugins/installed_plugins.json"),
                    r#"{"version": 2, "plugins": {"anthology@anthology": [{"scope": "user", "installPath": "@ROOT@/p/anthology"}]}}"#,
                ));
        }
        2 => {
            let scope = *rng.pick(&["user", "project", "local", "managed"]);
            let project = *rng.pick(&["@ROOT@/w", "@ROOT@/elsewhere", "@ROOT@"]);
            let gb = *rng.pick(GUARDS);
            b = b.entry(sguard("g/one", gb, "")).entry(file(
                &format!("{config}/plugins/installed_plugins.json"),
                &format!(
                    r#"{{"version": 2, "plugins": {{"anthology-guardrails@anthology": [{{"scope": "{scope}", "projectPath": "{project}", "installPath": "@ROOT@/g/one"}}]}}}}"#
                ),
            ));
        }
        3 => {
            let gb = *rng.pick(GUARDS);
            b = b
                .entry(sguard(&format!("{config}/plugins/cache/anthology/anthology-guardrails/0.1.0"), "g1", ""))
                .entry(dir("market/plugins/anthology-guardrails/scripts"))
                .entry(file(
                    "market/.claude-plugin/marketplace.json",
                    r#"{"name": "anthology", "plugins": [{"name": "anthology-guardrails", "source": "./plugins/anthology-guardrails"}]}"#,
                ))
                .entry(file(
                    &format!("{config}/plugins/known_marketplaces.json"),
                    r#"{"anthology": {"source": {"source": "directory", "path": "@ROOT@/market"}, "installLocation": "@ROOT@/market", "lastUpdated": "x"}}"#,
                ))
                .entry(file(
                    &format!("{config}/plugins/installed_plugins.json"),
                    &format!(
                        r#"{{"version": 2, "plugins": {{"anthology-guardrails@anthology": [{{"scope": "user", "installPath": "@ROOT@/{config}/plugins/cache/anthology/anthology-guardrails/0.1.0", "version": "0.1.0", "lastUpdated": "2026-01-01T00:00:00Z"}}]}}}}"#
                    ),
                ));
            if rng.chance(0.5) {
                b = b.entry(sguard("market/plugins/anthology-guardrails", gb, ""));
            }
        }
        _ => {
            let gb = *rng.pick(GUARDS);
            let reason = if gb == "setup" {
                if rng.chance(0.5) {
                    SETUP_REASON.to_string()
                } else {
                    let n = rng.index(420);
                    odd_text(rng, n)
                }
            } else {
                String::new()
            };
            b = b.guard(gb, &reason);
        }
    }
    // the project
    if rng.chance(0.25) {
        b = b.entry(file(
            "w/.claude/settings.local.json",
            r#"{"enabledPlugins": {"anthology-guardrails@x": false}}"#,
        ));
    }
    if rng.chance(0.15) {
        b = b.entry(file(
            "w/.claude/settings.json",
            r#"{"env": {"CLAUDE_CODE_SUBAGENT_MODEL": "m"}}"#,
        ));
    }
    for name in [
        "reviewer",
        "anthology-reviewer",
        "anthology:scout",
        "anthology_scout",
        "mine",
    ] {
        if rng.chance(0.15) {
            b = b.entry(dir(&format!("w/.claude/agent-memory/{name}")));
        }
    }
    if rng.chance(0.1) {
        b.tree.retain(|e| str_of(e, "p") != Some("w/.git"));
    }
    // environment
    match rng.below(10) {
        0 => b = b.env("CLAUDE_PLUGIN_DATA", None),
        1 => b = b.env("CLAUDE_PLUGIN_DATA", Some("")),
        _ => {}
    }
    match rng.below(12) {
        0 => b = b.env("CLAUDE_CODE_SUBAGENT_MODEL", Some("x")),
        1 => b = b.env("CLAUDE_CODE_SUBAGENT_MODEL", Some("  ")),
        2 => b = b.env("CLAUDE_CODE_SUBAGENT_MODEL", Some("")),
        _ => {}
    }
    // stdin
    match rng.below(12) {
        0 => b.stdin = "not json".into(),
        1 => b.stdin = "[1, 2]".into(),
        2 => b.stdin = String::new(),
        3 => b.stdin = r#"{"cwd": 5}"#.into(),
        4 => b.stdin = r#"{"cwd": ""}"#.into(),
        5 => {
            // Python's setup_ops looks for the guardrails from os.getcwd(), Rust from the hook's
            // cwd: the same folder in a real hook.
            b.stdin = r#"{"cwd": "@ROOT@"}"#.into();
            b.cwd = "@ROOT@".into();
        }
        _ => {}
    }
    b.memory = gen_memory(rng);
    b.rtk = match rng.below(8) {
        0 => Value::Null,
        1 => obj(vec![
            ("warning", s(&odd_text(rng, 60))),
            ("lock_exc", Value::Null),
            ("exc", Value::Null),
        ]),
        2 => obj(vec![
            ("warning", Value::Null),
            ("lock_exc", s(&odd_text(rng, 200))),
            ("exc", Value::Null),
        ]),
        3 => obj(vec![
            ("warning", Value::Null),
            ("lock_exc", Value::Null),
            ("exc", s(&odd_text(rng, 200))),
        ]),
        4 => obj(vec![
            ("warning", s("")),
            ("lock_exc", Value::Null),
            ("exc", Value::Null),
        ]),
        _ => b.rtk,
    };
    b.elapsed = match rng.below(10) {
        0 => 10.0,
        1 => 10.000001,
        2 => 45.0,
        _ => 0.0,
    };
    b.windows = rng.chance(0.02);
    b.build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use anthology_pycompat::json;

    /// Every outcome of every step is reached by the generator (Rust side only).
    #[test]
    fn generated_cases_reach_every_outcome() {
        let wanted = [
            ("nag", "run /anthology:setup to finish installing"),
            ("moved", "the plugin moved"),
            ("unreadable", "could not read settings.json"),
            ("ops", "anthology settings changed in this release"),
            ("rtk skipped", "skipped the rtk check"),
            ("rtk absent", "rtk wiring not present yet"),
            ("rtk lock", "rtk.lock is not valid"),
            ("rtk failed", "rtk setup failed"),
            ("data", "did not pass CLAUDE_PLUGIN_DATA"),
            ("wrappers", "bin/ wrappers are missing"),
            ("agent memory", "agent-memory/"),
            ("subagent", "CLAUDE_CODE_SUBAGENT_MODEL is set"),
            ("guide", "routing guide is missing"),
            ("memory", "anthology memory: "),
            ("closed", "failing closed"),
            ("not block", "did NOT block"),
            ("disabled", "is disabled in your settings"),
            ("not installed", "is not installed, so the browser"),
            ("not found", "guardrails plugin was not found"),
            ("live missing", "has no hooks/guard.sh"),
            ("truncated", "memory index truncated"),
            ("capped", "…\""),
            ("windows", "inactive on Windows"),
        ];
        let mut seen = vec![0usize; wanted.len()];
        let mut rng = Rng::new(20261121);
        for _ in 0..1500 {
            let out = String::from_utf8(run(&gen_case(&mut rng)).stdout).unwrap();
            let decoded = json::loads(out.split('\n').next().unwrap()).unwrap();
            let text = json::dumps(&decoded, &json::PY_DEFAULT_UNICODE);
            for (i, (_, needle)) in wanted.iter().enumerate() {
                if text.contains(needle) {
                    seen[i] += 1;
                }
            }
        }
        let missing: Vec<_> = wanted
            .iter()
            .zip(&seen)
            .filter(|(_, n)| **n == 0)
            .map(|((k, _), _)| *k)
            .collect();
        assert!(missing.is_empty(), "never reached: {missing:?} ({seen:?})");
    }

    #[test]
    fn seeds_run() {
        let mut ran = 0;
        for case in seeds() {
            assert_eq!(run(&case).exit, 0);
            ran += 1;
        }
        assert!(ran > 50, "{ran}");
    }
}
