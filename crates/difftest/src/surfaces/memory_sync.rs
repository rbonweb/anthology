//! The `memory.sync` difftest surface (MS2; oracle `port/difftest/oracle/memory_sync.py`).
//!
//! A case is one scenario in a fresh temporary folder: notes in repo, agent, personal and
//! legacy memory, settings files, links, an approval, then one op (`run`, `migrate`, `sweep` or
//! `save`). Both sides print the op's result as sorted JSON and return the whole folder as a
//! tree, so the context, the messages and every file written are compared. The oracle's module
//! doc has the case format. As the tests' `fake_repo`, no git runs: Python replaces
//! `repo_root`, `repo_key`, `_main_root`, `settings_tracked` and `ensure_ignored`; Rust runs the
//! real code over a fake `Git` that answers the same.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use anthology_core::Env;
use anthology_core::clock::{self, Date};
use anthology_core::git::{Git, GitOut};
use anthology_memory::privacy::Finding;
use anthology_memory::save;
use anthology_memory::sync::{self, SyncDeps};
use anthology_pycompat::json::{self, Int, Map, Value};
use anthology_testkit::{Tree, TreeEntry};

use crate::case::{Case, Rng, Surface};
use crate::observe::{Observed, from_oracle};
use crate::oracle::Oracle;

/// The surface name (plan §1.4).
pub const NAME: &str = "memory.sync";

/// Whether `difftest --all` runs this surface on macOS too (`ci/jobs/difftest-macos.sh`); set
/// `false` only for a surface that cannot run there, and say why.
pub const ON_MACOS: bool = true;

/// The registered implementation.
pub fn surface() -> Option<Box<dyn Surface>> {
    Some(Box::new(MemorySync))
}

struct MemorySync;

impl Surface for MemorySync {
    fn name(&self) -> &'static str {
        NAME
    }

    fn seeds(&self) -> Vec<Case> {
        seeds()
    }

    fn generate(&self, rng: &mut Rng, n: usize) -> Vec<Case> {
        (0..n)
            .map(|i| Case::new(format!("gen:{}", i + 1), gen_case(rng)))
            .collect()
    }

    fn run_python(&self, c: &Case, oracle: &mut Oracle) -> Observed {
        from_oracle(&oracle.call("memory_sync", &c.data))
    }

    fn run_rust(&self, c: &Case) -> Observed {
        run(&c.data)
    }
}

// ---------------------------------------------------------------------------------------------
// The Rust side

/// The fake work tree's git (see the module docs).
struct FakeGit {
    root: PathBuf,
    in_repo: bool,
    key: String,
    tracked: bool,
}

impl Git for FakeGit {
    fn run(&self, _env: &Env, _cwd: &Path, args: &[&str]) -> Option<GitOut> {
        let out = |code: i32, text: String| GitOut {
            code,
            stdout: text.into_bytes(),
            stderr: Vec::new(),
        };
        let root = self.root.to_string_lossy();
        Some(match args {
            ["rev-parse", "--show-toplevel"] if self.in_repo => out(0, format!("{root}\n")),
            ["rev-parse", "--show-toplevel"] => out(128, String::new()),
            ["rev-parse", "--path-format=absolute", "--git-common-dir"] => {
                out(0, format!("{root}/.git\n"))
            }
            ["remote"] => out(0, "origin\n".into()),
            // `repo_key` normalises `https://<key>` back to `<key>` (lowercased, as the keys
            // the generator uses already are).
            ["remote", "get-url", "origin"] => out(0, format!("https://{}\n", self.key)),
            ["ls-files", ..] => out(if self.tracked { 0 } else { 1 }, String::new()),
            ["check-ignore", ..] => out(0, String::new()),
            _ => out(128, String::new()),
        })
    }
}

fn obj(v: &Value) -> &Map {
    v.as_object().expect("memory.sync: a case object")
}

fn get<'a>(case: &'a Value, k: &str) -> Option<&'a Value> {
    obj(case).get(k)
}

fn flag(case: &Value, k: &str, default: bool) -> bool {
    get(case, k).and_then(Value::as_bool).unwrap_or(default)
}

fn text<'a>(case: &'a Value, k: &str) -> &'a str {
    get(case, k).and_then(Value::as_str).unwrap_or("")
}

fn encode(p: &Path) -> String {
    anthology_pycompat::path::realpath(p)
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn parse_date(s: &str) -> Date {
    let p: Vec<&str> = s.split('-').collect();
    Date {
        y: p[0].parse().expect("year"),
        m: p[1].parse().expect("month"),
        d: p[2].parse().expect("day"),
    }
}

fn s_val(s: &str) -> Value {
    Value::Str(s.to_string())
}

fn str_list(v: &[String]) -> Value {
    Value::Array(v.iter().map(|s| s_val(s)).collect())
}

fn findings(f: &[Finding]) -> Value {
    Value::Array(
        f.iter()
            .map(|x| {
                let mut m = Map::new();
                m.insert("kind".into(), s_val(&x.kind));
                m.insert("line".into(), Value::Int(Int::Small(x.line as i64)));
                Value::Object(m)
            })
            .collect(),
    )
}

/// Sets up the scenario (as the oracle's `_setup`); returns `(home, config, root, legacy)`.
fn setup(case: &Value, tmp: &Path) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    let home = tmp.join("home");
    let config = home.join(".claude");
    let root = tmp.join("repo");
    fs::create_dir_all(&home).expect("home");
    fs::create_dir_all(&root).expect("repo");
    for d in get(case, "dirs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        fs::create_dir_all(tmp.join(d.as_str().expect("dir"))).expect("dirs");
    }
    if let Some(files) = get(case, "files").and_then(Value::as_object) {
        for (rel, t) in files.iter() {
            let p = tmp.join(rel);
            fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
            fs::write(&p, t.as_str().expect("text")).expect("file");
        }
    }
    let legacy = config.join("projects").join(encode(&root)).join("memory");
    if let Some(files) = get(case, "legacy").and_then(Value::as_object) {
        for (name, t) in files.iter() {
            fs::create_dir_all(&legacy).expect("legacy");
            fs::write(legacy.join(name), t.as_str().expect("text")).expect("legacy file");
        }
    }
    if let Some(links) = get(case, "links").and_then(Value::as_object) {
        for (rel, target) in links.iter() {
            let p = tmp.join(rel);
            if fs::symlink_metadata(&p).is_ok() {
                continue; // a file or folder of the case is already there
            }
            fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
            symlink(tmp.join(target.as_str().expect("target")), &p).expect("symlink");
        }
    }
    if flag(case, "approved", false) {
        let anth = config.join("anthology");
        fs::create_dir_all(&anth).expect("anth");
        let mut entry = Map::new();
        entry.insert("approved".into(), s_val("2026-01-01"));
        let mut repos = Map::new();
        repos.insert(text(case, "key").to_string(), Value::Object(entry));
        let mut data = Map::new();
        data.insert("version".into(), Value::Int(Int::Small(1)));
        data.insert("repos".into(), Value::Object(repos));
        let t = json::dumps(&Value::Object(data), &json::INDENT2_SORTED) + "\n";
        fs::write(anth.join("approved-repos.json"), t).expect("approved");
    }
    (home, config, root, legacy)
}

fn op(case: &Value, tmp: &Path, env: &Env, git: &FakeGit) -> Result<Value, String> {
    let (home, config, root, legacy) = (
        tmp.join("home"),
        tmp.join("home/.claude"),
        tmp.join("repo"),
        tmp.join("home/.claude/projects")
            .join(encode(&tmp.join("repo")))
            .join("memory"),
    );
    let personal = config.join("anthology").join("personal-memory");
    let deps = SyncDeps {
        git,
        ..SyncDeps::default()
    };
    let today = || parse_date(text(case, "today"));
    let mut m = Map::new();
    match text(case, "op") {
        "run" => {
            let cwd = if text(case, "cwd") == "plain" {
                tmp.join("plain")
            } else {
                root
            };
            let (ctx, msgs) = sync::run(&cwd, env, &deps);
            m.insert("context".into(), s_val(&ctx));
            m.insert("messages".into(), str_list(&msgs));
        }
        "migrate" => {
            let repo_mem = root.join(".claude").join("memory");
            let ledger = config.join("anthology").join("migrated.json");
            let promote: Vec<String> = get(case, "promote")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
            let r = sync::try_migrate(
                &legacy,
                flag(case, "repo_mem", false).then_some(repo_mem.as_path()),
                &personal,
                flag(case, "ledger", false).then_some(ledger.as_path()),
                Some(today()),
                &promote,
                Some(&home.to_string_lossy()),
                &deps,
            )
            .map_err(|e| format!("{}: {}", e.type_name(), e.py_str()))?;
            m.insert("copied".into(), str_list(&r.copied));
            m.insert("skipped".into(), str_list(&r.skipped));
            m.insert("conflicts".into(), str_list(&r.side_by_side));
            m.insert("promoted".into(), str_list(&r.promoted));
            let blocked = r
                .blocked
                .iter()
                .zip(&r.blocked_findings)
                .map(|((file, reason), f)| {
                    let mut b = Map::new();
                    b.insert("file".into(), s_val(file));
                    b.insert("findings".into(), findings(f));
                    b.insert("reason".into(), s_val(reason));
                    Value::Object(b)
                })
                .collect();
            m.insert("blocked".into(), Value::Array(blocked));
        }
        "sweep" => {
            let src = tmp.join(text(case, "src"));
            let r = sync::try_sweep_user_notes(&src, &personal, Some(today()), &deps)
                .map_err(|e| format!("{}: {}", e.type_name(), e.py_str()))?;
            m.insert("moved".into(), str_list(&r.moved));
            m.insert("conflicts".into(), str_list(&r.conflicts));
        }
        "save" => {
            let s = get(case, "save").expect("save");
            let f = |k: &str| text(s, k).to_string();
            match save::save_note(
                &root,
                &f("agent"),
                &f("type"),
                &f("slug"),
                &f("name"),
                &f("description"),
                &f("body"),
                flag(s, "replace", false),
                Some(today()),
                env,
            ) {
                Err(e) => {
                    m.insert("error".into(), s_val(&e.0));
                }
                Ok(out) => {
                    m.insert("action".into(), s_val(&out.action));
                    m.insert("relative".into(), s_val(&out.relative));
                    m.insert("root".into(), s_val(&out.root.to_string_lossy()));
                    m.insert(
                        "index_warning".into(),
                        out.index_warning.as_deref().map_or(Value::Null, s_val),
                    );
                }
            }
        }
        other => panic!("memory.sync: unknown op {other:?}"),
    }
    Ok(Value::Object(m))
}

/// `<tmp>` for the temporary folder (also in its encoded per-path form) and `<today>`.
fn norm(t: &str, tmp: &str, today: &str) -> String {
    t.replace(tmp, "<tmp>")
        .replace(&encode(Path::new(tmp)), "<tmp-encoded>")
        .replace(today, "<today>")
}

/// The tree of `tmp`, with `<tmp>` and `<today>` written as the oracle writes them.
fn tree(tmp: &Path, today: &str) -> Tree {
    let t = tmp.to_string_lossy().into_owned();
    let snap = anthology_testkit::snapshot(tmp);
    let mut out = Tree::default();
    for (k, e) in snap.0 {
        let e = match e {
            TreeEntry::File { bytes, mode } => match String::from_utf8(bytes) {
                Ok(s) => TreeEntry::File {
                    bytes: norm(&s, &t, today).into_bytes(),
                    mode,
                },
                Err(e) => TreeEntry::File {
                    bytes: e.into_bytes(),
                    mode,
                },
            },
            TreeEntry::Symlink { target } => TreeEntry::Symlink {
                target: norm(&target, &t, today),
            },
            other => other,
        };
        out.0.insert(norm(&k, &t, today), e);
    }
    out
}

fn run(case: &Value) -> Observed {
    let dir = tempfile::tempdir().expect("tempdir");
    let tmp = anthology_pycompat::path::realpath(dir.path());
    let today = clock::today_local().iso();
    let (home, config, root, _legacy) = setup(case, &tmp);
    fs::create_dir_all(tmp.join("plain")).expect("plain");
    let git = FakeGit {
        root,
        in_repo: flag(case, "in_repo", true),
        key: text(case, "key").to_string(),
        tracked: flag(case, "tracked", false),
    };
    let mut env = Env::default();
    if let Some(vars) = get(case, "env").and_then(Value::as_object) {
        for (k, v) in vars.iter() {
            env = env.with(k, v.as_str().expect("env value"));
        }
    }
    let env = env
        .with_os("HOME", &home)
        .with_os("CLAUDE_CONFIG_DIR", &config);
    let t = tmp.to_string_lossy().into_owned();
    let result = op(case, &tmp, &env, &git);
    let files = Some(tree(&tmp, &today));
    match result {
        Ok(v) => {
            let out = norm(&json::dumps(&v, &json::PY_DEFAULT_SORTED), &t, &today);
            Observed {
                exit: 0,
                stdout: out.into_bytes(),
                stderr: Vec::new(),
                files,
            }
        }
        Err(e) => Observed {
            exit: 1,
            stdout: Vec::new(),
            stderr: norm(&e, &t, &today).into_bytes(),
            files,
        },
    }
}

// ---------------------------------------------------------------------------------------------
// Cases

const KEY: &str = "example.invalid/acme/app";
const PERSONAL: &str = "home/.claude/anthology/personal-memory";
const REPO_MEM: &str = "repo/.claude/memory";
const AGENT: &str = "repo/.claude/agent-memory/anthology-scout";
const TODAY: &str = "2026-04-05";

fn note(name: &str, kind: &str, desc: &str, updated: Option<&str>, body: &str) -> String {
    let mut lines = vec![
        "---".to_string(),
        format!("name: {name}"),
        format!("description: {desc}"),
        "metadata:".into(),
        format!("  type: {kind}"),
    ];
    if let Some(u) = updated {
        lines.push(format!("  modified: {u}"));
    }
    lines.extend(["---".to_string(), String::new(), body.to_string()]);
    lines.join("\n")
}

/// A case builder over a JSON object.
struct B(Map);

impl B {
    fn new(op: &str) -> B {
        let mut m = Map::new();
        m.insert("op".into(), s_val(op));
        m.insert("key".into(), s_val(KEY));
        m.insert("today".into(), s_val(TODAY));
        m.insert("approved".into(), Value::Bool(true));
        B(m)
    }
    fn set(mut self, k: &str, v: Value) -> B {
        self.0.insert(k.into(), v);
        self
    }
    fn map_put(mut self, field: &str, k: &str, v: &str) -> B {
        let mut m = self
            .0
            .remove(field)
            .and_then(Value::into_object)
            .unwrap_or_default();
        m.insert(k.into(), s_val(v));
        self.0.insert(field.into(), Value::Object(m));
        self
    }
    fn file(self, rel: &str, t: &str) -> B {
        self.map_put("files", rel, t)
    }
    fn legacy(self, name: &str, t: &str) -> B {
        self.map_put("legacy", name, t)
    }
    fn link(self, rel: &str, target: &str) -> B {
        self.map_put("links", rel, target)
    }
    fn env(self, k: &str, v: &str) -> B {
        self.map_put("env", k, v)
    }
    fn dir(mut self, rel: &str) -> B {
        let mut a = self
            .0
            .remove("dirs")
            .and_then(Value::into_array)
            .unwrap_or_default();
        a.push(s_val(rel));
        self.0.insert("dirs".into(), Value::Array(a));
        self
    }
    fn case(self, id: &str) -> Case {
        Case::new(id, Value::Object(self.0))
    }
}

fn host_body() -> String {
    ["Runs on build-", "box.lan\n"].concat()
}

fn save_args(
    agent: &str,
    kind: &str,
    slug: &str,
    name: &str,
    desc: &str,
    body: &str,
    replace: bool,
) -> Value {
    let mut m = Map::new();
    for (k, v) in [
        ("agent", agent),
        ("type", kind),
        ("slug", slug),
        ("name", name),
        ("description", desc),
        ("body", body),
    ] {
        m.insert(k.into(), s_val(v));
    }
    m.insert("replace".into(), Value::Bool(replace));
    Value::Object(m)
}

fn seeds() -> Vec<Case> {
    let user = note("user-role", "user", "Senior engineer", None, "Body.\n");
    let proj = note(
        "project-a",
        "project",
        "Repo thing",
        Some("2026-02-02"),
        "Body.\n",
    );
    let fb = note("feedback-b", "feedback", "Agent thing", None, "Body.\n");
    let mut out = vec![
        B::new("run")
            .set("approved", Value::Bool(false))
            .case("seed:unapproved-empty"),
        B::new("run")
            .set("approved", Value::Bool(false))
            .file(&format!("{REPO_MEM}/a.md"), &proj)
            .case("seed:unapproved-hint"),
        B::new("run")
            .set("in_repo", Value::Bool(false))
            .set("cwd", s_val("plain"))
            .file(&format!("{PERSONAL}/user-me.md"), &user)
            .case("seed:outside-a-repo"),
        B::new("run").case("seed:approved-empty"),
        B::new("run")
            .file(&format!("{REPO_MEM}/user-role.md"), &user)
            .file(&format!("{REPO_MEM}/project-a.md"), &proj)
            .file(
                &format!("{AGENT}/user-tz.md"),
                &note("user-tz", "User # me", "CET", None, "x\n"),
            )
            .file(&format!("{AGENT}/feedback-b.md"), &fb)
            .case("seed:sweep-at-session-start"),
        B::new("run")
            .file(&format!("{PERSONAL}/user-role.md"), "my own\n")
            .file(&format!("{REPO_MEM}/user-role.md"), &user)
            .case("seed:sweep-conflict"),
        B::new("run")
            .legacy(
                "reference-host.md",
                &note("reference-host", "reference", "Host", None, &host_body()),
            )
            .legacy(
                "project-plan.md",
                &note("project-plan", "project", "Plan", None, "x\n"),
            )
            .legacy("MEMORY.md", "- old index\n")
            .case("seed:legacy-to-personal"),
        B::new("run").tracked_case(),
        B::new("run")
            .file("repo/.claude/settings.local.json", "{ not json")
            .case("seed:invalid-settings-local"),
        B::new("run")
            .file(
                "repo/.claude/settings.local.json",
                "{\"zeta\": 1, \"autoMemoryDirectory\": \"/old\"}",
            )
            .case("seed:stale-settings-local"),
        B::new("run")
            .dir("outside")
            .link("repo/.claude/memory", "outside")
            .case("seed:symlinked-repo-memory"),
        B::new("run")
            .dir("outside")
            .link("repo/.claude", "outside")
            .case("seed:symlinked-dot-claude"),
        B::new("run")
            .dir("elsewhere")
            .link(PERSONAL, "elsewhere")
            .file(&format!("{REPO_MEM}/user-role.md"), &user)
            .legacy("p.md", &proj)
            .case("seed:symlinked-personal"),
        B::new("run")
            .file(
                "home/.claude/settings.json",
                "{\"autoMemoryEnabled\": false}",
            )
            .file(&format!("{PERSONAL}/u.md"), &user)
            .case("seed:auto-memory-off-user-setting"),
        B::new("run")
            .env("CLAUDE_CODE_DISABLE_AUTO_MEMORY", "yes")
            .case("seed:auto-memory-off-env"),
        B::new("run")
            .env("CLAUDE_CODE_DISABLE_AUTO_MEMORY", " FALSE ")
            .file(&format!("{PERSONAL}/u.md"), &user)
            .case("seed:auto-memory-env-false"),
        B::new("run")
            .file("home/.claude/anthology/migrated.json", "{\"done\": 5}")
            .legacy("p.md", &proj)
            .case("seed:ledger-done-int"),
        B::new("run")
            .file(
                "home/.claude/anthology/migrated.json",
                "{\"done\": \"abc\"}",
            )
            .legacy("p.md", &proj)
            .case("seed:ledger-done-string"),
        B::new("run")
            .file(&format!("{PERSONAL}/MEMORY.md"), "hand-made\nindex\n")
            .case("seed:hand-made-personal-index"),
        B::new("run")
            .set("in_repo", Value::Bool(false))
            .set("cwd", s_val("plain"))
            .dir(PERSONAL)
            .file("outside.txt", "outside the memory folder\n")
            .link(&format!("{PERSONAL}/MEMORY.md"), "outside.txt")
            .case("seed:symlinked-hand-made-index"),
        B::new("run")
            .file(
                &format!("{REPO_MEM}/MEMORY.md"),
                "## user\n- [user-role](user-role.md)\n",
            )
            .file(&format!("{REPO_MEM}/user-role.md"), &user)
            .case("seed:index-left-with-no-notes"),
        B::new("migrate")
            .legacy("user-me.md", &user)
            .legacy("project-b.md", &proj)
            .legacy(
                "reference-host.md",
                &note("reference-host", "reference", "Host", None, &host_body()),
            )
            .set(
                "promote",
                Value::Array(vec![
                    s_val("project-b.md"),
                    s_val("reference-host.md"),
                    s_val("user-me.md"),
                ]),
            )
            .set("repo_mem", Value::Bool(true))
            .set("ledger", Value::Bool(true))
            .case("seed:migrate-promote"),
        B::new("migrate")
            .legacy("project-b.md", &proj)
            .file(&format!("{PERSONAL}/project-b.md"), "mine\n")
            .file(
                &format!("{PERSONAL}/project-b.migrated-{TODAY}.md"),
                "taken\n",
            )
            .case("seed:migrate-conflict-names"),
        B::new("sweep")
            .set("src", s_val(REPO_MEM))
            .file(&format!("{REPO_MEM}/user-role.md"), &user)
            .file(&format!("{REPO_MEM}/project-a.md"), &proj)
            .case("seed:sweep"),
        B::new("save")
            .set(
                "save",
                save_args(
                    "scout",
                    "project",
                    "eval-probe",
                    "Eval probe",
                    "How it works",
                    "Body text.",
                    false,
                ),
            )
            .case("seed:save"),
        B::new("save")
            .set(
                "save",
                save_args(
                    "scout",
                    "project",
                    "project-x",
                    "N",
                    "D",
                    "\n\n  lead\r\nsecond\rthird   \n",
                    false,
                ),
            )
            .case("seed:save-prefix-and-body"),
        B::new("save")
            .set(
                "save",
                save_args("nobody", "project", "x", "N", "D", "B", false),
            )
            .case("seed:save-unknown-agent"),
        B::new("save")
            .set(
                "save",
                save_args("scout", "user", "x", "N", "D", "B", false),
            )
            .case("seed:save-bad-type"),
        B::new("save")
            .set(
                "save",
                save_args("scout", "project", "x", "N", "D", &host_body(), false),
            )
            .case("seed:save-private"),
        B::new("save")
            .env("MY_API_TOKEN", "value-of-the-thing")
            .set(
                "save",
                save_args(
                    "scout",
                    "project",
                    "x",
                    "N",
                    "D",
                    "has value-of-the-thing",
                    false,
                ),
            )
            .case("seed:save-env-secret"),
        B::new("save")
            .file(&format!("{AGENT}/project-x.md"), "old\n")
            .set(
                "save",
                save_args("scout", "project", "x", "N", "D", "B", false),
            )
            .case("seed:save-exists"),
        B::new("save")
            .file(&format!("{AGENT}/project-x.md"), "old\n")
            .set(
                "save",
                save_args("scout", "project", "x", "N", "D", "B", true),
            )
            .case("seed:save-replace"),
        B::new("save")
            .dir("outside")
            .link("repo/.claude/agent-memory", "outside")
            .set(
                "save",
                save_args("scout", "project", "x", "N", "D", "B", true),
            )
            .case("seed:save-symlinked-component"),
    ];
    out.push(
        B::new("save")
            .set(
                "save",
                save_args("scout", "project", "x", "a\u{202e}b", "D", "B", false),
            )
            .case("seed:save-hidden-char"),
    );
    // The secret refusal names the kind and the line, never the text.
    let url = [
        "connect to postgres://app:",
        "hunter2-long-value@db.example.com/x",
    ]
    .concat();
    let aws = ["AKIA", "IOSFODNN7EXAMPLE"].concat();
    for (case, name, desc, body) in [
        (
            "seed:save-secret-url-line",
            "N".to_string(),
            "D".to_string(),
            format!("first\nsecond\n{url}\nlast"),
        ),
        (
            "seed:save-secret-lines-and-kinds",
            "N".to_string(),
            "D".to_string(),
            format!("{aws}\nok\n{aws}\n{url}"),
        ),
        (
            "seed:save-secret-name",
            format!("see {aws}"),
            "D".to_string(),
            "B".to_string(),
        ),
        (
            "seed:save-secret-more-than-five",
            "N".to_string(),
            "D".to_string(),
            [aws.as_str(); 8].join("\n"),
        ),
        (
            "seed:save-secret-across-fields",
            "the token =".to_string(),
            "D".to_string(),
            "abcdefgh12345".to_string(),
        ),
        (
            "seed:save-secret-description-and-body",
            "N".to_string(),
            format!("key {aws}"),
            ["\n\nx\npassword", ": hunter2-long-value\n"].concat(),
        ),
    ] {
        out.push(
            B::new("save")
                .set(
                    "save",
                    save_args("scout", "project", "x", &name, &desc, &body, false),
                )
                .case(case),
        );
    }
    out
}

impl B {
    fn tracked_case(self) -> Case {
        self.set("tracked", Value::Bool(true))
            .file("repo/.claude/settings.local.json", "{}\n")
            .case("seed:tracked-settings-local")
    }
}

const TYPES: &[&str] = &[
    "user",
    "User",
    "user # me",
    "USER",
    "project",
    "feedback",
    "reference",
    "zeta",
    "",
];
const PLACES: &[&str] = &[
    REPO_MEM,
    AGENT,
    "repo/.claude/agent-memory/anthology-writer",
    PERSONAL,
];
const NAMES: &[&str] = &[
    "a",
    "b",
    "user-x",
    "project-y",
    "weird[1]",
    "é",
    "n-1",
    "MEMORY",
];

fn gen_note(rng: &mut Rng) -> String {
    let kind = *rng.pick(TYPES);
    let name = if rng.chance(0.1) {
        rng.string(8)
    } else {
        (*rng.pick(NAMES)).to_string()
    };
    let desc = if rng.chance(0.15) {
        rng.string(30)
    } else {
        format!("desc {}", rng.below(100))
    };
    let updated = rng
        .chance(0.6)
        .then(|| format!("2026-{:02}-{:02}", 1 + rng.below(12), 1 + rng.below(28)));
    let body = match rng.below(8) {
        0 => host_body(),
        1 => rng.string(40),
        _ => "Body.\n".to_string(),
    };
    if rng.chance(0.05) {
        return rng.string(60); // no frontmatter at all
    }
    note(&name, kind, &desc, updated.as_deref(), &body)
}

fn gen_settings(rng: &mut Rng) -> &'static str {
    rng.pick(&[
        "{}",
        "{ not json",
        "[1, 2]",
        "",
        "{\"autoMemoryEnabled\": false}",
        "{\"autoMemoryEnabled\": true}",
        "{\"autoMemoryEnabled\": 0}",
        "{\"a\": 1, \"autoMemoryDirectory\": \"/stale\"}",
        "\u{feff}{}",
    ])
}

fn gen_case(rng: &mut Rng) -> Value {
    let op = match rng.below(20) {
        0..=11 => "run",
        12..=14 => "migrate",
        15..=16 => "sweep",
        _ => "save",
    };
    let mut b = B::new(op).set("approved", Value::Bool(rng.chance(0.75)));
    if rng.chance(0.1) {
        b = b.set("in_repo", Value::Bool(false));
    }
    if rng.chance(0.15) {
        b = b.set("cwd", s_val("plain"));
    }
    if rng.chance(0.08) {
        b = b.set("tracked", Value::Bool(true));
    }
    for _ in 0..rng.index(7) {
        let place = *rng.pick(PLACES);
        let file = format!(
            "{}-{}.md",
            rng.pick(&["user", "project", "x"]),
            rng.below(6)
        );
        b = b.file(&format!("{place}/{file}"), &gen_note(rng));
    }
    for _ in 0..rng.index(4) {
        let file = format!(
            "{}-{}.md",
            rng.pick(&["user", "project", "x"]),
            rng.below(6)
        );
        b = b.legacy(&file, &gen_note(rng));
    }
    if rng.chance(0.2) {
        b = b.legacy("MEMORY.md", "- old\n");
    }
    if rng.chance(0.15) {
        let place = *rng.pick(PLACES);
        b = b.file(&format!("{place}/MEMORY.md"), &rng.string(30));
    }
    if rng.chance(0.25) {
        b = b.file("repo/.claude/settings.local.json", gen_settings(rng));
    }
    if rng.chance(0.15) {
        b = b.file("repo/.claude/settings.json", gen_settings(rng));
    }
    if rng.chance(0.15) {
        b = b.file("home/.claude/settings.json", gen_settings(rng));
    }
    if rng.chance(0.1) {
        let v = *rng.pick(&["", "0", "1", "false", "No", "yes", " off "]);
        b = b.env("CLAUDE_CODE_DISABLE_AUTO_MEMORY", v);
    }
    if rng.chance(0.15) {
        let ledger = *rng.pick(&[
            "{\"version\": 1, \"done\": []}",
            "{\"done\": 3}",
            "{\"done\": null}",
            "{\"done\": \"ab\"}",
            "{\"done\": {\"x\": 1}}",
            "[1]",
            "not json",
        ]);
        b = b.file("home/.claude/anthology/migrated.json", ledger);
    }
    if rng.chance(0.08) {
        let at = *rng.pick(&[
            REPO_MEM,
            "repo/.claude",
            PERSONAL,
            "repo/.claude/agent-memory",
        ]);
        b = b.dir("outside").link(at, "outside");
    }
    if rng.chance(0.05) {
        b = b
            .link(&format!("{REPO_MEM}/user-link.md"), "outside.md")
            .file("outside.md", &note("user-out", "user", "Out", None, "x\n"));
    }
    match op {
        "migrate" => {
            let promote: Vec<Value> = (0..rng.index(3))
                .map(|_| {
                    s_val(&format!(
                        "{}-{}.md",
                        rng.pick(&["user", "project", "x"]),
                        rng.below(6)
                    ))
                })
                .collect();
            b = b
                .set("promote", Value::Array(promote))
                .set("repo_mem", Value::Bool(rng.chance(0.7)))
                .set("ledger", Value::Bool(rng.chance(0.6)));
        }
        "sweep" => {
            b = b.set("src", s_val(rng.pick(PLACES)));
        }
        "save" => {
            let agent = *rng.pick(&["scout", "reviewer", "nobody", "Scout"]);
            let kind = *rng.pick(&["project", "feedback", "reference", "user", ""]);
            let slug = *rng.pick(&["x", "project-x", "a-b", "a--b", "A", "", "feedback", "n-1"]);
            let name = match rng.below(10) {
                0..=2 => rng.string(20),
                3 => "Name-token-value".to_string(),
                _ => "Name".to_string(),
            };
            let desc = if rng.chance(0.3) {
                rng.string(20)
            } else {
                "Desc".to_string()
            };
            let body = match rng.below(6) {
                0 => rng.string(60),
                1 => host_body(),
                2 => "\n\n  lead\r\nx\ry   \n".to_string(),
                3 => {
                    let secret = rng
                        .pick(&[
                            ["AKIA", "IOSFODNN7EXAMPLE"].concat(),
                            ["postgres://app:", "hunter2-long-value@db.example.com/x"].concat(),
                            ["password", ": hunter2-long-value"].concat(),
                        ])
                        .clone();
                    let mut lines = vec!["line".to_string(); rng.index(4)];
                    lines.push(secret);
                    lines.push("end".to_string());
                    lines.join("\n")
                }
                _ => "Body.".to_string(),
            };
            if rng.chance(0.3) {
                b = b.file(&format!("{AGENT}/project-x.md"), "old\n");
            }
            if rng.chance(0.1) {
                b = b.env("MY_API_TOKEN", "Name-token-value");
            }
            b = b.set(
                "save",
                save_args(agent, kind, slug, &name, &desc, &body, rng.chance(0.4)),
            );
        }
        _ => {}
    }
    Value::Object(b.0)
}
