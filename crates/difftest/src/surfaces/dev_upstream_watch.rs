//! The `dev.upstream-watch` difftest surface (DV3). The oracle is
//! `port/difftest/oracle/dev_upstream_watch.py`.
//!
//! A case is one of:
//! - `{"op": "main", "args": [...], "env": {k: v}, "files": {rel: text}, "refs": {repo: R},
//!   "http": {url: H}, "gh": null | G, "show": [rel]}`: both sides write `files` into a fresh
//!   folder, make it the working folder and run `main` (Python: `upstream_watch.main(args, env,
//!   net, gh)`; Rust: `upstream_watch::main_with` with the same fakes). `R` is `{"pairs":
//!   [[ref, sha], ...]}` (a dict, so a repeated ref keeps its first place and its last sha),
//!   `{"raise": "<Class>"}` or `{"watch": "<message>"}` (a `WatchError`). `H` is `{"text": ...}`,
//!   `{"hex": ...}`, `{"raise": ...}` or `{"watch": ...}`. A repo or URL the case does not name
//!   raises `KeyError`, as the Python tests' fakes do. `G` is `{"issues": [...], "raise":
//!   "<Class>" | null, "fail_title": text | null, ["create_returns": value]}`: the listing, a
//!   create that raises `OSError` when the title contains `fail_title`, and what create returns
//!   (default `{"number": 900 + calls}`). The result is the exit code; stdout, then
//!   `=== calls ===` and each GitHub call as one JSON line, then each `show` file
//!   (`=== <rel> ===` and its text, or `<absent>`); and stderr. The folder is written `<ROOT>`.
//! - a library op printing one JSON value, or exiting 1 with `WatchError: <message>`:
//!   `sanitise` {text, limit}, `ls_remote` {text}, `manifest` {hex}, `latest` {pairs, pin},
//!   `without_budget` {text}, `render` {change, extra}, `classify` {entry}.
//!
//! The expected result is Python's with F-17 applied where the Python builds its commands (the
//! oracle docs). Normalization, both sides: DV2's D-2 rule for `[Errno N] <text>` (P1's errno
//! tables are pending). The oracle marks D-1 where Python raised past `main` (a missing
//! `number`, an unwritable `--json` or `GITHUB_OUTPUT`): Rust prints the same line.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::Path;

use anthology_core::Env;
use anthology_dev::memory_diff::pystr;
use anthology_dev::upstream_watch::{self as uw, Change, Deps, Error, GitHub, Net, Refs};
use anthology_pycompat::json::{self, Int, Map, PY_DEFAULT, Value};

use super::dev_dv2::{field, normalize, obj, replace, s, strings, text};
use crate::case::{Case, Rng, Surface};
use crate::observe::{Observed, from_oracle};
use crate::oracle::Oracle;

/// The surface name (plan §1.4).
pub const NAME: &str = "dev.upstream-watch";

/// Whether `difftest --all` runs this surface on macOS too (`ci/jobs/difftest-macos.sh`); set
/// `false` only for a surface that cannot run there, and say why.
pub const ON_MACOS: bool = true;

/// The registered implementation (DV3).
pub fn surface() -> Option<Box<dyn crate::case::Surface>> {
    Some(Box::new(UpstreamWatch))
}

struct UpstreamWatch;

impl Surface for UpstreamWatch {
    fn name(&self) -> &'static str {
        NAME
    }

    fn seeds(&self) -> Vec<Case> {
        seeds()
            .into_iter()
            .enumerate()
            .map(|(i, v)| Case::new(format!("seed:{}", i + 1), v))
            .collect()
    }

    fn generate(&self, rng: &mut Rng, n: usize) -> Vec<Case> {
        (0..n)
            .map(|i| Case::new(format!("gen:{}", i + 1), generate(rng)))
            .collect()
    }

    fn run_python(&self, c: &Case, oracle: &mut Oracle) -> Observed {
        let mut o = from_oracle(&oracle.call("dev_upstream_watch", &c.data));
        if normalize(&mut o) {
            oracle.mark("D-2");
        }
        o
    }

    fn run_rust(&self, c: &Case) -> Observed {
        let mut o = run_rust(&c.data);
        normalize(&mut o);
        o
    }
}

// ---- the Rust side ---------------------------------------------------------------------------

fn class_error(spec: &Map) -> Error {
    match spec.get("watch").and_then(Value::as_str) {
        Some(m) => Error::Watch(m.to_string()),
        None => Error::Other(
            spec.get("raise")
                .and_then(Value::as_str)
                .unwrap_or("Exception")
                .to_string(),
        ),
    }
}

/// `dict(pairs)`: a repeated key keeps its first place and takes the last value.
fn refs_of(pairs: &Value) -> Refs {
    let mut refs: Refs = Vec::new();
    for p in pairs.as_array().map_or(&[][..], Vec::as_slice) {
        let p = strings(p);
        let (k, v) = (p[0].clone(), p[1].clone());
        match refs.iter_mut().find(|(r, _)| *r == k) {
            Some(slot) => slot.1 = v,
            None => refs.push((k, v)),
        }
    }
    refs
}

struct FakeNet<'a> {
    refs: Option<&'a Map>,
    http: Option<&'a Map>,
}

impl Net for FakeNet<'_> {
    fn ls_remote(&self, repo: &str) -> Result<Refs, Error> {
        let spec = self
            .refs
            .and_then(|m| m.get(repo))
            .and_then(Value::as_object)
            .ok_or_else(|| Error::Other("KeyError".into()))?;
        match spec.get("pairs") {
            Some(pairs) => Ok(refs_of(pairs)),
            None => Err(class_error(spec)),
        }
    }

    fn http_get(&self, url: &str, _headers: &[(String, String)]) -> Result<Vec<u8>, Error> {
        let spec = self
            .http
            .and_then(|m| m.get(url))
            .and_then(Value::as_object)
            .ok_or_else(|| Error::Other("KeyError".into()))?;
        if let Some(t) = spec.get("text").and_then(Value::as_str) {
            return Ok(t.as_bytes().to_vec());
        }
        if let Some(h) = spec.get("hex").and_then(Value::as_str) {
            return Ok(unhex(h));
        }
        Err(class_error(spec))
    }
}

struct FakeGitHub<'a> {
    spec: &'a Map,
    calls: RefCell<Vec<Value>>,
}

impl GitHub for FakeGitHub<'_> {
    fn list_issues(&self) -> Result<Vec<Value>, Error> {
        self.calls.borrow_mut().push(Value::Array(vec![s("list")]));
        if let Some(Value::Str(class)) = self.spec.get("raise") {
            return Err(Error::Other(class.clone()));
        }
        Ok(self
            .spec
            .get("issues")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    fn create_issue(&self, title: &str, body: &str) -> Result<Value, Error> {
        if let Some(Value::Str(fail)) = self.spec.get("fail_title")
            && !fail.is_empty()
            && title.contains(fail.as_str())
        {
            return Err(Error::Other("OSError".into()));
        }
        self.calls
            .borrow_mut()
            .push(Value::Array(vec![s("create"), s(title), s(body)]));
        if let Some(v) = self.spec.get("create_returns") {
            return Ok(v.clone());
        }
        let n = 900 + self.calls.borrow().len() as i64;
        Ok(obj(vec![("number", Value::Int(Int::Small(n)))]))
    }

    fn update_issue(&self, number: &Value, title: &str, body: &str) -> Result<Value, Error> {
        self.calls.borrow_mut().push(Value::Array(vec![
            s("update"),
            number.clone(),
            s(title),
            s(body),
        ]));
        Ok(obj(vec![("number", number.clone())]))
    }
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).expect("hex"))
        .collect()
}

fn ok_json(v: &Value) -> Observed {
    Observed::text(0, &format!("{}\n", json::dumps(v, &PY_DEFAULT)), "")
}

fn strs(items: &[String]) -> Value {
    Value::Array(items.iter().map(|t| s(t)).collect())
}

fn run_rust(case: &Value) -> Observed {
    match text(case, "op") {
        "main" => run_main(case),
        "sanitise" => {
            let t = text(case, "text");
            let limit = field(case, "limit").as_i64().unwrap_or(80) as usize;
            ok_json(&strs(&[uw::sanitise_path(t), uw::sanitise_prose(t, limit)]))
        }
        "ls_remote" => ok_json(&Value::Array(
            uw::parse_ls_remote(text(case, "text"))
                .into_iter()
                .map(|(k, v)| strs(&[k, v]))
                .collect(),
        )),
        "manifest" => ok_json(&Value::Array(
            uw::parse_manifest(&unhex(text(case, "hex")))
                .into_iter()
                .map(|(k, v)| strs(&[k, v]))
                .collect(),
        )),
        "latest" => ok_json(&match uw::latest_stable_tag(
            &refs_of(field(case, "pairs")),
            text(case, "pin"),
        ) {
            Some((t, c)) => strs(&[t, c]),
            None => Value::Null,
        }),
        "without_budget" => {
            let t = text(case, "text").replace("\r\n", "\n");
            match uw::without_budget(pystr::strip(&t)) {
                Ok(v) => ok_json(&s(&v)),
                Err(e) => Observed::text(1, "", &format!("{}: {}\n", e.ty, e.msg)),
            }
        }
        "render" => {
            let c = field(case, "change");
            let pairs = |v: &Value| -> Vec<(String, String)> {
                v.as_array()
                    .map_or(&[][..], Vec::as_slice)
                    .iter()
                    .map(|d| {
                        let d = strings(d);
                        (d[0].clone(), d[1].clone())
                    })
                    .collect()
            };
            let change = Change {
                key: text(c, "key").into(),
                kind: text(c, "kind").into(),
                old: text(c, "old").into(),
                new: text(c, "new").into(),
                new_target: text(c, "new_target").into(),
                command: text(c, "command").into(),
                details: pairs(field(c, "details")),
                files: strings(field(c, "files")),
                files_total: field(c, "files_total").as_i64().unwrap_or(0) as usize,
                files_heading: text(c, "files_heading").into(),
                notes: strings(field(c, "notes")),
            };
            let extra = pairs(field(case, "extra"));
            ok_json(&Value::Array(vec![
                s(&uw::render_title(&change)),
                s(&uw::render_body(&change, &extra)),
                change.to_json(),
            ]))
        }
        "classify" => match uw::classify(field(case, "entry")) {
            Ok(k) => ok_json(&s(k.as_str())),
            Err(e) => Observed::text(1, "", &format!("WatchError: {}\n", e.shown())),
        },
        other => panic!("dev_upstream_watch: unknown op {other:?}"),
    }
}

fn run_main(case: &Value) -> Observed {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    if let Some(files) = field(case, "files").as_object() {
        for (rel, body) in files.iter() {
            let path = root.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("mkdir");
            }
            std::fs::write(&path, body.as_str().unwrap_or("")).expect("write");
        }
    }
    let mut env = Env::default();
    if let Some(m) = field(case, "env").as_object() {
        for (k, v) in m.iter() {
            env = env.with(k, v.as_str().unwrap_or(""));
        }
    }
    let net = FakeNet {
        refs: field(case, "refs").as_object(),
        http: field(case, "http").as_object(),
    };
    let gh = field(case, "gh").as_object().map(|spec| FakeGitHub {
        spec,
        calls: RefCell::new(Vec::new()),
    });
    let deps = Deps {
        net: Some(&net),
        gh: gh.as_ref().map(|g| g as &dyn GitHub),
        ..Deps::default()
    };
    let args: Vec<OsString> = strings(field(case, "args"))
        .into_iter()
        .map(Into::into)
        .collect();
    let out = uw::main_with(&args, &env, root, &deps);
    let mut stdout = out.stdout;
    stdout.extend_from_slice(b"=== calls ===\n");
    if let Some(g) = &gh {
        for call in g.calls.borrow().iter() {
            stdout.extend_from_slice(json::dumps(call, &PY_DEFAULT).as_bytes());
            stdout.push(b'\n');
        }
    }
    for rel in strings(field(case, "show")) {
        stdout.extend_from_slice(format!("=== {rel} ===\n").as_bytes());
        let path = root.join(&rel);
        match std::fs::read(&path) {
            Ok(b) if path.is_file() => {
                let t = String::from_utf8_lossy(&b)
                    .replace("\r\n", "\n")
                    .replace('\r', "\n");
                stdout.extend_from_slice(t.as_bytes());
            }
            _ => stdout.extend_from_slice(b"<absent>\n"),
        }
    }
    let shown = |b: &[u8]| -> Vec<u8> {
        let mut t = b.to_vec();
        for r in [
            std::fs::canonicalize(root).unwrap_or_default(),
            root.to_path_buf(),
        ] {
            let r = r.to_string_lossy().into_owned();
            if !r.is_empty() {
                t = replace(&t, r.as_bytes(), b"<ROOT>");
            }
        }
        t
    };
    Observed {
        exit: out.code,
        stdout: shown(&stdout),
        stderr: shown(&out.stderr),
        files: None,
    }
}

// ---- cases -----------------------------------------------------------------------------------

const SP_REPO: &str = "https://github.com/obra/superpowers";
const SKILLS_REPO: &str = "https://github.com/anthropics/skills";
const RTK_REPO: &str = "https://github.com/rtk-ai/rtk";
const OTHER_REPO: &str = "https://git.example.test/team/skills";
const NPM_URL: &str = "https://registry.npmjs.org/@anthropic-ai/claude-code/latest";
const UI_URL: &str = "https://uizze.sh/.well-known/skills/ui-taste";

const LS_TAGS: &str =
    include_str!("../../../anthology-dev/tests/fixtures/upstream_watch/ls_remote_tags.txt");
const LS_NO_PREFIX: &str =
    include_str!("../../../anthology-dev/tests/fixtures/upstream_watch/ls_remote_no_prefix.txt");
const COMPARE_PATH_CHANGED: &str =
    include_str!("../../../anthology-dev/tests/fixtures/upstream_watch/compare_path_changed.json");
const COMPARE_HOSTILE: &str =
    include_str!("../../../anthology-dev/tests/fixtures/upstream_watch/compare_hostile_paths.json");
const MANIFEST_OLD: &str =
    include_str!("../../../anthology-dev/tests/fixtures/upstream_watch/manifest_old.txt");
const MANIFEST_NEW: &str =
    include_str!("../../../anthology-dev/tests/fixtures/upstream_watch/manifest_new.txt");

fn sha(c: char) -> String {
    c.to_string().repeat(40)
}

fn sum_of(text: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn compare_url(repo: &str, old: &str, new: &str) -> String {
    let tail = repo
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .split("github.com/")
        .nth(1)
        .unwrap_or("x/y");
    format!("https://api.github.com/repos/{tail}/compare/{old}...{new}")
}

fn map(pairs: Vec<(String, Value)>) -> Value {
    let mut m = Map::new();
    for (k, v) in pairs {
        m.insert(k, v);
    }
    Value::Object(m)
}

fn loads(t: &str) -> Value {
    json::loads(t).expect("case json")
}

fn pairs_of_ls(text: &str) -> Value {
    Value::Array(
        uw::parse_ls_remote(text)
            .into_iter()
            .map(|(k, v)| strs(&[k, v]))
            .collect(),
    )
}

fn text_body(t: &str) -> Value {
    obj(vec![("text", s(t))])
}

fn pairs_body(p: Value) -> Value {
    obj(vec![("pairs", p)])
}

fn compare_body(files: &[String], total: Value) -> String {
    let items: Vec<Value> = files
        .iter()
        .map(|f| obj(vec![("filename", s(f)), ("status", s("modified"))]))
        .collect();
    json::dumps(
        &obj(vec![
            ("status", s("ahead")),
            ("total_commits", total),
            ("commits", Value::Array(Vec::new())),
            ("files", Value::Array(items)),
        ]),
        &PY_DEFAULT,
    )
}

fn package_json(version: &str) -> String {
    format!(
        "{{\"dependencies\": {{\"@anthropic-ai/claude-code\": {}}}}}",
        json::dumps(&s(version), &PY_DEFAULT)
    )
}

/// A `make_repo`-like agent tree: `reader` reads alpha and beta, `skilled` reads beta and
/// preloads `plan`, `scout` (a real budget) reads alpha.
fn agent_files(files: &mut Vec<(String, Value)>) {
    let agent = |name: &str, skills: &str, startup: &[&str]| {
        let lines: Vec<String> = startup
            .iter()
            .map(|k| format!("- `${{CLAUDE_PLUGIN_ROOT}}/knowledge/{k}/SKILL.md`"))
            .collect();
        format!(
            "---\nname: {name}\ndescription: \"Use when testing.\"\nmodel: haiku\n{skills}---\n\n\
             ## Startup\nBefore any other work, Read every file below, in order:\n{}\n\n## Role\nTest agent.\n",
            lines.join("\n")
        )
    };
    for (rel, body) in [
        (
            "plugins/anthology/knowledge/alpha/SKILL.md".to_string(),
            "a".repeat(100),
        ),
        (
            "plugins/anthology/knowledge/beta/SKILL.md".to_string(),
            "b".repeat(200),
        ),
        (
            "plugins/anthology-superpowers/skills/plan/SKILL.md".to_string(),
            "p".repeat(400),
        ),
        (
            "plugins/anthology/agents/reader.md".into(),
            agent("reader", "", &["alpha", "beta"]),
        ),
        (
            "plugins/anthology/agents/skilled.md".into(),
            agent(
                "skilled",
                "skills:\n  - anthology-superpowers:plan\n",
                &["beta"],
            ),
        ),
        (
            "plugins/anthology/agents/scout.md".into(),
            agent("scout", "", &["alpha"]),
        ),
    ] {
        files.push((rel, s(&body)));
    }
}

fn main_case(
    args: &[&str],
    env: &[(&str, &str)],
    files: Vec<(String, Value)>,
    refs: Vec<(String, Value)>,
    http: Vec<(String, Value)>,
    gh: Value,
) -> Value {
    obj(vec![
        ("op", s("main")),
        ("args", Value::Array(args.iter().map(|a| s(a)).collect())),
        (
            "env",
            map(env.iter().map(|(k, v)| ((*k).to_string(), s(v))).collect()),
        ),
        ("files", map(files)),
        ("refs", map(refs)),
        ("http", map(http)),
        ("gh", gh),
        ("show", Value::Array(vec![s("out.json"), s("gh_output")])),
    ])
}

fn sources_file(entries: Vec<Value>) -> (String, Value) {
    (
        "vendor/sources.json".into(),
        s(&json::dumps(
            &obj(vec![("sources", Value::Array(entries))]),
            &PY_DEFAULT,
        )),
    )
}

fn git_entry(name: &str, repo: &str, path: &str, pin: &str, commit: &str, target: &str) -> Value {
    obj(vec![
        ("name", s(name)),
        ("repo", s(repo)),
        ("path", s(path)),
        ("ref", s(pin)),
        ("commit", s(commit)),
        ("target", s(target)),
    ])
}

fn gh_spec(issues: Vec<Value>) -> Value {
    obj(vec![
        ("issues", Value::Array(issues)),
        ("raise", Value::Null),
        ("fail_title", Value::Null),
    ])
}

fn bot_issue(number: Value, state: &str, title: &str, body: &str) -> Value {
    obj(vec![
        ("number", number),
        ("state", s(state)),
        ("title", s(title)),
        ("body", s(body)),
        (
            "labels",
            Value::Array(vec![obj(vec![("name", s("upstream-watch"))])]),
        ),
        ("user", obj(vec![("login", s("github-actions[bot]"))])),
    ])
}

fn seeds() -> Vec<Value> {
    let mut out = Vec::new();
    // The MainTests world: a tag source, a url source, Claude Code; dry run and real.
    let world = |gh: Value, args: &[&str], env: &[(&str, &str)], with_agents: bool| {
        let mut files = vec![
            sources_file(vec![
                git_entry(
                    "superpowers",
                    SP_REPO,
                    "",
                    "v7.0.2",
                    &sha('5'),
                    "plugins/anthology-superpowers",
                ),
                obj(vec![
                    ("name", s("ui-taste")),
                    ("url", s(UI_URL)),
                    ("checksum", s(&sum_of(MANIFEST_OLD))),
                    ("target", s("plugins/anthology/knowledge/alpha")),
                ]),
                obj(vec![("name", s("mystery")), ("target", s("t"))]),
            ]),
            (
                "ci/claude-code/package.json".into(),
                s(&package_json("2.1.329")),
            ),
            (
                "vendor/snapshots/ui-taste/CHECKSUMS.sha256".into(),
                s(MANIFEST_OLD),
            ),
        ];
        if with_agents {
            agent_files(&mut files);
        }
        main_case(
            args,
            env,
            files,
            vec![(SP_REPO.into(), pairs_body(pairs_of_ls(LS_TAGS)))],
            vec![
                (
                    compare_url(SP_REPO, &sha('5'), &sha('a')),
                    text_body(COMPARE_PATH_CHANGED),
                ),
                (
                    format!("{UI_URL}/CHECKSUMS.sha256"),
                    text_body(MANIFEST_NEW),
                ),
                (NPM_URL.into(), text_body("{\"version\": \"2.1.400\"}")),
            ],
            gh,
        )
    };
    let real = [
        ("GITHUB_TOKEN", "t"),
        ("GITHUB_REPOSITORY", "o/r"),
        ("GITHUB_OUTPUT", "gh_output"),
    ];
    out.push(world(
        Value::Null,
        &["--dry-run", "--root", "."],
        &[],
        false,
    ));
    out.push(world(
        Value::Null,
        &["--dry-run", "--root", ".", "--json", "out.json"],
        &[],
        true,
    ));
    out.push(world(gh_spec(vec![]), &["--root", "."], &real, true));
    out.push(world(Value::Null, &["--root", "."], &[], false));
    out.push(world(
        gh_spec(vec![]),
        &["--root", ".", "--json", "out.json"],
        &real,
        true,
    ));
    let mut failing = gh_spec(vec![]);
    failing
        .as_object_mut()
        .expect("obj")
        .insert("raise".into(), s("OSError"));
    out.push(world(failing, &["--dry-run", "--root", "."], &real, true));
    let mut flaky = gh_spec(vec![]);
    flaky
        .as_object_mut()
        .expect("obj")
        .insert("fail_title".into(), s("superpowers"));
    out.push(world(flaky, &["--root", "."], &real, false));
    // Issue states: dismissed, unchanged-looking, stale, and a missing `number` (KeyError).
    let marker = "<!-- upstream-watch:superpowers -->\nold";
    out.push(world(
        gh_spec(vec![
            bot_issue(Value::Int(Int::Small(3)), "open", "x", marker),
            bot_issue(Value::Int(Int::Small(9)), "closed", "x", marker),
        ]),
        &["--root", "."],
        &real,
        true,
    ));
    let mut no_number = bot_issue(Value::Null, "open", "x", marker);
    no_number.as_object_mut().expect("obj").remove("number");
    out.push(world(
        gh_spec(vec![no_number]),
        &["--root", "."],
        &real,
        false,
    ));
    // An empty --json is no --json (`if args.json:`).
    out.push(world(
        Value::Null,
        &["--dry-run", "--root", ".", "--json", ""],
        &[],
        false,
    ));
    // Unwritable outputs: --json and GITHUB_OUTPUT naming a folder.
    out.push(world(
        Value::Null,
        &["--dry-run", "--root", ".", "--json", "vendor"],
        &[],
        false,
    ));
    out.push(world(
        gh_spec(vec![]),
        &["--root", "."],
        &[
            ("GITHUB_TOKEN", "t"),
            ("GITHUB_REPOSITORY", "o/r"),
            ("GITHUB_OUTPUT", "vendor"),
        ],
        false,
    ));
    // argparse.
    for args in [
        vec!["-h"],
        vec!["--bogus"],
        vec!["--json"],
        vec!["--dry", "--root", "."],
        vec!["--root=.", "--dry-run", "extra"],
    ] {
        out.push(world(Value::Null, &args, &[], false));
    }
    // A missing sources.json: the check error and the startup-budget warning.
    out.push(main_case(
        &["--dry-run", "--root", "."],
        &[],
        vec![(
            "ci/claude-code/package.json".into(),
            s(&package_json("2.1.329")),
        )],
        vec![],
        vec![(NPM_URL.into(), text_body("{\"version\": \"2.1.329\"}"))],
        Value::Null,
    ));
    // Branch pins against the hostile and path fixtures, and a tag pin without a v prefix.
    out.push(main_case(
        &["--dry-run", "--root", "."],
        &[],
        vec![sources_file(vec![
            git_entry("docs", SKILLS_REPO, "docs", "main", &sha('1'), "t"),
            git_entry(
                "frontend-design",
                SKILLS_REPO,
                "skills/frontend-design",
                "main",
                &sha('1'),
                "t",
            ),
            git_entry("np", SP_REPO, "", "1.2.0", &sha('5'), "t"),
        ])],
        vec![
            (
                SKILLS_REPO.into(),
                pairs_body(pairs_of_ls(&format!("{}\trefs/heads/main\n", sha('2')))),
            ),
            (SP_REPO.into(), pairs_body(pairs_of_ls(LS_NO_PREFIX))),
        ],
        vec![
            (
                compare_url(SKILLS_REPO, &sha('1'), &sha('2')),
                text_body(COMPARE_HOSTILE),
            ),
            (
                compare_url(SP_REPO, &sha('5'), &sha('2')),
                text_body(COMPARE_PATH_CHANGED),
            ),
        ],
        Value::Null,
    ));
    // Library ops.
    out.push(lib(
        "sanitise",
        vec![
            ("text", s("a`b\nc\u{0}d\u{202e}e")),
            ("limit", Value::Int(Int::Small(80))),
        ],
    ));
    out.push(lib("ls_remote", vec![("text", s(LS_TAGS))]));
    out.push(lib(
        "manifest",
        vec![("hex", s(&hex(MANIFEST_NEW.as_bytes())))],
    ));
    out.push(lib(
        "latest",
        vec![("pairs", pairs_of_ls(LS_TAGS)), ("pin", s("v7.0.2"))],
    ));
    out.push(lib(
        "without_budget",
        vec![(
            "text",
            s("a\r\n### Startup budget\r\nx\r\n### Update\r\nz \n"),
        )],
    ));
    out
}

fn lib(op: &str, pairs: Vec<(&str, Value)>) -> Value {
    let mut all = vec![("op", s(op))];
    all.extend(pairs);
    obj(all)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

// ---- generation ------------------------------------------------------------------------------

const NAMES: &[&str] = &[
    "alpha",
    "beta",
    "superpowers",
    "ui-taste",
    "rtk",
    "x.y",
    "..",
    "a b",
    "a`b",
    "",
    "dup",
];
const PATHS: &[&str] = &["", "skills", "skills/a", "docs", "/skills/a/", "a b", "x\n"];
const PINS: &[&str] = &[
    "v7.0.2", "v7.0", "1.2.0", "v1.2.1", "main", "develop", "v8.0.0", "a b", "",
];
const TARGETS: &[&str] = &[
    "plugins/anthology/knowledge/alpha",
    "plugins/anthology/knowledge/beta",
    "plugins/anthology-superpowers",
    "plugins/anthology/knowledge/none",
    "",
    "t",
];
const SHA_CHARS: &[char] = &['1', '2', '3', '5', '7', 'a', 'b'];
const TAGS: &[&str] = &[
    "v7.0.1",
    "v7.0.2",
    "v7.1.0",
    "v7.10.0",
    "v7.1",
    "v7.1.9-rc.1",
    "nightly",
    "1.2.0",
    "1.3",
    "v1.9.0",
    "v1.3.0",
    "v99.0.0`x`",
    "v007.1.0",
    "v8",
];
const FILE_NAMES: &[&str] = &[
    "skills/a/SKILL.md",
    "skills/ab/x.md",
    "docs/ok.md",
    "README.md",
    "a b.md",
    "docs/evil```\n@octocat \u{7}x.md",
    "\u{202e}rtl.md",
    "skills/a/\u{2028}y.md",
];

fn gen_sha(rng: &mut Rng) -> String {
    match rng.below(10) {
        0 => "zzz".into(),
        1 => format!("{}\n", sha('1')),
        _ => sha(*rng.pick(SHA_CHARS)),
    }
}

fn gen_files(rng: &mut Rng) -> Vec<String> {
    if rng.chance(0.05) {
        return (0..300).map(|i| format!("elsewhere/f{i}.md")).collect();
    }
    (0..rng.index(5))
        .map(|_| rng.pick(FILE_NAMES).to_string())
        .collect()
}

fn gen_compare(rng: &mut Rng) -> Value {
    match rng.below(12) {
        0 => obj(vec![("raise", s("URLError"))]),
        1 => obj(vec![("watch", s("response too large"))]),
        2 => text_body("[1, 2]"),
        3 => text_body("{\"files\": {}}"),
        4 => text_body("not json"),
        5 => obj(vec![("hex", s("ff00"))]),
        6 => text_body(COMPARE_PATH_CHANGED),
        _ => {
            let total = match rng.below(5) {
                0 => Value::Int(Int::Small(-1)),
                1 => Value::Bool(true),
                2 => s("3"),
                _ => Value::Int(Int::Small(rng.range(0, 40))),
            };
            text_body(&compare_body(&gen_files(rng), total))
        }
    }
}

fn gen_refs(rng: &mut Rng, pinned: &str) -> Value {
    match rng.below(10) {
        0 => obj(vec![("raise", s("OSError"))]),
        1 => obj(vec![("watch", s("git ls-remote failed (exit 128)"))]),
        _ => {
            let mut pairs = Vec::new();
            for _ in 0..rng.index(6) {
                let tag = rng.pick(TAGS);
                pairs.push(strs(&[format!("refs/tags/{tag}"), gen_sha(rng)]));
                if rng.chance(0.4) {
                    pairs.push(strs(&[format!("refs/tags/{tag}^{{}}"), gen_sha(rng)]));
                }
            }
            for branch in ["main", "develop"] {
                if rng.chance(0.7) {
                    let head = if rng.chance(0.3) {
                        pinned.to_string()
                    } else {
                        gen_sha(rng)
                    };
                    pairs.push(strs(&[format!("refs/heads/{branch}"), head]));
                }
            }
            rng.shuffle(&mut pairs);
            pairs_body(Value::Array(pairs))
        }
    }
}

fn gen_manifest(rng: &mut Rng) -> String {
    match rng.below(6) {
        0 => MANIFEST_OLD.to_string(),
        1 => MANIFEST_NEW.to_string(),
        2 => "garbage\n".to_string(),
        _ => {
            let mut lines = Vec::new();
            for _ in 0..rng.index(5) {
                let h = rng.pick(&['1', '3', '5', '6', 'A']).to_string().repeat(64);
                let sep = *rng.pick(&["  ", " *", "\t", " "]);
                let name = *rng.pick(&[
                    "SKILL.md",
                    "references/same.md",
                    "references/x y.md",
                    "a`b",
                    " pad ",
                ]);
                lines.push(format!("{h}{sep}{name}"));
            }
            lines.join(if rng.chance(0.2) { "\r\n" } else { "\n" })
        }
    }
}

/// One random world for `main`.
fn generate(rng: &mut Rng) -> Value {
    if rng.chance(0.08) {
        return gen_lib(rng);
    }
    let mut files: Vec<(String, Value)> = Vec::new();
    let mut refs: Vec<(String, Value)> = Vec::new();
    let mut http: Vec<(String, Value)> = Vec::new();
    let mut entries = Vec::new();
    for _ in 0..rng.index(5) {
        let name = rng.pick(NAMES).to_string();
        let target = rng.pick(TARGETS).to_string();
        match rng.below(6) {
            0 | 1 => {
                let repo = *rng.pick(&[
                    SKILLS_REPO,
                    SP_REPO,
                    OTHER_REPO,
                    "https://github.com/anthropics/skills.git",
                    "file:///x",
                ]);
                let pinned = gen_sha(rng);
                let pin = rng.pick(PINS).to_string();
                let mut e = git_entry(&name, repo, rng.pick(PATHS), &pin, &pinned, &target);
                if rng.chance(0.1) {
                    e.as_object_mut().expect("obj").remove("path");
                }
                if !refs.iter().any(|(r, _)| r == repo) && rng.chance(0.9) {
                    let r = gen_refs(rng, &pinned);
                    if let Some(Value::Array(pairs)) = r.as_object().and_then(|m| m.get("pairs")) {
                        for p in pairs {
                            let new = strings(p)[1].clone();
                            if rng.chance(0.8) {
                                http.push((compare_url(repo, &pinned, &new), gen_compare(rng)));
                            }
                        }
                    }
                    refs.push((repo.to_string(), r));
                }
                entries.push(e);
            }
            2 => {
                let url = *rng.pick(&[
                    UI_URL,
                    "https://uizze.sh/.well-known/skills/ui-taste/",
                    "https://example.test/m/CHECKSUMS.sha256",
                    "http://example.test/x",
                ]);
                let checksum = match rng.below(4) {
                    0 => "zz".to_string(),
                    1 => sum_of(MANIFEST_NEW),
                    _ => sum_of(MANIFEST_OLD),
                };
                entries.push(obj(vec![
                    ("name", s(&name)),
                    ("url", s(url)),
                    ("checksum", s(&checksum)),
                    ("target", s(&target)),
                ]));
                let manifest = if url.ends_with(".sha256") {
                    url.to_string()
                } else {
                    format!("{}/CHECKSUMS.sha256", url.trim_end_matches('/'))
                };
                let body = match rng.below(8) {
                    0 => obj(vec![("raise", s("TimeoutError"))]),
                    1 => obj(vec![("watch", s("response too large"))]),
                    _ => text_body(&gen_manifest(rng)),
                };
                http.push((manifest, body));
                if rng.chance(0.6) {
                    files.push((
                        format!("vendor/snapshots/{name}/CHECKSUMS.sha256"),
                        s(&gen_manifest(rng)),
                    ));
                }
            }
            3 => {
                let commit = gen_sha(rng);
                let pin = *rng.pick(&["v1.2.1", "v1.3.0", "main"]);
                let mut e = obj(vec![
                    ("name", s(&name)),
                    ("target", s(&target)),
                    (
                        "upstream",
                        obj(vec![
                            ("repo", s(RTK_REPO)),
                            ("ref", s(pin)),
                            ("commit", s(&commit)),
                        ]),
                    ),
                ]);
                if rng.chance(0.5) {
                    e.as_object_mut()
                        .expect("obj")
                        .insert("managed_by".into(), s("subtree"));
                }
                if !refs.iter().any(|(r, _)| r == RTK_REPO) {
                    let r = gen_refs(rng, &commit);
                    if let Some(Value::Array(pairs)) = r.as_object().and_then(|m| m.get("pairs")) {
                        for p in pairs {
                            let new = strings(p)[1].clone();
                            http.push((compare_url(RTK_REPO, &commit, &new), gen_compare(rng)));
                        }
                    }
                    refs.push((RTK_REPO.into(), r));
                }
                entries.push(e);
            }
            4 => entries.push(match rng.below(5) {
                0 => obj(vec![("name", s("mystery")), ("target", s(&target))]),
                1 => s("not an object"),
                2 => obj(vec![("name", s(&name)), ("managed_by", s("subtree"))]),
                3 => obj(vec![
                    ("name", Value::Int(Int::Small(3))),
                    ("repo", s(SKILLS_REPO)),
                ]),
                _ => obj(vec![
                    ("name", Value::Array(vec![])),
                    ("target", s("t")),
                    ("url", s(UI_URL)),
                ]),
            }),
            _ => entries.push(git_entry(
                &name,
                SP_REPO,
                "",
                rng.pick(&["v7.0.2", "v7.0"]),
                &sha('5'),
                &target,
            )),
        }
    }
    if !refs.iter().any(|(r, _)| r == SP_REPO) {
        refs.push((SP_REPO.into(), pairs_body(pairs_of_ls(LS_TAGS))));
        for c in ['7', 'a'] {
            http.push((compare_url(SP_REPO, &sha('5'), &sha(c)), gen_compare(rng)));
        }
    }
    match rng.below(12) {
        0 => {}
        1 => files.push(("vendor/sources.json".into(), s("[1, 2]"))),
        2 => files.push(("vendor/sources.json".into(), s("{\"sources\": 5}"))),
        3 => files.push(("vendor/sources.json".into(), s("{\"sources\": {\"a\": 1}}"))),
        _ => files.push(sources_file(entries)),
    }
    match rng.below(8) {
        0 => {}
        1 => files.push(("ci/claude-code/package.json".into(), s("[]"))),
        _ => files.push((
            "ci/claude-code/package.json".into(),
            s(&package_json(rng.pick(&[
                "2.1.329",
                "2.1.400",
                "2.1.329-beta.1",
                "x",
            ]))),
        )),
    }
    let npm = match rng.below(9) {
        0 => obj(vec![("raise", s("URLError"))]),
        1 => text_body("not json"),
        2 => text_body("[\"2.1.400\"]"),
        3 => text_body(&format!("{{\"version\": \"2.1.400-{}\"}}", "a".repeat(64))),
        _ => text_body(&format!(
            "{{\"version\": {}}}",
            json::dumps(
                &s(rng.pick(&[
                    "2.1.400",
                    "2.1.329",
                    "2.1.300",
                    "2.1.329-beta.1",
                    "2.1.400\n",
                    "2.2.0+b"
                ])),
                &PY_DEFAULT
            )
        )),
    };
    http.push((NPM_URL.into(), npm));
    if rng.chance(0.6) {
        agent_files(&mut files);
        if rng.chance(0.1) {
            files.push((
                "plugins/anthology/agents/broken.md".into(),
                s("no frontmatter"),
            ));
        }
    }

    let dry_run = rng.chance(0.5);
    let mut args: Vec<&str> = Vec::new();
    if dry_run {
        args.push(*rng.pick(&["--dry-run", "--dry"]));
    }
    args.extend(["--root", "."]);
    if rng.chance(0.4) {
        args.extend(["--json", *rng.pick(&["out.json", "out.json", "vendor"])]);
    }
    if rng.chance(0.03) {
        args.push(*rng.pick(&["--bogus", "-h", "x"]));
    }
    let mut env: Vec<(&str, &str)> = Vec::new();
    let with_client = !dry_run || rng.chance(0.6);
    if with_client {
        env.extend([("GITHUB_TOKEN", "t"), ("GITHUB_REPOSITORY", "o/r")]);
    } else if rng.chance(0.3) {
        env.push(("GITHUB_TOKEN", "t"));
    }
    if rng.chance(0.5) {
        env.push((
            "GITHUB_OUTPUT",
            *rng.pick(&["gh_output", "gh_output", "vendor"]),
        ));
    }
    if !dry_run && rng.chance(0.05) {
        env.clear();
    }
    let gh = if with_client && !env.is_empty() {
        gen_gh(rng, &files, &refs, &http, &args)
    } else {
        Value::Null
    };
    main_case(&args, &env, files, refs, http, gh)
}

/// Issues for the world: some carry the body Rust renders for the world's changes (so the
/// unchanged, dismissed and update paths are reached), some are near misses.
fn gen_gh(
    rng: &mut Rng,
    files: &[(String, Value)],
    refs: &[(String, Value)],
    http: &[(String, Value)],
    _args: &[&str],
) -> Value {
    let rendered = render_world(files, refs, http);
    let mut issues = Vec::new();
    let mut keys: Vec<String> = rendered.iter().map(|(k, _, _)| k.clone()).collect();
    keys.push("other".into());
    for key in keys {
        for _ in 0..rng.index(3) {
            let (title, body) = rendered
                .iter()
                .find(|(k, _, _)| *k == key)
                .map(|(_, t, b)| (t.clone(), b.clone()))
                .unwrap_or_else(|| ("t".into(), format!("<!-- upstream-watch:{key} -->\nx")));
            let body = match rng.below(8) {
                0 => body.replace('\n', "\r\n") + "\r\n  ",
                1 => format!("<!-- upstream-watch:{key} -->\nolder text"),
                2 => format!("see <!-- upstream-watch:{key} -->"),
                3 if body.contains("### Update") => body.replacen(
                    "\n### Update",
                    "\n### Startup budget\n| `x` | 1 | 2 | 50% |\n\n### Update",
                    1,
                ),
                _ => body,
            };
            let number = match rng.below(14) {
                0 => s(&rng.range(1, 30).to_string()),
                1 => Value::Float(rng.range(1, 30) as f64 + 0.5),
                2 => Value::Null,
                _ => Value::Int(Int::Small(rng.range(1, 30))),
            };
            let mut issue = bot_issue(
                number,
                rng.pick(&["open", "open", "closed"]),
                if rng.chance(0.8) {
                    &title
                } else {
                    "Upstream: other"
                },
                &body,
            );
            let m = issue.as_object_mut().expect("obj");
            match rng.below(16) {
                0 => {
                    m.remove("number");
                }
                1 => m.insert("user".into(), obj(vec![("login", s("mallory"))])),
                2 => m.insert("labels".into(), Value::Array(vec![s("upstream-watch")])),
                3 => m.insert(
                    "labels".into(),
                    Value::Array(vec![obj(vec![("name", s("bug"))])]),
                ),
                4 => m.insert("pull_request".into(), obj(vec![])),
                5 => m.insert("body".into(), Value::Null),
                6 => m.insert("body".into(), Value::Int(Int::Small(5))),
                _ => {}
            }
            issues.push(issue);
        }
    }
    rng.shuffle(&mut issues);
    let mut spec = gh_spec(issues);
    let m = spec.as_object_mut().expect("obj");
    match rng.below(14) {
        0 => {
            m.insert("raise".into(), s("HTTPError"));
        }
        1 => {
            m.insert(
                "fail_title".into(),
                s(rng.pick(&["superpowers", "claude-code", "ui"])),
            );
        }
        2 => m.insert("create_returns".into(), Value::Array(vec![])),
        3 => m.insert("create_returns".into(), obj(vec![])),
        _ => {}
    }
    spec
}

/// `(key, title, body)` of each change Rust finds in the world (with its startup sections).
fn render_world(
    files: &[(String, Value)],
    refs: &[(String, Value)],
    http: &[(String, Value)],
) -> Vec<(String, String, String)> {
    let dir = tempfile::tempdir().expect("tempdir");
    for (rel, body) in files {
        let path = dir.path().join(rel);
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).expect("mkdir");
        }
        std::fs::write(&path, body.as_str().unwrap_or("")).expect("write");
    }
    let refs = map(refs.to_vec());
    let http = map(http.to_vec());
    let net = FakeNet {
        refs: refs.as_object(),
        http: http.as_object(),
    };
    let (changes, _) = uw::check_all(dir.path(), &net, None);
    let root = anthology_dev::startup_budget::Root::new(dir.path(), Path::new("/"));
    let extra: HashMap<String, uw::Sections> =
        uw::startup_budget_sections(&root, &changes, &anthology_dev::startup_budget::BUDGET)
            .unwrap_or_default();
    changes
        .iter()
        .map(|c| {
            let sections = extra.get(&c.key).map_or(&[][..], Vec::as_slice);
            (
                c.key.clone(),
                uw::render_title(c),
                uw::render_body(c, sections),
            )
        })
        .collect()
}

fn gen_text(rng: &mut Rng) -> String {
    let mut t = String::new();
    for _ in 0..rng.index(6) {
        match rng.below(6) {
            0 => t.push_str("### Startup budget\n"),
            1 => t.push_str("### Update\n"),
            2 => t.push_str("\r\n"),
            3 => t.push('`'),
            _ => t.push_str(&rng.string(8)),
        }
    }
    t
}

fn gen_lib(rng: &mut Rng) -> Value {
    match rng.below(7) {
        0 => lib(
            "sanitise",
            vec![
                ("text", s(&gen_text(rng))),
                ("limit", Value::Int(Int::Small(rng.range(0, 90)))),
            ],
        ),
        1 => {
            let mut t = String::new();
            for _ in 0..rng.index(6) {
                let line = match rng.below(4) {
                    0 => format!("{}\trefs/tags/{}", gen_sha(rng), rng.pick(TAGS)),
                    1 => format!("{}\t", sha('1')),
                    2 => rng.string(10),
                    _ => format!("{}\trefs/heads/main", gen_sha(rng)),
                };
                t.push_str(&line);
                t.push_str(rng.pick(&["\n", "\r\n", "\u{2028}", "\u{1c}"]));
            }
            lib("ls_remote", vec![("text", s(&t))])
        }
        2 => {
            let mut b = gen_manifest(rng).into_bytes();
            if rng.chance(0.2) {
                b.extend_from_slice(b"\xff\xfe x\n");
            }
            lib("manifest", vec![("hex", s(&hex(&b)))])
        }
        3 => {
            let mut pairs = Vec::new();
            for _ in 0..rng.index(7) {
                let tag = rng.pick(TAGS);
                let peeled = if rng.chance(0.3) { "^{}" } else { "" };
                let commit = if rng.chance(0.1) {
                    String::new()
                } else {
                    gen_sha(rng)
                };
                pairs.push(strs(&[format!("refs/tags/{tag}{peeled}"), commit]));
            }
            let pin = *rng.pick(&["v7.0.2", "v7.0", "1.2.0", "v1.2.1", "v0.0.0", "0.1"]);
            lib(
                "latest",
                vec![("pairs", Value::Array(pairs)), ("pin", s(pin))],
            )
        }
        4 => lib("without_budget", vec![("text", s(&gen_text(rng)))]),
        5 => {
            let files: Vec<Value> = (0..rng.index(60))
                .map(|i| s(&format!("f{i:03}.md")))
                .collect();
            let files_total = files.len() as i64 + rng.range(0, 3) * 10;
            let change = obj(vec![
                ("key", s(rng.pick(NAMES))),
                ("kind", s("git")),
                ("old", s("v1")),
                ("new", s("v2")),
                ("new_target", s("v2")),
                ("command", s("scripts/dev.sh vendor sync x --ref v2")),
                (
                    "details",
                    Value::Array(vec![strs(&[gen_text(rng), gen_text(rng)])]),
                ),
                ("files", Value::Array(files)),
                ("files_total", Value::Int(Int::Small(files_total))),
                (
                    "files_heading",
                    s(&format!("Changed  files\t{}", gen_text(rng))),
                ),
                ("notes", Value::Array(vec![s("n")])),
            ]);
            let extra = Value::Array(vec![strs(&[gen_text(rng), gen_text(rng)])]);
            lib("render", vec![("change", change), ("extra", extra)])
        }
        _ => {
            let entry = match rng.below(6) {
                0 => loads("[]"),
                1 => loads("{\"upstream\": {}}"),
                2 => loads("{\"managed_by\": \"subtree\"}"),
                3 => loads("{\"url\": 1, \"checksum\": 2}"),
                4 => loads("{\"repo\": null}"),
                _ => loads("{\"upstream\": [], \"url\": \"u\"}"),
            };
            lib("classify", vec![("entry", entry)])
        }
    }
}
