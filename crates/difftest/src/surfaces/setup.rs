//! The `setup` difftest surface (RT4): `setup::main` against
//! `plugins/anthology/scripts/anthology_setup.py`'s `main(argv)` on generated layouts.
//!
//! Oracle: `port/difftest/oracle/setup.py`; its module docs give the case format and the forced
//! changes it applies to Python's answer (F-6, F-15, the argparse prog marked D-4). Both sides
//! build the same tree (RT1a's layout entries) under a root of the same length
//! (`/tmp/dt-rt4-XXXXXXXX`), run the case's steps (argv lists, or `@WRITE@ <path> <text>`, which
//! edits a file between two commands) in that tree, and print each step's exit code, stdout and
//! stderr, then the final state of the `show` paths, with the root written `@ROOT@` and each
//! step's settings sha256 `@SHA@`.
//!
//! Each side reads the same seams from the case: doctor's `check_env`,
//! `installed_plugins_state` and `find_duplicates` (RT1b's surface tests the real doctor);
//! the rtk module (a fake with the same interface; the real one is `rtk_install.reconcile`'s
//! surface); `platform.system()` and `machine()`; the privacy scan's host name; and, Rust
//! only, `ensure_guard_binary` (BS1's surface tests the bootstrap).
//!
//! The generator draws: config folder forms (set, unset); the plugin root (valid, relative,
//! missing, not anthology, a glob character, every `version` type); the guardrails (a sibling,
//! none, registry entries for it or for anthology only, an unreadable registry); settings.json
//! (missing, objects, the rule already there, invalid JSON, not an object, odd indents, a BOM,
//! CRLF, not UTF-8, empty); state.json's `data_dir`; `CLAUDE_PLUGIN_DATA` (under the config's
//! `plugins/data`, outside it, unset); the rtk folder (a folder, a symlink, none), rtk's history
//! folder `state.rtk_data_dir()` (a folder, a symlink, a file, none) and `<ah>/bin` (a folder, a
//! symlink, none); the personal-memory folder; every fake rtk outcome and an edit
//! during the download; doctor answers; the guard binary statuses;
//! the platforms; and step sequences of preview, apply (the current sha or a stale one,
//! `--skip-approve`, `--promote`), remove (`--preview`, `--unapprove`, `--unapprove-all`),
//! usage errors and edits.
//!
//! The memory functions behind setup are real on both sides (`anthology_memory`, and
//! `memory_sync` in the oracle), `repo_root`, `repo_key` and `approve` included: both run the
//! same [`fake_git`] as `git` (the oracle image has none), and only the host name is fixed
//! (`alice-laptop`). The repo at `@ROOT@/w` has a `.git` folder and remotes (an https remote,
//! none, several, scp, `file://` and relative-path forms, odd text), or is not a repo. The
//! cases write legacy notes (clean, home path, email, private host name and IP, a
//! `type: user` note, no frontmatter, not UTF-8) under `<config>/projects/@ENC@-w/memory/`,
//! notes already in the repo's memory (identical, conflicting, a symlinked folder), a
//! `migrated.json` ledger, `approved-repos.json` and an unsafe `.gitattributes` (a symlink, a
//! folder), then preview, `--promote` (listed and
//! unlisted notes), apply, and remove with `--unapprove` or `--unapprove-all`.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anthology_core::Env;
use anthology_core::lock::Lock;
use anthology_pycompat::json::{Int, Map, Value};
use anthology_pycompat::pyerr::PyExc;
use anthology_runtime::bootstrap::GuardBinaryStatus;
use anthology_runtime::rtk_install::Reconciled;
use anthology_runtime::session_start::RtkWiring;
use anthology_runtime::setup::{self, MemoryDeps, SetupDeps, SetupDoctor};
use anthology_settings as settings;
use anthology_testkit::rt_a;
use sha2::{Digest, Sha256};

use super::fake_git;
use crate::case::{Case, Rng, Surface};
use crate::observe::{Observed, from_oracle};
use crate::oracle::Oracle;

/// The surface name (plan §1.4).
pub const NAME: &str = "setup";

/// Whether `difftest --all` runs this surface on macOS too: yes, every layout exists there.
pub const ON_MACOS: bool = true;

/// The registered implementation.
pub fn surface() -> Option<Box<dyn crate::case::Surface>> {
    Some(Box::new(Setup))
}

struct Setup;

impl Surface for Setup {
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
        from_oracle(&oracle.call("setup", &c.data))
    }

    fn run_rust(&self, c: &Case) -> Observed {
        run(&c.data)
    }
}

// --- the Rust side ------------------------------------------------------------------------

fn field<'a>(v: &'a Value, k: &str) -> Option<&'a Value> {
    v.as_object().and_then(|o| o.get(k))
}

fn str_of<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    field(v, k).and_then(Value::as_str)
}

fn opt_string(v: &Value, k: &str) -> Option<String> {
    str_of(v, k).map(str::to_string)
}

/// The fake rtk module of the case (the oracle's `FakeRtk`).
struct CaseRtk {
    root: PathBuf,
    lock_exc: Option<String>,
    exc: Option<String>,
    warning: Option<String>,
    installed: bool,
    touch: Option<String>,
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

    fn reconcile(&self, _: &Env, _: &Path, _: &Lock, timeout: f64) -> Result<Reconciled, PyExc> {
        assert!(timeout == 60.0, "setup: rtk timeout {timeout}");
        if let Some(rel) = &self.touch {
            let p = self.root.join(rel);
            fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
            let mut bytes = fs::read(&p).unwrap_or_default();
            bytes.push(b' ');
            fs::write(&p, bytes).expect("touch");
        }
        match &self.exc {
            Some(m) => Err(PyExc::Other("RuntimeError", m.clone())),
            None => Ok(Reconciled {
                rtk_path: None,
                warning: self.warning.clone(),
                installed: self.installed,
            }),
        }
    }
}

/// `socket.gethostname()`, as the oracle patches it.
fn alice_laptop() -> Option<String> {
    Some("alice-laptop".into())
}

fn linux() -> (String, String) {
    ("Linux".into(), "x86_64".into())
}

fn darwin() -> (String, String) {
    ("Darwin".into(), "arm64".into())
}

fn riscv() -> (String, String) {
    ("Linux".into(), "riscv64".into())
}

fn windows() -> (String, String) {
    ("Windows".into(), "AMD64".into())
}

/// The `@SHA@` of a settings.json that is not UTF-8 (the oracle's `NO_SHA`).
const NO_SHA: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// The sha256 setup computes for the current settings.json text.
fn settings_sha(env: &Env) -> String {
    match settings::read_text(&settings::settings_path(env)) {
        Ok(t) => Sha256::digest(t.unwrap_or_default().as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
        Err(_) => NO_SHA.into(),
    }
}

/// A `show` path's final state (the oracle's `_show`).
fn show(root: &Path, rel: &str) -> String {
    let p = root.join(rel);
    match fs::symlink_metadata(&p) {
        Ok(m) if m.file_type().is_symlink() => "<link>".into(),
        Ok(m) if m.is_dir() => format!("<dir {:o}>", m.permissions().mode() & 0o7777),
        _ => match fs::read(&p) {
            Ok(b) => String::from_utf8_lossy(&b).into_owned(),
            Err(_) => "<absent>".into(),
        },
    }
}

fn run(case: &Value) -> Observed {
    // The same root form as the oracle's `mkdtemp(prefix="dt-rt4-", dir="/tmp")`.
    let tmp = tempfile::Builder::new()
        .prefix("dt-rt4-")
        .rand_bytes(8)
        .tempdir_in("/tmp")
        .expect("tempdir");
    let root = fs::canonicalize(tmp.path()).expect("realpath");
    let root_text = root.to_string_lossy().into_owned();
    let x = |s: &str| rt_a::expand(s, &root, None);
    let mut env = Env::default();
    if let Some(vars) = field(case, "env").and_then(Value::as_object) {
        for (k, v) in vars.iter() {
            env = env.with(k, &x(v.as_str().expect("env value")));
        }
    }
    env = env.with("PATH", &fake_git::path(&root_text, "/usr/bin:/bin"));
    // The old path-keyed memory folders are named after the root (`@ENC@`).
    let enc: String = root_text
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let tree: Vec<Value> = field(case, "tree")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|mut e| {
            let p = str_of(&e, "p").map(str::to_string);
            if let (Some(m), Some(p)) = (e.as_object_mut(), p) {
                m.insert("p".into(), Value::Str(p.replace("@ENC@", &enc)));
            }
            e
        })
        .collect();
    rt_a::materialize(&root, &tree);

    let none = Value::Null;
    let doc = field(case, "doctor").unwrap_or(&none);
    let empty = || Value::Array(Vec::new());
    let checks = |_: &Env, _: &Path, _: &Path| field(doc, "checks").cloned().unwrap_or_else(empty);
    let installed_state = |_: &Env| -> (Option<Value>, &'static str) {
        let installed = field(doc, "installed").filter(|v| !matches!(v, Value::Null));
        let state = match str_of(doc, "state") {
            Some("ok") => "ok",
            Some("unreadable") => "unreadable",
            _ => "missing",
        };
        (installed.cloned(), state)
    };
    let duplicates = |_: Option<&Value>| field(doc, "duplicates").cloned().unwrap_or_else(empty);
    let status = str_of(case, "guard_binary")
        .unwrap_or("present")
        .to_string();
    let ensure = |_: &Path, budget: u64, _: &Env| {
        assert_eq!(budget, 60, "setup: the guard binary budget");
        match status.as_str() {
            "present" => GuardBinaryStatus::Present,
            "installed" => GuardBinaryStatus::Installed,
            other => GuardBinaryStatus::Failed(
                other
                    .strip_prefix("failed: ")
                    .expect("guard_binary status")
                    .to_string(),
            ),
        }
    };
    let rtk = field(case, "rtk")
        .filter(|v| !matches!(v, Value::Null))
        .map(|v| CaseRtk {
            root: root.clone(),
            lock_exc: opt_string(v, "lock_exc"),
            exc: opt_string(v, "exc"),
            warning: opt_string(v, "warning"),
            installed: field(v, "installed").and_then(Value::as_bool) == Some(true),
            touch: opt_string(v, "touch"),
        });
    let deps = SetupDeps {
        uname: match str_of(case, "uname") {
            Some("darwin") => darwin,
            Some("riscv") => riscv,
            Some("windows") => windows,
            _ => linux,
        },
        memory: MemoryDeps {
            hostname: Some(alice_laptop),
            ..MemoryDeps::default()
        },
        doctor: SetupDoctor {
            checks: &checks,
            installed_plugins_state: &installed_state,
            find_duplicates: &duplicates,
        },
        rtk: rtk.as_ref().map(|r| r as &dyn RtkWiring),
        ensure_guard_binary: &ensure,
        ..SetupDeps::default()
    };

    let mut report = String::new();
    let (mut code, mut n) = (0, 0);
    let steps = field(case, "steps")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for step in &steps {
        let args: Vec<&str> = step
            .as_array()
            .expect("step")
            .iter()
            .map(|a| a.as_str().expect("arg"))
            .collect();
        if args.first() == Some(&WRITE) {
            let target = root.join(args[1]);
            fs::create_dir_all(target.parent().expect("parent")).expect("mkdir");
            fs::write(&target, x(args[2])).expect("@WRITE@");
            continue;
        }
        n += 1;
        let sha = settings_sha(&env);
        let argv: Vec<String> = args.iter().map(|a| x(a).replace("@SHA@", &sha)).collect();
        let (c, out, err) = setup::main(&argv, &env, &deps);
        code = c;
        report.push_str(
            &format!("--- step {n}: exit {c} ---\n{out}--- stderr ---\n{err}")
                .replace(&sha, "@SHA@"),
        );
    }
    if let Some(paths) = field(case, "show").and_then(Value::as_array) {
        for rel in paths.iter().filter_map(Value::as_str) {
            report.push_str(&format!("=== {rel} ===\n{}\n", show(&root, rel)));
        }
    }
    rt_a::unlock(&root);
    Observed::text(
        code,
        &report.replace(&root_text, "@ROOT@").replace(&enc, "@ENC@"),
        "",
    )
}

// --- cases --------------------------------------------------------------------------------

/// The step that writes a file instead of running a command: `[WRITE, path, text]`.
const WRITE: &str = "@WRITE@";

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

fn file_hex(p: &str, hex: &str) -> Value {
    obj(vec![("t", s("file")), ("p", s(p)), ("hex", s(hex))])
}

fn dir(p: &str) -> Value {
    obj(vec![("t", s("dir")), ("p", s(p))])
}

fn link(p: &str, to: &str) -> Value {
    obj(vec![("t", s("link")), ("p", s(p)), ("to", s(to))])
}

fn guard(p: &str) -> Value {
    obj(vec![("t", s("guard")), ("p", s(p))])
}

/// A note as Claude Code writes it (the oracle and Rust read the same text).
fn note_text(name: &str, kind: &str, desc: &str, body: &str) -> String {
    format!("---\nname: {name}\ndescription: {desc}\nmetadata:\n  type: {kind}\n---\n\n{body}")
}

/// The legacy-note pool: file name and text (`None` is not UTF-8). The home path, the email
/// and the address are written in pieces so no file of the repo holds one whole.
fn pool() -> Vec<(&'static str, Option<String>)> {
    vec![
        (
            "clean.md",
            Some(note_text(
                "clean",
                "project",
                "A clean note",
                "Use Docker.\n",
            )),
        ),
        (
            "dirty.md",
            Some(note_text(
                "dirty",
                "reference",
                "Where things live",
                concat!("Checkout in /", "Users/alice/src\n"),
            )),
        ),
        (
            "mail.md",
            Some(note_text(
                "mail",
                "reference",
                "Contact",
                concat!("Ask alice", "@", "mail.acme.zz\n"),
            )),
        ),
        (
            "user-me.md",
            Some(note_text("user-me", "user", "About me", "Body.\n")),
        ),
        (
            "host.md",
            Some(note_text(
                "host",
                "project",
                "Host",
                "Runs on alice-laptop\n",
            )),
        ),
        (
            "ip.md",
            Some(note_text(
                "ip",
                "reference",
                "Hosts",
                concat!("server 10.1", ".2.3\n"),
            )),
        ),
        ("plain.md", Some("just text, no frontmatter\n".into())),
        ("bad.md", None),
        ("MEMORY.md", Some("- index\n".into())),
    ]
}

/// Names `--promote` may be given: the pool's, and some that are not notes.
fn promote_names() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = pool().iter().map(|(n, _)| *n).collect();
    v.extend(["a.md", "../clean.md", "sub/clean.md", ""]);
    v
}

fn note_entry(p: &str, i: usize) -> Value {
    match &pool()[i].1 {
        Some(text) => file(p, text),
        None => file_hex(p, "2d2d2d0aff0a"),
    }
}

/// The sha256 `migrate` records for pool note `i`.
fn pool_sha(i: usize) -> String {
    let bytes = match &pool()[i].1 {
        Some(text) => text.clone().into_bytes(),
        None => vec![0x2d, 0x2d, 0x2d, 0x0a, 0xff, 0x0a],
    };
    Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

const PLUGIN: &str = "p/anthology";
const GUARD: &str = "p/anthology-guardrails";
const ROOT_ARG: &str = "@ROOT@/p/anthology";
const CWD: &str = "@ROOT@/w";
/// `repo_key` of the repo at `@ROOT@/w` with its default origin.
const KEY: &str = "github.com/example/w";
/// The fake git's remotes file of that repo.
const REMOTES: &str = "w/.git/remotes";
/// Remotes files whose key `normalize_remote` derives in each of its ways.
const REMOTE_FORMS: [&str; 9] = [
    "upstream https://github.com/Other/W.git\norigin git@github.com:Example/W.git\n",
    "zeta ssh://user:pw@host.example:22/a/b.git/\nalpha https://h.example:8443//x//y/\n",
    "origin file://@ROOT@/elsewhere/repo.git\n",
    "origin ../up.git\n",
    "origin ./sub/../w2\n",
    "origin ~/repos/x.git\n",
    "origin host.example:team/proj\n",
    "origin\n",
    "origin HTTP://User@Example.COM:80/A/B/\n",
];

/// A remote URL: a scheme or scp form around odd text.
fn odd_url(rng: &mut Rng) -> String {
    let head = *rng.pick(&["https://", "ssh://git@", "file://", "", "git@", "/", "../"]);
    let mid = odd_text(rng, 12);
    let tail = *rng.pick(&[".git", "/", ":22/x", "", "//a//b.git/"]);
    format!("{head}{mid}{tail}").replace(['\n', '\r'], "")
}
const MANIFEST: &str = r#"{"name": "anthology", "version": "0.1.0"}"#;
/// `plugin_read_rule` of the plugin root (the root starts with `/`).
const RULE: &str = "Read(/@ROOT@/p/**)";
const GUARD_ENTRY: &str = r#"{"version": 2, "plugins": {"anthology-guardrails@m": [{"scope": "user", "installPath": "@ROOT@/p/anthology-guardrails", "lastUpdated": "2026-01-01"}]}}"#;
const ANTHOLOGY_ONLY: &str = r#"{"version": 2, "plugins": {"anthology@m": [{"scope": "user", "installPath": "@ROOT@/p/anthology"}]}}"#;

fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

fn preview_cmd() -> Vec<String> {
    args(&["preview", "--plugin-root", ROOT_ARG, "--cwd", CWD])
}

fn apply_cmd(extra: &[&str]) -> Vec<String> {
    let mut a = args(&[
        "apply",
        "--plugin-root",
        ROOT_ARG,
        "--expect-sha256",
        "@SHA@",
        "--cwd",
        CWD,
    ]);
    a.extend(args(extra));
    a
}

fn remove_cmd(extra: &[&str]) -> Vec<String> {
    let mut a = args(&["remove", "--plugin-root", ROOT_ARG, "--cwd", CWD]);
    a.extend(args(extra));
    a
}

/// A case builder.
struct B {
    tree: Vec<Value>,
    env: Map,
    uname: &'static str,
    doctor: Value,
    rtk: Value,
    guard_binary: String,
    steps: Vec<Vec<String>>,
}

impl B {
    /// A healthy layout: the plugin, a sibling guardrails, the config in `@ROOT@/cfg`, the data
    /// folder under its `plugins/data`, a repo at `@ROOT@/w` whose origin gives [`KEY`], an rtk
    /// that installs, and the [`fake_git`].
    fn new() -> B {
        let mut env = Map::new();
        env.insert("HOME".into(), s("@ROOT@/home"));
        env.insert("CLAUDE_CONFIG_DIR".into(), s("@ROOT@/cfg"));
        env.insert(
            "CLAUDE_PLUGIN_DATA".into(),
            s("@ROOT@/cfg/plugins/data/anthology"),
        );
        B {
            tree: [
                file(&format!("{PLUGIN}/.claude-plugin/plugin.json"), MANIFEST),
                guard(GUARD),
                dir("home"),
                dir("w/.git"),
                file(REMOTES, "origin https://github.com/example/w.git\n"),
            ]
            .into_iter()
            .chain(fake_git::entries())
            .collect(),
            env,
            uname: "linux",
            doctor: obj(vec![
                (
                    "checks",
                    Value::Array(vec![obj(vec![
                        ("name", s("rtk")),
                        ("ok", Value::Bool(true)),
                        ("detail", s("rtk 9.9.9")),
                    ])]),
                ),
                ("installed", Value::Null),
                ("state", s("missing")),
                ("duplicates", Value::Array(Vec::new())),
            ]),
            rtk: obj(vec![
                ("lock_exc", Value::Null),
                ("exc", Value::Null),
                ("warning", Value::Null),
                ("installed", Value::Bool(true)),
                ("touch", Value::Null),
            ]),
            guard_binary: "present".into(),
            steps: Vec::new(),
        }
    }

    /// The config folder, relative to the root.
    fn config(&self) -> String {
        match self.env.get("CLAUDE_CONFIG_DIR").and_then(Value::as_str) {
            Some(c) if !c.is_empty() => c.trim_start_matches("@ROOT@/").to_string(),
            _ => "home/.claude".into(),
        }
    }

    fn at(&self, rel: &str) -> String {
        format!("{}/{rel}", self.config())
    }

    /// Adds `e`, replacing an entry at the same path.
    fn put(mut self, e: Value) -> B {
        let p = opt_string(&e, "p");
        self.tree.retain(|o| opt_string(o, "p") != p);
        self.tree.push(e);
        self
    }

    fn settings(self, text: &str) -> B {
        let p = self.at("settings.json");
        self.put(file(&p, text))
    }

    fn state(self, text: &str) -> B {
        let p = self.at("anthology/state.json");
        self.put(file(&p, text))
    }

    fn installed(self, text: &str) -> B {
        let p = self.at("plugins/installed_plugins.json");
        self.put(file(&p, text))
    }

    /// Pool note `i` as a legacy note of the repo at `@ROOT@/w`.
    fn legacy(self, i: usize) -> B {
        let p = format!("{}/projects/@ENC@-w/memory/{}", self.config(), pool()[i].0);
        self.put(note_entry(&p, i))
    }

    /// Pool note `i` (or `text` in its place) already in the repo's memory.
    fn repo_note(self, i: usize, text: Option<&str>) -> B {
        let p = format!("w/.claude/memory/{}", pool()[i].0);
        match text {
            Some(t) => self.put(file(&p, t)),
            None => self.put(note_entry(&p, i)),
        }
    }

    /// `<ah>/migrated.json` naming the pool notes `done`.
    fn ledger(self, done: &[usize]) -> B {
        let shas: Vec<String> = done
            .iter()
            .map(|i| format!("\"{}\"", pool_sha(*i)))
            .collect();
        let p = self.at("anthology/migrated.json");
        self.put(file(
            &p,
            &format!("{{\"version\": 1, \"done\": [{}]}}", shas.join(", ")),
        ))
    }

    /// `<ah>/approved-repos.json` approving `keys`.
    fn approvals(self, keys: &[&str]) -> B {
        let repos: Vec<String> = keys
            .iter()
            .map(|k| format!("{k:?}: {{\"approved\": \"2026-01-01\"}}"))
            .collect();
        let p = self.at("anthology/approved-repos.json");
        self.put(file(
            &p,
            &format!("{{\"version\": 1, \"repos\": {{{}}}}}", repos.join(", ")),
        ))
    }

    fn no_guard(mut self) -> B {
        self.tree.retain(|e| str_of(e, "t") != Some("guard"));
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

    fn set(mut v: Value, k: &str, x: Value) -> Value {
        if let Some(m) = v.as_object_mut() {
            m.insert(k.into(), x);
        }
        v
    }

    fn rtk(mut self, k: &str, v: Value) -> B {
        self.rtk = B::set(self.rtk, k, v);
        self
    }

    /// `@ROOT@/w` is not a git repo.
    fn not_repo(mut self) -> B {
        self.tree
            .retain(|e| !str_of(e, "p").is_some_and(|p| p.starts_with("w/.git")));
        self.put(dir("w"))
    }

    /// The repo's remotes (`<name> <url>` lines); `None` has none, so its key is its path.
    fn remotes(self, text: Option<&str>) -> B {
        match text {
            Some(t) => self.put(file(REMOTES, t)),
            None => {
                let mut b = self;
                b.tree.retain(|e| str_of(e, "p") != Some(REMOTES));
                b
            }
        }
    }

    fn doc(mut self, k: &str, v: Value) -> B {
        self.doctor = B::set(self.doctor, k, v);
        self
    }

    fn step(mut self, a: Vec<String>) -> B {
        self.steps.push(a);
        self
    }

    fn write(self, rel: &str, text: &str) -> B {
        self.step(args(&[WRITE, rel, text]))
    }

    fn build(self) -> Value {
        let show: Vec<Value> = [
            "settings.json",
            "anthology/state.json",
            "anthology/bin",
            "anthology/personal-memory",
            "plugins/data/anthology/rtk",
            "anthology/rtk",
        ]
        .iter()
        .map(|r| s(&self.at(r)))
        .chain([s("data/rtk"), s("elsewhere/history/history.db")])
        .chain(
            [
                "anthology/migrated.json",
                "anthology/approved-repos.json",
                "anthology/personal-memory/MEMORY.md",
            ]
            .iter()
            .map(|r| s(&self.at(r))),
        )
        .chain(pool().iter().flat_map(|(n, _)| {
            [
                s(&format!("w/.claude/memory/{n}")),
                s(&self.at(&format!("anthology/personal-memory/{n}"))),
            ]
        }))
        .chain([s("w/.claude/memory"), s("w/.claude"), s("w/.gitattributes")])
        .collect();
        obj(vec![
            ("tree", Value::Array(self.tree)),
            ("env", Value::Object(self.env)),
            ("uname", s(self.uname)),
            ("doctor", self.doctor),
            ("rtk", self.rtk),
            ("guard_binary", s(&self.guard_binary)),
            (
                "steps",
                Value::Array(
                    self.steps
                        .into_iter()
                        .map(|a| Value::Array(a.iter().map(|x| s(x)).collect()))
                        .collect(),
                ),
            ),
            ("show", Value::Array(show)),
        ])
    }
}

fn seeds() -> Vec<Value> {
    let mut out = Vec::new();
    let mut add = |b: B| out.push(b.build());
    // preview and apply
    add(B::new().step(preview_cmd()));
    add(B::new().step(apply_cmd(&[])));
    add(B::new()
        .step(preview_cmd())
        .step(apply_cmd(&[]))
        .step(preview_cmd()));
    add(B::new().step(apply_cmd(&[])).step(apply_cmd(&[])));
    add(B::new()
        .settings("{\"permissions\": {\"allow\": [\"Bash(ls)\"]}, \"model\": \"x\"}\n")
        .step(preview_cmd())
        .step(apply_cmd(&[])));
    add(B::new()
        .settings(&format!(
            "{{\n  \"permissions\": {{\n    \"allow\": [\"{RULE}\"]\n  }}\n}}\n"
        ))
        .step(apply_cmd(&[])));
    add(B::new().step(args(&[
        "apply",
        "--plugin-root",
        ROOT_ARG,
        "--expect-sha256",
        "deadbeef",
    ])));
    // the rtk step
    let p = B::new().at("settings.json");
    add(B::new().rtk("touch", s(&p)).step(apply_cmd(&[])));
    add(B::new()
        .settings("{}\n")
        .rtk("touch", s(&p))
        .step(apply_cmd(&[])));
    add(B::new()
        .rtk("installed", Value::Bool(false))
        .step(apply_cmd(&[])));
    add(B::new()
        .rtk("warning", s("rtk 1.0 is outdated; using it anyway"))
        .step(apply_cmd(&[])));
    add(B::new()
        .rtk("warning", s(""))
        .rtk("installed", Value::Bool(false))
        .step(apply_cmd(&[])));
    add(B::new()
        .rtk("exc", s("download refused"))
        .step(apply_cmd(&[])));
    add(B::new().rtk("exc", s("")).step(apply_cmd(&[])));
    add(B::new()
        .rtk("exc", s(&"é".repeat(300)))
        .step(apply_cmd(&[])));
    add(B::new()
        .rtk("lock_exc", s("rtk.lock is not valid JSON"))
        .step(preview_cmd())
        .step(apply_cmd(&[])));
    let mut b = B::new();
    b.rtk = Value::Null;
    add(b.step(preview_cmd()).step(apply_cmd(&[])));
    add(B::new()
        .env("CLAUDE_PLUGIN_DATA", None)
        .step(preview_cmd())
        .step(apply_cmd(&[])));
    add(B::new()
        .env("CLAUDE_PLUGIN_DATA", Some(""))
        .state(r#"{"data_dir": "@ROOT@/cfg/plugins/data/anthology"}"#)
        .step(preview_cmd())
        .step(apply_cmd(&[]))
        .step(remove_cmd(&[])));
    add(B::new()
        .env("CLAUDE_PLUGIN_DATA", None)
        .state(r#"{"data_dir": 7}"#)
        .step(apply_cmd(&[])));
    // remove
    add(B::new().step(remove_cmd(&[])));
    add(B::new().step(remove_cmd(&["--preview"])));
    add(B::new()
        .step(apply_cmd(&[]))
        .step(remove_cmd(&["--preview"]))
        .step(remove_cmd(&[])));
    add(B::new()
        .settings("{\"model\": \"x\"}\n")
        .step(apply_cmd(&[]))
        .step(remove_cmd(&["--unapprove"])));
    add(B::new()
        .step(apply_cmd(&[]))
        .step(remove_cmd(&["--unapprove-all", "--preview"]))
        .step(remove_cmd(&["--unapprove-all"])));
    add(B::new()
        .step(apply_cmd(&[]))
        .write(&p, "{")
        .step(remove_cmd(&[])));
    add(B::new()
        .step(apply_cmd(&[]))
        .write(&p, "{\"permissions\": []}")
        .step(remove_cmd(&["--preview"])));
    // <ah>/bin
    let bin = B::new().at("anthology/bin");
    let bin_file = format!("{bin}/anthology");
    add(B::new()
        .put(file(&bin_file, "#!/bin/sh\n"))
        .step(remove_cmd(&["--preview"]))
        .step(remove_cmd(&[])));
    add(B::new()
        .installed(GUARD_ENTRY)
        .put(file(&bin_file, "#!/bin/sh\n"))
        .step(remove_cmd(&["--preview"]))
        .step(remove_cmd(&[])));
    add(B::new()
        .installed("{not json")
        .put(file(&bin_file, ""))
        .step(remove_cmd(&[])));
    add(B::new()
        .installed(ANTHOLOGY_ONLY)
        .put(file(&bin_file, ""))
        .step(remove_cmd(&[])));
    add(B::new()
        .put(dir("elsewhere/bin"))
        .put(link(&bin, "@ROOT@/elsewhere/bin"))
        .step(remove_cmd(&["--preview"]))
        .step(remove_cmd(&[])));
    // the rtk folder
    let rtk = B::new().at("plugins/data/anthology/rtk");
    add(B::new()
        .put(file(&format!("{rtk}/9.9.9/rtk"), "bin"))
        .step(remove_cmd(&["--preview"]))
        .step(remove_cmd(&[])));
    add(B::new()
        .put(dir("elsewhere/rtk"))
        .put(link(&rtk, "@ROOT@/elsewhere/rtk"))
        .step(remove_cmd(&[])));
    add(B::new()
        .env("CLAUDE_PLUGIN_DATA", Some("@ROOT@/data"))
        .put(file("data/rtk/9.9.9/rtk", "bin"))
        .step(remove_cmd(&["--preview"]))
        .step(remove_cmd(&[])));
    add(B::new()
        .env("CLAUDE_PLUGIN_DATA", None)
        .step(remove_cmd(&[])));
    // rtk's history folder (state.rtk_data_dir): a folder, a symlink, a file; none is above
    let history = B::new().at("anthology/rtk");
    add(B::new()
        .put(file(&format!("{history}/history.db"), "commands"))
        .put(file(&format!("{history}/tee/1_x.log"), "out"))
        .step(apply_cmd(&[]))
        .step(remove_cmd(&["--preview"]))
        .step(remove_cmd(&[])));
    add(B::new()
        .put(file("elsewhere/history/history.db", "keep"))
        .put(link(&history, "@ROOT@/elsewhere/history"))
        .step(remove_cmd(&["--preview"]))
        .step(remove_cmd(&[])));
    add(B::new()
        .put(file(&history, "not a folder"))
        .step(remove_cmd(&["--preview"]))
        .step(remove_cmd(&[])));
    let b = B::new().env("CLAUDE_CONFIG_DIR", None);
    let db = b.at("anthology/rtk/history.db");
    add(b.put(file(&db, "x")).step(remove_cmd(&[])));
    // platforms
    for cmd in [preview_cmd(), apply_cmd(&[]), remove_cmd(&["--preview"])] {
        let mut b = B::new();
        b.uname = "windows";
        add(b.step(cmd));
    }
    for u in ["darwin", "riscv"] {
        let mut b = B::new();
        b.uname = u;
        add(b.step(preview_cmd()).step(apply_cmd(&[])));
    }
    // usage errors (D-4)
    for a in [
        vec![],
        args(&["bogus"]),
        args(&["preview"]),
        args(&["preview", "--plugin-root"]),
        args(&["apply", "--plugin-root", ROOT_ARG]),
        args(&["remove", "--plugin-root", ROOT_ARG, "--bogus"]),
        args(&["--json"]),
    ] {
        add(B::new().step(a));
    }
    // the guardrails folder
    add(B::new().no_guard().step(preview_cmd()).step(apply_cmd(&[])));
    add(B::new()
        .installed(ANTHOLOGY_ONLY)
        .step(preview_cmd())
        .step(apply_cmd(&[])));
    add(B::new()
        .installed(GUARD_ENTRY)
        .step(preview_cmd())
        .step(apply_cmd(&[])));
    // malformed settings.json
    for text in [
        "{",
        "[]",
        "\"x\"",
        "",
        "{\n\t\"a\": 1\n}\n",
        "{\n   \"a\": [\n      1\n   ]\n}",
        "{\"permissions\": {\"allow\": \"Bash(ls)\"}}",
        "{\"permissions\": []}",
        "\u{feff}{}",
        "{\"a\": 1}\r\n",
        "{\"a\": \"é\u{1F600}\"}",
    ] {
        add(B::new()
            .settings(text)
            .step(preview_cmd())
            .step(apply_cmd(&[])));
    }
    add(B::new()
        .put(file_hex(&p, "7b22ff227d"))
        .step(preview_cmd())
        .step(apply_cmd(&[]))
        .step(remove_cmd(&[])));
    // the plugin root
    for root in ["p/anthology", "@ROOT@/p/none", "@ROOT@/p", ""] {
        add(B::new()
            .step(args(&["preview", "--plugin-root", root]))
            .step(args(&[
                "apply",
                "--plugin-root",
                root,
                "--expect-sha256",
                "@SHA@",
            ])));
    }
    for manifest in [
        r#"{"name": "other"}"#,
        r#"{"name": "anthology"}"#,
        r#"{"name": "anthology", "version": 3}"#,
        r#"{"name": "anthology", "version": true}"#,
        r#"{"name": "anthology", "version": null}"#,
        r#"{"name": "anthology", "version": [1, 2]}"#,
    ] {
        add(B::new()
            .put(file(
                &format!("{PLUGIN}/.claude-plugin/plugin.json"),
                manifest,
            ))
            .step(apply_cmd(&[])));
    }
    // memory
    add(B::new().not_repo().step(preview_cmd()).step(apply_cmd(&[])));
    add(B::new()
        .put(dir("elsewhere"))
        .put(link("w/.gitattributes", "@ROOT@/elsewhere"))
        .step(apply_cmd(&[])));
    add(B::new()
        .put(dir("w/.gitattributes"))
        .step(apply_cmd(&[]))
        .step(remove_cmd(&["--unapprove"])));
    add(B::new()
        .remotes(None)
        .step(apply_cmd(&[]))
        .step(preview_cmd()));
    add(B::new()
        .put(file(
            "w/.gitattributes",
            "*.md text\n.claude/memory/MEMORY.md   merge=union",
        ))
        .step(apply_cmd(&[]))
        .step(apply_cmd(&[])));
    for remotes in REMOTE_FORMS {
        add(B::new()
            .remotes(Some(remotes))
            .step(apply_cmd(&[]))
            .step(remove_cmd(&["--unapprove", "--preview"])));
    }
    add(B::new().step(apply_cmd(&["--skip-approve"])));
    add(B::new().step(apply_cmd(&["--skip-approve", "--promote", "a.md"])));
    add(B::new().not_repo().step(apply_cmd(&["--promote", "a.md"])));
    add(B::new().step(apply_cmd(&[
        "--promote",
        "b.md",
        "--promote",
        "a.md",
        "--promote",
        "b.md",
    ])));
    let pm = B::new().at("anthology/personal-memory/x.md");
    add(B::new().put(file(&pm, "x")).step(apply_cmd(&[])));
    // legacy notes, promotion, approvals (real memory functions)
    let all = || (0..pool().len()).fold(B::new(), |b, i| b.legacy(i));
    let every: Vec<&str> = pool().iter().flat_map(|(n, _)| ["--promote", *n]).collect();
    add(all().step(preview_cmd()));
    add(all().step(apply_cmd(&[])));
    add(all()
        .step(apply_cmd(&["--promote", "clean.md"]))
        .step(apply_cmd(&[])));
    add(all().step(apply_cmd(&every)).step(preview_cmd()));
    add(all().step(apply_cmd(&["--promote", "MEMORY.md"])));
    add(all()
        .repo_note(2, Some("the repo's own note\n"))
        .repo_note(0, None)
        .step(preview_cmd())
        .step(apply_cmd(&[
            "--promote",
            "clean.md",
            "--promote",
            "mail.md",
        ])));
    add(all()
        .ledger(&[0, 1])
        .step(preview_cmd())
        .step(apply_cmd(&[])));
    add(all()
        .put(dir("elsewhere"))
        .put(link("w/.claude/memory", "@ROOT@/elsewhere"))
        .step(preview_cmd())
        .step(apply_cmd(&["--promote", "clean.md"])));
    add(all()
        .put(file("w/.claude", "x"))
        .step(preview_cmd())
        .step(apply_cmd(&["--promote", "clean.md"])));
    add(all()
        .put(file("w/.claude/memory", "x"))
        .step(preview_cmd())
        .step(apply_cmd(&["--promote", "clean.md"])));
    add(all().step(apply_cmd(&["--promote", "clean.md", "--skip-approve"])));
    add(all()
        .not_repo()
        .step(preview_cmd())
        .step(apply_cmd(&["--promote", "clean.md"])));
    add(all()
        .step(apply_cmd(&["--promote", "clean.md"]))
        .step(remove_cmd(&["--unapprove"])));
    add(B::new()
        .approvals(&[KEY, "a/b"])
        .step(remove_cmd(&["--preview"]))
        .step(remove_cmd(&["--unapprove", "--preview"]))
        .step(remove_cmd(&["--unapprove-all", "--preview"]))
        .step(remove_cmd(&["--unapprove"]))
        .step(remove_cmd(&["--unapprove-all"])));
    add(B::new()
        .approvals(&["c/d", "a/b"])
        .step(remove_cmd(&["--unapprove-all"]))
        .step(preview_cmd()));
    add(B::new()
        .approvals(&[KEY])
        .step(preview_cmd())
        .step(apply_cmd(&[]))
        .step(remove_cmd(&["--unapprove"])));
    add(B::new()
        .put(file("cfg/anthology/approved-repos.json", "not json"))
        .step(remove_cmd(&["--unapprove-all"])));
    // doctor
    add(B::new()
        .doc("state", s("ok"))
        .doc(
            "installed",
            obj(vec![("version", Value::Int(Int::Small(2)))]),
        )
        .doc(
            "duplicates",
            Value::Array(vec![obj(vec![
                ("plugin", s("superpowers@x")),
                ("scope", s("user")),
                ("command", s("/plugin uninstall superpowers@x")),
            ])]),
        )
        .step(preview_cmd()));
    add(B::new()
        .doc("state", s("unreadable"))
        .doc("checks", Value::Array(Vec::new()))
        .step(preview_cmd()));
    // the guard binary
    for st in ["installed", "failed: download failed (é)"] {
        let mut b = B::new();
        b.guard_binary = st.into();
        add(b.step(apply_cmd(&[])));
    }
    // the config folder from HOME
    let b = B::new().env("CLAUDE_CONFIG_DIR", None);
    let data = format!("@ROOT@/{}", b.at("plugins/data/anthology"));
    add(b
        .env("CLAUDE_PLUGIN_DATA", Some(&data))
        .step(preview_cmd())
        .step(apply_cmd(&[]))
        .step(remove_cmd(&[])));
    add(B::new()
        .step(args(&["preview", "--plugin-root", ROOT_ARG, "--json"]))
        .step(args(&["remove", "--plugin-root", ROOT_ARG])));
    // CLAUDE_CODE_SUBAGENT_MODEL: offered, removed on yes, restored by remove
    for text in MODEL_SETTINGS {
        add(B::new()
            .settings(text)
            .step(preview_cmd())
            .step(apply_cmd(&["--remove-subagent-model"]))
            .step(remove_cmd(&["--preview"]))
            .step(remove_cmd(&[])));
    }
    out
}

// --- the generator --------------------------------------------------------------------------

fn odd_text(rng: &mut Rng, n: usize) -> String {
    match rng.below(4) {
        0 => String::new(),
        1 => rng.ascii_string(n),
        _ => rng.string(n),
    }
}

const SETTINGS: [&str; 19] = [
    "{}",
    "{}\n",
    "{\"model\": \"x\"}\n",
    "{\"permissions\": {\"allow\": [\"Bash(ls)\"], \"deny\": []}}",
    "{\n    \"permissions\": {\n        \"allow\": [\n            \"Read(/@ROOT@/p/**)\"\n        ]\n    }\n}\n",
    "{\n\t\"worktree\": {\"baseRef\": \"main\"}\n}",
    "{\"worktree\": {\"baseRef\": \"head\"}, \"permissions\": {\"allow\": [\"Bash(rtk *)\"]}}",
    "{",
    "[]",
    "null",
    "",
    "  \n",
    "\u{feff}{}",
    "{\"a\": 1}\r\n",
    "{\"permissions\": {\"allow\": \"x\"}}",
    "{\"permissions\": 3}",
    "{\"worktree\": []}",
    "{\"a\": \"é\u{1F600}\u{2028}\"}",
    "{\"a\": 1, \"a\": 2}",
];

/// settings.json texts setting `CLAUDE_CODE_SUBAGENT_MODEL`: alone, beside other keys, first of
/// two, twice, blank, and a non-object `env`.
const MODEL_SETTINGS: [&str; 7] = [
    "{\"env\": {\"CLAUDE_CODE_SUBAGENT_MODEL\": \"m\"}}\n",
    "{\n  \"model\": \"x\",\n  \"env\": {\n    \"A\": \"1\",\n    \"CLAUDE_CODE_SUBAGENT_MODEL\": \"m\"\n  }\n}\n",
    "{\"env\":{\"CLAUDE_CODE_SUBAGENT_MODEL\":\"m\",\"B\":\"2\"},\"permissions\":{\"allow\":[]}}",
    "{\"env\": {\"CLAUDE_CODE_SUBAGENT_MODEL\": \"a\", \"CLAUDE_CODE_SUBAGENT_MODEL\": \"m\"}}",
    "{\"env\": {\"CLAUDE_CODE_SUBAGENT_MODEL\": \"  \"}}",
    "{\"env\": \"CLAUDE_CODE_SUBAGENT_MODEL\"}",
    "{\r\n\t\"env\": {\r\n\t\t\"CLAUDE_CODE_SUBAGENT_MODEL\": \"\\u00e9\"\r\n\t}\r\n}\r\n",
];

fn gen_cmd(rng: &mut Rng) -> Vec<String> {
    match rng.below(20) {
        0..=4 => {
            let mut a = preview_cmd();
            if rng.chance(0.2) {
                a.truncate(3);
            }
            if rng.chance(0.1) {
                a.push("--json".into());
            }
            a
        }
        5..=10 => {
            let mut extra = Vec::new();
            if rng.chance(0.15) {
                extra.push("--skip-approve");
            }
            if rng.chance(0.25) {
                for _ in 0..1 + rng.index(3) {
                    extra.extend(["--promote", *rng.pick(&promote_names())]);
                }
            }
            if rng.chance(0.3) {
                extra.push("--remove-subagent-model");
            }
            let mut a = apply_cmd(&extra);
            if rng.chance(0.12) {
                a[4] = rng
                    .pick(&[
                        "deadbeef",
                        "",
                        "E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855",
                    ])
                    .to_string();
            }
            a
        }
        11..=16 => {
            let mut extra = Vec::new();
            if rng.chance(0.4) {
                extra.push("--preview");
            }
            if rng.chance(0.2) {
                extra.push("--unapprove");
            }
            if rng.chance(0.15) {
                extra.push("--unapprove-all");
            }
            remove_cmd(&extra)
        }
        17 => args(rng.pick(&[
            &[][..],
            &["bogus"][..],
            &["preview"][..],
            &["apply", "--plugin-root", ROOT_ARG][..],
            &["remove", "--plugin-root", ROOT_ARG, "--x"][..],
            &["preview", "--plugin-root"][..],
            &["apply", "--expect-sha256", "x"][..],
        ])),
        _ => {
            let root = *rng.pick(&[
                "p/anthology",
                "@ROOT@/p/none",
                "@ROOT@/p",
                "@ROOT@/p/anthology/",
                "@ROOT@/p/anth*",
            ]);
            match rng.below(3) {
                0 => args(&["preview", "--plugin-root", root]),
                1 => args(&["apply", "--plugin-root", root, "--expect-sha256", "@SHA@"]),
                _ => args(&["remove", "--plugin-root", root, "--preview"]),
            }
        }
    }
}

fn gen_case(rng: &mut Rng) -> Value {
    let mut b = B::new();
    // the config and data folders
    if rng.chance(0.15) {
        b = b.env("CLAUDE_CONFIG_DIR", None);
        let data = format!("@ROOT@/{}", b.at("plugins/data/anthology"));
        b = b.env("CLAUDE_PLUGIN_DATA", Some(&data));
    }
    match rng.below(8) {
        0 => b = b.env("CLAUDE_PLUGIN_DATA", None),
        1 => b = b.env("CLAUDE_PLUGIN_DATA", Some("")),
        2 => b = b.env("CLAUDE_PLUGIN_DATA", Some("@ROOT@/data")),
        _ => {}
    }
    match rng.below(8) {
        0 => {
            let d = b.at("plugins/data/anthology");
            b = b.state(&format!("{{\"data_dir\": \"@ROOT@/{d}\"}}"));
        }
        1 => b = b.state(r#"{"data_dir": "@ROOT@/data", "setup_done": "0.0.1"}"#),
        2 => {
            b = b.state(rng.pick(&[
                "not json",
                "[]",
                r#"{"data_dir": 5}"#,
                r#"{"data_dir": ""}"#,
            ]))
        }
        _ => {}
    }
    // the plugin
    if rng.chance(0.12) {
        let m = *rng.pick(&[
            r#"{"name": "other"}"#,
            r#"{"name": "anthology"}"#,
            r#"{"name": "anthology", "version": 1.5}"#,
            r#"{"name": "anthology", "version": false}"#,
            r#"{"name": "anthology", "version": "é"}"#,
            "{",
        ]);
        b = b.put(file(&format!("{PLUGIN}/.claude-plugin/plugin.json"), m));
    }
    // the guardrails
    match rng.below(10) {
        0 => b = b.no_guard(),
        1 => b = b.installed(GUARD_ENTRY),
        2 => b = b.installed(ANTHOLOGY_ONLY),
        3 => b = b.installed(rng.pick(&["{", "[]", "{\"plugins\": []}", ""])),
        4 => b = b.no_guard().installed(GUARD_ENTRY),
        _ => {}
    }
    // settings.json
    let settings_path = b.at("settings.json");
    match rng.below(10) {
        0 | 1 => {}
        2 => b = b.settings(&format!("{{\"x\": {}}}", rng.range(-5, 5))),
        3 if rng.chance(0.2) => {
            b = b.put(file_hex(
                &settings_path,
                rng.pick(&["ff", "7b22ff227d", "c3"]),
            ))
        }
        _ => b = b.settings(rng.pick(&SETTINGS)),
    }
    // CLAUDE_CODE_SUBAGENT_MODEL in settings.json, the project's local file or the process
    // environment ("m" there is the copy Claude Code makes of a settings file's value)
    let model_set = rng.chance(0.3);
    if model_set {
        b = b.settings(rng.pick(&MODEL_SETTINGS));
    }
    let model_where = rng.below(10);
    if model_where == 1 {
        b = b.env(
            "CLAUDE_CODE_SUBAGENT_MODEL",
            Some(rng.pick(&["m", "x", "  "])),
        );
    }
    // the rtk folder and <ah>/bin
    match rng.below(6) {
        0 => {
            let p = b.at("plugins/data/anthology/rtk/9.9.9/rtk");
            b = b.put(file(&p, "bin"));
        }
        1 => {
            let p = b.at("plugins/data/anthology/rtk");
            b = b
                .put(dir("elsewhere/rtk"))
                .put(link(&p, "@ROOT@/elsewhere/rtk"));
        }
        2 => b = b.put(file("data/rtk/9.9.9/rtk", "bin")),
        _ => {}
    }
    match rng.below(5) {
        0 => {
            let p = b.at("anthology/bin/anthology");
            b = b.put(file(&p, "#!/bin/sh\n"));
        }
        1 => {
            let p = b.at("anthology/bin");
            b = b
                .put(dir("elsewhere/bin"))
                .put(link(&p, "@ROOT@/elsewhere/bin"));
        }
        _ => {}
    }
    // rtk's history folder (state.rtk_data_dir)
    let history = b.at("anthology/rtk");
    match rng.below(6) {
        0 => {
            b = b
                .put(file(&format!("{history}/history.db"), "commands"))
                .put(file(&format!("{history}/tee/1_x.log"), "out"));
        }
        1 => {
            b = b
                .put(file("elsewhere/history/history.db", "keep"))
                .put(link(&history, "@ROOT@/elsewhere/history"));
        }
        2 => b = b.put(file(&history, "")),
        _ => {}
    }
    if rng.chance(0.1) {
        let p = b.at("anthology/personal-memory");
        b = b.put(dir(&p));
    }
    // the rtk module
    b = match rng.below(10) {
        0 => {
            b.rtk = Value::Null;
            b
        }
        1 => {
            let t = odd_text(rng, 80);
            b.rtk("lock_exc", s(&t))
        }
        2 => {
            let t = odd_text(rng, 260);
            b.rtk("exc", s(&t))
        }
        3 => {
            let t = odd_text(rng, 60);
            b.rtk("warning", s(&t))
        }
        4 => b.rtk("installed", Value::Bool(false)),
        5 if rng.chance(0.5) => b.rtk("touch", s(&settings_path)),
        _ => b,
    };
    // doctor and memory
    if rng.chance(0.2) {
        let plugin = odd_text(rng, 12);
        let state = *rng.pick(&["ok", "missing", "unreadable"]);
        b = b
            .doc("state", s(state))
            .doc("installed", obj(vec![("plugins", obj(vec![]))]))
            .doc(
                "duplicates",
                Value::Array(vec![obj(vec![
                    ("plugin", s(&plugin)),
                    ("scope", s("user")),
                    ("command", s("/plugin uninstall x")),
                ])]),
            );
    }
    match rng.below(14) {
        0 => b = b.not_repo(),
        1 => {
            b = b
                .put(dir("elsewhere"))
                .put(link("w/.gitattributes", "@ROOT@/elsewhere"))
        }
        2 => b = b.put(dir("w/.gitattributes")),
        3 => b = b.remotes(None),
        4 => {
            let u = odd_url(rng);
            b = b.remotes(Some(&format!("origin {u}\n")));
        }
        5 => b = b.remotes(Some(rng.pick(&REMOTE_FORMS))),
        6 => {
            let t = *rng.pick(&["", "x\n", ".claude/memory/MEMORY.md merge=union\n"]);
            b = b.put(file("w/.gitattributes", t));
        }
        _ => {}
    }
    // legacy notes, what the repo's memory already holds, the ledger, approvals
    let mut listed = Vec::new();
    if rng.chance(0.4) {
        let n = pool().len();
        for i in 0..n {
            if rng.chance(0.55) {
                b = b.legacy(i);
                listed.push(pool()[i].0);
            }
        }
        match rng.below(10) {
            0 => b = b.repo_note(rng.index(n), None),
            1 => b = b.repo_note(rng.index(n), Some("the repo's own note\n")),
            2 => {
                b = b
                    .put(dir("elsewhere"))
                    .put(link("w/.claude/memory", "@ROOT@/elsewhere"))
            }
            3 => b = b.put(file(rng.pick(&["w/.claude", "w/.claude/memory"]), "x")),
            4 => b = b.ledger(&[rng.index(n), rng.index(n)]),
            _ => {}
        }
    }
    if rng.chance(0.2) {
        let keys = *rng.pick(&[
            &[KEY][..],
            &[KEY, "a/b"][..],
            &["a/b"][..],
            &["c/d", "a/b"][..],
        ]);
        b = b.approvals(keys);
    } else if rng.chance(0.03) {
        b = b.put(file(
            "cfg/anthology/approved-repos.json",
            rng.pick(&["not json", "[]", "{}"]),
        ));
    }
    // the project's local file, unless the tree made `w/.claude` something else
    let claude_taken = b
        .tree
        .iter()
        .any(|e| matches!(opt_string(e, "p").as_deref(), Some("w" | "w/.claude")));
    if model_where == 0 && !claude_taken {
        b = b.put(file(
            "w/.claude/settings.local.json",
            "{\"env\": {\"CLAUDE_CODE_SUBAGENT_MODEL\": \"m\"}}",
        ));
    }
    b.guard_binary = match rng.below(6) {
        0 => "installed".into(),
        1 => format!("failed: {}", odd_text(rng, 30)),
        _ => "present".into(),
    };
    b.uname = match rng.below(40) {
        0 => "windows",
        1 => "darwin",
        2 => "riscv",
        _ => "linux",
    };
    // the steps
    for _ in 0..1 + rng.index(3) {
        if rng.chance(0.07) {
            let t = *rng.pick(&[
                "{",
                "{}",
                "{\"permissions\": []}",
                "",
                "{\"env\": {\"CLAUDE_CODE_SUBAGENT_MODEL\": \"mine\"}}",
            ]);
            b = b.write(&settings_path, t);
        }
        let c = gen_cmd(rng);
        b = b.step(c);
    }
    // the opt-in removal, then what remove would put back
    if model_set && rng.chance(0.4) {
        b = b
            .step(apply_cmd(&["--remove-subagent-model"]))
            .step(remove_cmd(&["--preview"]));
    }
    // a promotion of one listed legacy note, then what it left
    if !listed.is_empty() && rng.chance(0.3) {
        b = b
            .step(apply_cmd(&["--promote", rng.pick(&listed)]))
            .step(preview_cmd());
    }
    b.build()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every outcome of every command is reached by the generator (Rust side only). Apply's
    /// `skipped: guardrails not found` is not one: a plan without the guardrails is refused
    /// before the guard binary step.
    #[test]
    fn generated_cases_reach_every_outcome() {
        let wanted = [
            ("preview", "\"bypass_note\""),
            ("applied", "\"approved_key\""),
            ("stale", "changed since the preview"),
            ("during", "changed during setup"),
            ("installed", "\"status\": \"installed\""),
            ("up to date", "\"status\": \"up to date\""),
            ("warning", "\"status\": \"warning\""),
            ("failed", "\"status\": \"failed\""),
            ("no wiring", "skipped: rtk wiring not present"),
            ("no lock", "rtk wiring not present (no rtk.lock)"),
            ("no data", "skipped: plugin data folder unknown"),
            ("rtk error", "unsupported platform"),
            ("remove", "\"unapproved\""),
            ("remove preview", "\"unapprove_all\""),
            ("deleted", "\"status\": \"deleted\""),
            ("rtk symlink", "the rtk folder is a symlink"),
            ("rtk outside", "not under the plugin data folder"),
            (
                "history deleted",
                "\"rtk_history\": {\n    \"status\": \"deleted\"",
            ),
            ("history left", "is not a plain folder; left in place"),
            ("bin kept", "kept (anthology-guardrails still uses it)"),
            ("bin delete", "\"status\": \"delete\""),
            ("guard missing", "(hooks/guard.sh) was not found"),
            ("guard failed", "\"status\": \"failed: "),
            ("windows", "inactive on Windows"),
            ("usage", "usage: anthology setup"),
            ("unsafe", "repo memory is approved, but: "),
            ("promote", "--promote"),
            ("legacy listed", "\"movable\": true"),
            ("legacy blocked", "\"kind\": \"home-path\""),
            ("legacy conflict", "\"status\": \"conflict\""),
            ("legacy identical", "\"status\": \"identical\""),
            ("promoted", "\"repo\": [\n"),
            ("user note blocked", "type: user"),
            ("unapproved", "\"unapproved\": [\n"),
            ("approvals listed", "\"unapprove_all\": [\n"),
            ("not a repo", "not a git repo"),
            ("bad plugin root", "the plugin root"),
            ("revert", "Fix the file and run remove again"),
            ("raised", "Error: "),
            ("bad json", "Expecting"),
            ("model offered", "\"removable\": true"),
            ("model elsewhere", "\"how\": \""),
            ("model removed", "\"removed\": true"),
            (
                "model restored",
                r#"+    \"CLAUDE_CODE_SUBAGENT_MODEL\": \"<redacted>\""#,
            ),
        ];
        let mut seen = vec![0usize; wanted.len()];
        let mut rng = Rng::new(20261121);
        for _ in 0..1500 {
            let out = String::from_utf8(run(&gen_case(&mut rng)).stdout).unwrap();
            for (i, (_, needle)) in wanted.iter().enumerate() {
                if out.contains(needle) {
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
            let out = run(&case);
            assert!(
                (0..=2).contains(&out.exit),
                "{}",
                String::from_utf8_lossy(&out.stdout)
            );
            ran += 1;
        }
        assert!(ran > 60, "{ran}");
    }
}
