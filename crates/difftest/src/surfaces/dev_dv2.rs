//! The engine of DV2's five surfaces: `dev.version-bump`, `dev.release-edit`, `dev.memory-diff`,
//! `dev.startup-budget` and `dev.eval-baseline`. The oracle is `port/difftest/oracle/dev_tools.py`.
//!
//! A case is one of:
//! - `{"op": "cli", "tool": T, "args": [...], "files": {rel: text}, "env": {k: v}, "show": [rel]}`:
//!   both sides write `files` into a fresh folder, make it the working folder, set the
//!   environment to PATH, HOME (the folder), LANG=C.UTF-8 plus `env`, and run the tool
//!   (Python: the script's `main(args)` in the oracle process; Rust: `<module>::run_in`). The
//!   result is the exit code, stdout and stderr, then for each `show` path `=== <rel> ===` and
//!   its text (`<absent>`). The folder is written `<ROOT>`.
//! - a library op calling one function, printing a fixed `repr` rendering of its result, or
//!   exiting 1 with `<Type>: <message>`: `parse` {v}, `compare` {a, b}, `changed` {paths},
//!   `verdict` {plugin, base, head, base_exists, head_exists} (version-bump); `range` {rng, v}
//!   (release-edit); `parse_ns` {hex}, `escape` {s}, `render` {entries} (memory-diff);
//!   `sections` {body}, `split` {text} (startup-budget).
//!
//! Normalization, both sides: `<TODAY>` for today's date (changelog-move without `--date`); D-2
//! the OS text in `[Errno N] <text>` (P1's errno tables are pending, ruling R3), so the errno
//! number and file names are still compared. argparse usage and help text is compared verbatim
//! (no D-4). The oracle marks D-1 where Python raised (a traceback; Rust prints
//! `anthology-dev <tool>: <Type>: ...`), and D-2 where it wrote eval-baseline's bare strerror
//! `errno N`, the Rust port's placeholder text.
//!
//! The Linux oracle image has no git, so the git paths of version-bump and memory-diff compare
//! their git-missing outcome there; macOS runs them for real.

use std::path::Path;

use anthology_core::Env;
use anthology_dev::json_edit::str_repr;
use anthology_dev::{
    eval_baseline, memory_diff, release_edit, semver, startup_budget, version_bump,
};
use anthology_pycompat::json::{self, Int, Map, PY_DEFAULT, Value};

use crate::case::{Case, Rng, Surface};
use crate::normalize::{LIBTEXT, Normalizer};
use crate::observe::{Observed, from_oracle};
use crate::oracle::Oracle;

/// One of DV2's tools.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Tool {
    VersionBump,
    ReleaseEdit,
    MemoryDiff,
    StartupBudget,
    EvalBaseline,
}

impl Tool {
    pub fn cli_name(self) -> &'static str {
        match self {
            Tool::VersionBump => "version-bump",
            Tool::ReleaseEdit => "release-edit",
            Tool::MemoryDiff => "memory-diff",
            Tool::StartupBudget => "startup-budget",
            Tool::EvalBaseline => "eval-baseline",
        }
    }

    fn surface_name(self) -> &'static str {
        match self {
            Tool::VersionBump => super::dev_version_bump::NAME,
            Tool::ReleaseEdit => super::dev_release_edit::NAME,
            Tool::MemoryDiff => super::dev_memory_diff::NAME,
            Tool::StartupBudget => super::dev_startup_budget::NAME,
            Tool::EvalBaseline => super::dev_eval_baseline::NAME,
        }
    }
}

/// The surface of one tool.
pub struct Dv2(pub Tool);

impl Surface for Dv2 {
    fn name(&self) -> &'static str {
        self.0.surface_name()
    }

    fn seeds(&self) -> Vec<Case> {
        seeds(self.0)
            .into_iter()
            .enumerate()
            .map(|(i, v)| Case::new(format!("seed:{}", i + 1), v))
            .collect()
    }

    fn generate(&self, rng: &mut Rng, n: usize) -> Vec<Case> {
        (0..n)
            .map(|i| Case::new(format!("gen:{}", i + 1), generate(self.0, rng)))
            .collect()
    }

    fn run_python(&self, c: &Case, oracle: &mut Oracle) -> Observed {
        let mut o = from_oracle(&oracle.call("dev_tools", &c.data));
        if normalize(&mut o) {
            oracle.mark("D-2");
        }
        normalizer().apply_python(&mut o, oracle);
        o
    }

    fn run_rust(&self, c: &Case) -> Observed {
        let mut o = run_rust(self.0, &c.data);
        normalize(&mut o);
        normalizer().apply(&mut o);
        o
    }
}

fn normalizer() -> Normalizer {
    Normalizer::new().today_local()
}

/// The D-2 rule: `[Errno N] <text>` → `[Errno N] <LIBTEXT>`, where `<text>` runs to the next
/// `:`, `)` or newline. True when it changed `o`.
fn normalize(o: &mut Observed) -> bool {
    let before = (o.stdout.clone(), o.stderr.clone());
    o.stdout = errno_text(&o.stdout);
    o.stderr = errno_text(&o.stderr);
    (o.stdout.clone(), o.stderr.clone()) != before
}

fn errno_text(b: &[u8]) -> Vec<u8> {
    const OPEN: &[u8] = b"[Errno ";
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i..].starts_with(OPEN) {
            let digits = b[i + OPEN.len()..]
                .iter()
                .take_while(|c| c.is_ascii_digit())
                .count();
            let close = i + OPEN.len() + digits;
            if digits > 0 && b[close..].starts_with(b"] ") {
                out.extend_from_slice(&b[i..close + 2]);
                out.extend_from_slice(LIBTEXT.as_bytes());
                i = close + 2;
                while i < b.len() && !matches!(b[i], b':' | b')' | b'\n') {
                    i += 1;
                }
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

// ---- the Rust side ---------------------------------------------------------------------------

fn field<'a>(case: &'a Value, k: &str) -> &'a Value {
    static NULL: Value = Value::Null;
    case.as_object().and_then(|m| m.get(k)).unwrap_or(&NULL)
}

fn text<'a>(case: &'a Value, k: &str) -> &'a str {
    field(case, k).as_str().unwrap_or("")
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn ok(stdout: String) -> Observed {
    Observed::text(0, &format!("{stdout}\n"), "")
}

fn raised(ty: &str, msg: &str) -> Observed {
    Observed::text(1, "", &format!("{ty}: {msg}\n"))
}

fn py_bool(b: bool) -> &'static str {
    if b { "True" } else { "False" }
}

fn list_repr(items: &[String]) -> String {
    let shown: Vec<String> = items.iter().map(|s| str_repr(s)).collect();
    format!("[{}]", shown.join(", "))
}

fn run_rust(tool: Tool, case: &Value) -> Observed {
    match text(case, "op") {
        "cli" => run_cli(tool, case),
        "parse" => ok(match semver::parse_value(field(case, "v")) {
            None => "None".into(),
            Some(s) => format!(
                "({}, {}, {}, {})",
                s.major,
                s.minor,
                s.patch,
                list_repr(&s.pre)
            ),
        }),
        "compare" => match semver::compare(text(case, "a"), text(case, "b")) {
            Ok(o) => ok((o as i8).to_string()),
            Err(e) => raised("ValueError", &e.0),
        },
        "changed" => ok(list_repr(&version_bump::changed_plugins(&strings(field(
            case, "paths",
        ))))),
        "verdict" => {
            let (good, why) = version_bump::verdict(
                text(case, "plugin"),
                field(case, "base"),
                field(case, "head"),
                field(case, "base_exists").as_bool().unwrap_or(false),
                field(case, "head_exists").as_bool().unwrap_or(false),
            );
            ok(format!("({}, {})", py_bool(good), str_repr(&why)))
        }
        "range" => match semver::range_accepts(field(case, "rng"), text(case, "v")) {
            Ok(b) => ok(py_bool(b).into()),
            Err(semver::RangeError::Unsupported(m)) => raised("UnsupportedRange", &m),
            Err(semver::RangeError::BadVersion(m)) => raised("ReleaseError", &m),
            Err(semver::RangeError::Uncaught { ty, msg }) => raised(ty, &msg),
        },
        "parse_ns" => {
            let raw = unhex(text(case, "hex"));
            let entries: Vec<String> = memory_diff::parse_name_status(&raw)
                .iter()
                .map(|(l, p)| format!("({}, {})", str_repr(l), list_repr(p)))
                .collect();
            ok(format!("[{}]", entries.join(", ")))
        }
        "escape" => ok(memory_diff::escape_name(text(case, "s"))),
        "render" => {
            let entries: Vec<memory_diff::Entry> = field(case, "entries")
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|e| {
                            let e = e.as_array().expect("an entry");
                            (e[0].as_str().unwrap_or("").to_string(), strings(&e[1]))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let r = memory_diff::render(&entries);
            Observed::text(0, &format!("{r}\n"), "")
        }
        "sections" => {
            let (names, map) = startup_budget::sections(text(case, "body"));
            let pairs: Vec<String> = map
                .iter()
                .map(|(k, v)| format!("{}: {}", str_repr(k), str_repr(v)))
                .collect();
            ok(format!("({}, {{{}}})", list_repr(&names), pairs.join(", ")))
        }
        "split" => {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = dir.path().join("agent.md");
            std::fs::write(&path, text(case, "text")).expect("write");
            let root = dir.path().to_string_lossy().into_owned();
            match startup_budget::split_agent(&path) {
                Ok((skills, body)) => ok(format!("({}, {})", list_repr(&skills), str_repr(&body))
                    .replace(&root, "<ROOT>")),
                Err(e) => raised(&e.ty, &e.msg.replace(&root, "<ROOT>")),
            }
        }
        other => panic!("dev_dv2: unknown op {other:?}"),
    }
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).expect("hex"))
        .collect()
}

/// The environment both sides give the tool: PATH, HOME, LANG and the case's `env`.
fn env_for(case: &Value, home: &Path) -> Env {
    let process = Env::from_process();
    let mut env = process.clone();
    let keys: Vec<String> = process
        .iter()
        .filter_map(|(k, _)| k.to_str().map(str::to_string))
        .collect();
    for k in keys {
        env = env.without(&k);
    }
    env = env
        .with("PATH", process.get("PATH").unwrap_or("/usr/bin:/bin"))
        .with_os("HOME", home)
        .with("LANG", "C.UTF-8");
    if let Some(m) = field(case, "env").as_object() {
        for (k, v) in m.iter() {
            env = env.with(k, v.as_str().unwrap_or(""));
        }
    }
    env
}

fn run_cli(tool: Tool, case: &Value) -> Observed {
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
    let args: Vec<std::ffi::OsString> = strings(field(case, "args"))
        .into_iter()
        .map(Into::into)
        .collect();
    let env = env_for(case, root);
    let stdin = &mut std::io::empty();
    let out = match tool {
        Tool::VersionBump => version_bump::run_in(&args, stdin, &env, root),
        Tool::ReleaseEdit => release_edit::run_in(&args, stdin, &env, root),
        Tool::MemoryDiff => memory_diff::run_in(&args, stdin, &env, root),
        Tool::StartupBudget => startup_budget::run_in(&args, stdin, &env, root),
        Tool::EvalBaseline => eval_baseline::run_in(&args, stdin, &env, root),
    };
    let mut stdout = out.stdout;
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
        // The realpath first: on macOS it is the longer one (/private/var/… for /var/…).
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

fn replace(hay: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(hay.len());
    let mut i = 0;
    while i < hay.len() {
        if hay[i..].starts_with(from) {
            out.extend_from_slice(to);
            i += from.len();
        } else {
            out.push(hay[i]);
            i += 1;
        }
    }
    out
}

// ---- cases -----------------------------------------------------------------------------------

fn s(t: &str) -> Value {
    Value::Str(t.to_string())
}

fn arr(items: &[&str]) -> Value {
    Value::Array(items.iter().map(|t| s(t)).collect())
}

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    let mut m = Map::new();
    for (k, v) in pairs {
        m.insert(k.to_string(), v);
    }
    Value::Object(m)
}

fn files(pairs: &[(&str, String)]) -> Value {
    let mut m = Map::new();
    for (k, v) in pairs {
        m.insert(k.to_string(), Value::Str(v.clone()));
    }
    Value::Object(m)
}

fn cli(tool: Tool, args: &[&str], fs: Value, env: Value, show: &[&str]) -> Value {
    obj(vec![
        ("op", s("cli")),
        ("tool", s(tool.cli_name())),
        ("args", arr(args)),
        ("files", fs),
        ("env", env),
        ("show", arr(show)),
    ])
}

fn no_env() -> Value {
    Value::Object(Map::new())
}

fn lib(op: &str, pairs: Vec<(&str, Value)>) -> Value {
    let mut all = vec![("op", s(op))];
    all.extend(pairs);
    obj(all)
}

fn loads(t: &str) -> Value {
    json::loads(t).expect("case json")
}

const VERSIONS: &[&str] = &[
    "0.1.0",
    "1.2.3",
    "10.20.30",
    "1.0.0-rc.1",
    "1.0.0-alpha",
    "1.0.0-alpha.1",
    "1.0.0-0.3.7",
    "1.0.0-x.7.z.92",
    "1.0.0+build.5",
    "1.0.0-rc.1+b",
    "01.2.3",
    "1.2",
    "1.2.3.4",
    "v1.2.3",
    "1.2.3-",
    "1.2.3-01",
    "1.2.3-0a",
    " 1.2.3",
    "1.2.3\n",
    "99999999999999999999999.0.0",
    "1.2.3-99999999999999999999",
    "1.2.3-a..b",
    "",
    "1.0.0-beta.11",
    "1.0.0-beta.2",
    "2.0.0",
    "1.10.0",
    "1.9.0",
    "0.0.0",
    "1.2.3-\u{663}",
    "\u{663}.2.3",
];

fn version(rng: &mut Rng) -> String {
    if rng.chance(0.6) {
        return rng.pick(VERSIONS).to_string();
    }
    let num = |rng: &mut Rng| -> String {
        match rng.below(6) {
            0 => "0".into(),
            1 => format!("0{}", rng.below(10)),
            2 => "1".repeat(1 + rng.index(25)),
            _ => rng.below(30).to_string(),
        }
    };
    let mut v = format!("{}.{}.{}", num(rng), num(rng), num(rng));
    if rng.chance(0.4) {
        v.push('-');
        let ids: Vec<String> = (0..1 + rng.index(3))
            .map(|_| match rng.below(4) {
                0 => num(rng),
                1 => rng
                    .pick(&["alpha", "beta", "rc", "x-y", "-", "a1"])
                    .to_string(),
                2 => rng.ascii_string(3),
                _ => String::new(),
            })
            .collect();
        v.push_str(&ids.join("."));
    }
    if rng.chance(0.2) {
        v.push('+');
        v.push_str(rng.pick(&["b", "build.5", "", "x..y", "001"]));
    }
    if rng.chance(0.1) {
        let i = rng.index(v.len() + 1);
        if v.is_char_boundary(i) {
            v.insert(i, rng.unicode_char());
        }
    }
    v
}

fn odd_value(rng: &mut Rng) -> Value {
    match rng.below(8) {
        0 => Value::Null,
        1 => Value::Bool(rng.chance(0.5)),
        2 => Value::Int(Int::Small(rng.range(-3, 30))),
        3 => Value::Float(*rng.pick(&[1.5, 0.0, -0.0, 1e16, 2.0])),
        4 => loads("[1, \"1.2.3\"]"),
        5 => loads("{\"v\": \"1.2.3\"}"),
        _ => s(&version(rng)),
    }
}

fn range_text(rng: &mut Rng) -> String {
    let op = *rng.pick(&["", "~", "^", ">=", ">", "<", "=", "~>", "^^", " ^"]);
    let base = match rng.below(4) {
        0 => rng
            .pick(&[
                "0", "1", "2", "10", "0.0", "0.1", "1.0", "1.2", "01", "1.02", "1.x", "*",
            ])
            .to_string(),
        _ => version(rng),
    };
    let mut r = format!("{op}{base}");
    if rng.chance(0.05) {
        r.push(*rng.pick(&[' ', '\n', '\u{a0}', '\u{1c}']));
    }
    r
}

fn plugin_name(rng: &mut Rng) -> String {
    rng.pick(&[
        "anthology",
        "anthology-guardrails",
        "a",
        "anthology-superpowers",
        "x.y",
        "é",
    ])
    .to_string()
}

fn manifest(rng: &mut Rng, name: &str, version: &Value, deps: Option<Value>) -> String {
    let mut m = Map::new();
    if rng.chance(0.3) {
        m.insert(
            "meta".into(),
            loads(r#"{"version": "9.9.9", "x": [1, {"version": "8"}]}"#),
        );
    }
    m.insert("name".into(), s(name));
    if !matches!(version, Value::Null) || rng.chance(0.5) {
        m.insert(
            "version".into(),
            json::loads(&json::dumps(version, &PY_DEFAULT)).expect("v"),
        );
    }
    if let Some(d) = deps {
        m.insert("dependencies".into(), d);
    }
    let text = match rng.below(3) {
        0 => json::dumps(&Value::Object(m), &json::INDENT2),
        1 => json::dumps(&Value::Object(m), &PY_DEFAULT),
        _ => json::dumps(&Value::Object(m), &json::INDENT2).replace('\n', "\r\n"),
    };
    if rng.chance(0.05) {
        text[..text.len() / 2].to_string()
    } else {
        text + "\n"
    }
}

const CHANGELOG: &str = "# Changelog\n\nIntro text.\n\n## anthology\n\n### [Unreleased]\n\n\
#### Added\n- a thing\n\n### [0.1.0] - 2026-01-01\n- first\n\n## anthology-guardrails\n\n\
### [Unreleased]\n\n#### Fixed\n\n## anthology-superpowers\n\n### [Unreleased]\n- sp entry\n";

fn changelog(rng: &mut Rng) -> String {
    let mut lines: Vec<String> = CHANGELOG
        .split_inclusive('\n')
        .map(str::to_string)
        .collect();
    for _ in 0..rng.below(3) {
        let i = rng.index(lines.len());
        match rng.below(4) {
            0 => {
                lines.remove(i);
            }
            1 => lines.insert(
                i,
                rng.pick(&[
                    "\n",
                    "### [0.2.0]\n",
                    "## anthology\n",
                    "- x\n",
                    "  \n",
                    "### [Unreleased]\n",
                ])
                .to_string(),
            ),
            2 => lines[i] = lines[i].replace('\n', "\r\n"),
            _ => lines[i] = lines[i].trim_end_matches('\n').to_string(),
        }
    }
    lines.concat()
}

fn sources(rng: &mut Rng) -> String {
    let entry = |t: &str, v: &str| {
        format!(
            "    {{\n      \"name\": \"{t}\",\n      \"target\": \"plugins/{t}\",\n      \"plugin_version\": \"{v}\"\n    }}"
        )
    };
    let mut entries = vec![
        entry("anthology-superpowers", "0.1.0"),
        entry("rtk", "1.2.1"),
    ];
    if rng.chance(0.2) {
        entries.push("    {\"name\": \"x\", \"target\": \"plugins/x\"}".into());
    }
    if rng.chance(0.1) {
        entries.push("    1".into());
    }
    rng.shuffle(&mut entries);
    format!(
        "{{\n  \"version\": 1,\n  \"sources\": [\n{}\n  ]\n}}\n",
        entries.join(",\n")
    )
}

fn seeds(tool: Tool) -> Vec<Value> {
    let mut rng = Rng::new(0xd2);
    let mut out: Vec<Value> = Vec::new();
    match tool {
        Tool::VersionBump => {
            for v in VERSIONS {
                out.push(lib("parse", vec![("v", s(v))]));
            }
            for (a, b) in [
                ("1.0.0", "1.0.0-rc.1"),
                ("1.0.0-alpha", "1.0.0-alpha.1"),
                ("1.0.0-alpha.1", "1.0.0-alpha.beta"),
                ("1.0.0-beta.2", "1.0.0-beta.11"),
                ("1.10.0", "1.9.0"),
                ("1.0.0+a", "1.0.0+b"),
                ("x", "1.0.0"),
            ] {
                out.push(lib("compare", vec![("a", s(a)), ("b", s(b))]));
            }
            out.push(lib(
                "changed",
                vec![(
                    "paths",
                    arr(&[
                        "plugins/a/x",
                        "plugins/a/y",
                        "plugins/b/z",
                        "README.md",
                        "plugins/c",
                        "pluginsx/d/e",
                        "plugins//x",
                    ]),
                )],
            ));
            out.push(lib(
                "verdict",
                vec![
                    ("plugin", s("p")),
                    ("base", s("0.1.0")),
                    ("head", s("0.1.0")),
                    ("base_exists", Value::Bool(true)),
                    ("head_exists", Value::Bool(true)),
                ],
            ));
            out.push(lib(
                "verdict",
                vec![
                    ("plugin", s("p")),
                    ("base", Value::Null),
                    ("head", loads("1")),
                    ("base_exists", Value::Bool(true)),
                    ("head_exists", Value::Bool(true)),
                ],
            ));
            for args in [
                &["0000000", "HEAD"][..],
                &["0", "x", "--repo", "."],
                &[],
                &["a"],
                &["a", "b", "--repo"],
                &["a", "b", "--x"],
                &["a", "b"],
                &["--", "0", "x"],
                &["-h"],
            ] {
                out.push(cli(tool, args, no_env(), no_env(), &[]));
            }
        }
        Tool::ReleaseEdit => {
            for rng_text in [
                "^1.0",
                "^1",
                "~1",
                "~1.2",
                "^0.0",
                "^0.1",
                "^0",
                "~0",
                "1.2.3",
                ">=1.2.3",
                "^1.2.3-beta.1",
                "*",
                "1.x",
                ">1.2.3",
                "^1.2.3+b",
                " ^1.0",
                "^1.0\n",
                "^01",
                "^1.2.",
                "^1\nx",
            ] {
                for v in [
                    "1.0.0",
                    "1.9.3",
                    "2.0.0",
                    "0.9.9",
                    "1.0.1-rc.1",
                    "1.2.3-beta.2",
                    "0.0.5",
                    "bad",
                ] {
                    out.push(lib("range", vec![("rng", s(rng_text)), ("v", s(v))]));
                }
            }
            out.push(lib("range", vec![("rng", loads("1")), ("v", s("1.0.0"))]));
            out.push(lib("range", vec![("rng", Value::Null), ("v", s("1.0.0"))]));
            let pj = "plugins/anthology/.claude-plugin/plugin.json";
            let plugin = "{\n  \"name\": \"anthology\",\n  \"version\": \"0.1.0\",\n  \"meta\": {\"version\": \"x\"}\n}\n".to_string();
            out.push(cli(
                tool,
                &["plugin-version", pj],
                files(&[(pj, plugin.clone())]),
                no_env(),
                &[],
            ));
            out.push(cli(
                tool,
                &["set-version", pj, "0.2.0"],
                files(&[(pj, plugin.clone())]),
                no_env(),
                &[pj],
            ));
            out.push(cli(
                tool,
                &["set-version", pj, "0.2.0-é"],
                files(&[(pj, plugin)]),
                no_env(),
                &[pj],
            ));
            out.push(cli(
                tool,
                &["set-version", "missing.json", "1.0.0"],
                no_env(),
                no_env(),
                &[],
            ));
            for (cur, new, tag) in [
                ("0.1.0", "0.2.0", "0"),
                ("0.1.0", "0.1.0", "0"),
                ("0.1.0", "0.1.0", "1"),
                ("0.2.0", "0.1.0", "0"),
                ("x", "1.0.0", "0"),
                ("1.0.0", "y", "0"),
                ("1.0.0", "1.0.1", "2"),
            ] {
                out.push(cli(
                    tool,
                    &["check-version", cur, new, tag],
                    no_env(),
                    no_env(),
                    &[],
                ));
            }
            for plugin in [
                "anthology",
                "anthology-guardrails",
                "anthology-superpowers",
                "missing",
            ] {
                let cl = files(&[("CHANGELOG.md", CHANGELOG.to_string())]);
                out.push(cli(
                    tool,
                    &["changelog-check", "CHANGELOG.md", plugin],
                    cl,
                    no_env(),
                    &[],
                ));
                let cl = files(&[("CHANGELOG.md", CHANGELOG.to_string())]);
                out.push(cli(
                    tool,
                    &[
                        "changelog-move",
                        "CHANGELOG.md",
                        plugin,
                        "0.2.0",
                        "--date",
                        "2026-11-21",
                    ],
                    cl,
                    no_env(),
                    &["CHANGELOG.md"],
                ));
            }
            let cl = files(&[("CHANGELOG.md", CHANGELOG.to_string())]);
            out.push(cli(
                tool,
                &["changelog-move", "CHANGELOG.md", "anthology", "0.2.0"],
                cl,
                no_env(),
                &["CHANGELOG.md"],
            ));
            let cl = files(&[("CHANGELOG.md", CHANGELOG.to_string())]);
            out.push(cli(
                tool,
                &[
                    "changelog-move",
                    "CHANGELOG.md",
                    "anthology",
                    "0.1.0",
                    "--date",
                    "x",
                ],
                cl,
                no_env(),
                &["CHANGELOG.md"],
            ));
            out.push(cli(
                tool,
                &[
                    "set-source-version",
                    "vendor/sources.json",
                    "plugins/anthology-superpowers",
                    "0.2.0",
                ],
                files(&[("vendor/sources.json", sources(&mut rng))]),
                no_env(),
                &["vendor/sources.json"],
            ));
            let deps = |r: &str| {
                loads(&format!(
                    "[\"anthology\", {{\"name\": \"anthology\", \"version\": {}}}]",
                    json::dumps(&s(r), &PY_DEFAULT)
                ))
            };
            for r in ["^0.1", "~0.1.0", "*", "^1"] {
                let tree = files(&[
                    (
                        "plugins/anthology/.claude-plugin/plugin.json",
                        "{\"name\": \"anthology\", \"version\": \"0.1.0\"}".into(),
                    ),
                    (
                        "plugins/dep/.claude-plugin/plugin.json",
                        manifest(&mut rng, "dep", &s("1.0.0"), Some(deps(r))),
                    ),
                    ("plugins/bad/.claude-plugin/plugin.json", "{not json".into()),
                ]);
                out.push(cli(
                    tool,
                    &["check-dependents", ".", "anthology", "0.2.0"],
                    tree,
                    no_env(),
                    &[],
                ));
            }
            for args in [
                &[][..],
                &["nope"],
                &["--", "check-version", "1.0.0", "1.0.1", "1"],
                &["check-version", "1", "2"],
                &["check-version", "a", "b", "c"],
                &["--=x"],
                &["-=x"],
                &["set-version", "--=x", "a"],
            ] {
                out.push(cli(tool, args, no_env(), no_env(), &[]));
            }
        }
        Tool::MemoryDiff => {
            for raw in [
                &b"A\0.claude/memory/a\nb.md\0R100\0.claude/memory/old.md\0.claude/memory/new.md\0"
                    [..],
                b"",
                b"M\0m\0D\0d\0T\0t\0C75\0src\0dst\0",
                b"A\0a\xffb\0R\0only\0",
                b"\0x\0",
            ] {
                out.push(lib("parse_ns", vec![("hex", s(&hex(raw)))]));
            }
            for name in [
                "a\nb\rc\td\x00e\x1b",
                "a`b|c",
                "a\u{2028}b\u{85}c",
                &"x".repeat(500),
                "\u{200b}\u{e000}\u{378}\u{fffd}\u{feff}",
                "é\u{1f600}",
            ] {
                out.push(lib("escape", vec![("s", s(name))]));
            }
            out.push(lib("render", vec![("entries", loads("[]"))]));
            out.push(lib(
                "render",
                vec![(
                    "entries",
                    loads(r#"[["A", ["a|b"]], ["R", ["x", "y`"]], ["", []]]"#),
                )],
            ));
            for args in [
                &["only-one"][..],
                &["--output=x", "HEAD"],
                &[],
                &["a", "b", "c"],
                &["a", "-b"],
                &["base", "head"],
            ] {
                out.push(cli(tool, args, no_env(), no_env(), &[]));
            }
        }
        Tool::StartupBudget => {
            for args in [
                &[][..],
                &["researcher"],
                &["nope"],
                &["researcher", "scout", "researcher"],
                &["--root"],
                &["-x"],
            ] {
                out.push(cli(tool, args, no_env(), no_env(), &[]));
            }
            out.push(cli(
                tool,
                &["--root", "."],
                agent_repo(&mut rng, false),
                no_env(),
                &[],
            ));
            out.push(cli(
                tool,
                &["--root", ".", "reader"],
                agent_repo(&mut rng, false),
                no_env(),
                &[],
            ));
            for t in [
                "---\nskills:\n  - a:b\n  - c\nx: 1\n---\nbody",
                "no",
                "---\nunterminated",
                "---\n---\nx",
                "---\nskills:\n- a\n---\n\n\nb",
            ] {
                out.push(lib("split", vec![("text", s(t))]));
            }
            out.push(lib(
                "sections",
                vec![("body", s("intro\n## A \nx\n## B\ny\n## A\nz"))],
            ));
        }
        Tool::EvalBaseline => {
            let result = result_json(
                &mut rng,
                &[("route-scout", "1.0"), ("guard-g1", "0.5")],
                &[],
            );
            let base = baseline_json(
                &[("route-scout", "1.0"), ("guard-g1", "0.75")],
                "\"2.1.311\"",
            );
            out.push(cli(
                tool,
                &["record", "result.json", "baseline.json"],
                files(&[("result.json", result.clone())]),
                no_env(),
                &["baseline.json"],
            ));
            out.push(cli(
                tool,
                &["candidate", "result.json", "c.json"],
                files(&[("result.json", result.clone())]),
                no_env(),
                &["c.json"],
            ));
            out.push(cli(
                tool,
                &[
                    "compare",
                    "result.json",
                    "baseline.json",
                    "--summary",
                    "s.md",
                ],
                files(&[
                    ("result.json", result.clone()),
                    ("baseline.json", base.clone()),
                ]),
                no_env(),
                &["s.md"],
            ));
            out.push(cli(
                tool,
                &["compare", "result.json", "missing.json"],
                files(&[("result.json", result.clone())]),
                no_env(),
                &[],
            ));
            out.push(cli(
                tool,
                &[
                    "compare",
                    "result.json",
                    "baseline.json",
                    "--tolerance",
                    "0.3",
                    "--allow-missing",
                ],
                files(&[("result.json", result), ("baseline.json", base)]),
                no_env(),
                &[],
            ));
            for args in [
                &[][..],
                &["nope"],
                &["record"],
                &["compare", "a", "b", "--tolerance", "x"],
                &["--x", "record", "a", "b", "c"],
                &["compare", "a", "b", "--allow-missing=1"],
                &["record", "nope.json", "b.json"],
            ] {
                out.push(cli(tool, args, no_env(), no_env(), &[]));
            }
        }
    }
    out
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// A tests/_agents.py-style agent repository, perturbed.
fn agent_repo(rng: &mut Rng, perturb: bool) -> Value {
    let mut fs: Vec<(String, String)> = vec![
        (
            "plugins/anthology/knowledge/alpha/SKILL.md".into(),
            "a".repeat(1 + rng.index(200)),
        ),
        (
            "plugins/anthology/knowledge/beta/SKILL.md".into(),
            "b".repeat(1 + rng.index(200)),
        ),
        (
            "plugins/anthology-superpowers/skills/plan/SKILL.md".into(),
            "p".repeat(1 + rng.index(400)),
        ),
    ];
    let line = |k: &str| format!("- `${{CLAUDE_PLUGIN_ROOT}}/knowledge/{k}/SKILL.md`");
    let agent = |name: &str, skills: &str, files: &str| {
        format!(
            "---\nname: {name}\ndescription: \"Use when testing.\"\nmodel: haiku\n{skills}---\n\n## Startup\nBefore any other work, Read every file below, in order:\n{files}\n\n## Role\nTest agent.\n"
        )
    };
    fs.push((
        "plugins/anthology/agents/reader.md".into(),
        agent(
            "reader",
            "",
            &format!("{}\n{}", line("alpha"), line("beta")),
        ),
    ));
    fs.push((
        "plugins/anthology/agents/skilled.md".into(),
        agent(
            "skilled",
            "skills:\n  - anthology-superpowers:plan\n",
            &line("beta"),
        ),
    ));
    fs.push((
        "plugins/anthology/agents/bare.md".into(),
        agent("bare", "", ""),
    ));
    if perturb {
        for _ in 0..1 + rng.index(3) {
            let i = rng.index(fs.len());
            let (path, body) = fs[i].clone();
            let new = match rng.below(9) {
                0 => body.replace('\n', "\r\n"),
                1 => body.replacen("---\n", "", 1),
                2 => body.replace("## Startup", "## Startup\n## Startup"),
                3 => body.replace("alpha", "gamma"),
                4 => body.replace("skills:\n", "skills: \u{a0}\n"),
                5 => body.replace("knowledge/", "knowledge/./"),
                6 => {
                    fs.push((
                        format!(
                            "plugins/anthology/agents/{}.md",
                            rng.pick(&["a b", "évil", "x.y", ".hid", "Z9", "-n"])
                        ),
                        body.clone(),
                    ));
                    body
                }
                7 => body.replace("Role", &rng.string(6)),
                _ => {
                    fs.remove(i);
                    continue;
                }
            };
            fs[i] = (path, new);
        }
    }
    let pairs: Vec<(&str, String)> = fs.iter().map(|(p, b)| (p.as_str(), b.clone())).collect();
    files(&pairs)
}

fn result_json(rng: &mut Rng, scores: &[(&str, &str)], extra: &[(&str, &str)]) -> String {
    let cases: Vec<String> = scores
        .iter()
        .map(|(n, sc)| {
            if rng.chance(0.05) {
                format!("{{\"name\": {}}}", json::dumps(&s(n), &PY_DEFAULT))
            } else if rng.chance(0.04) {
                let name = rng.pick(&["1", "null", "[\"a\"]", "true", "1.5", "{}"]);
                format!("{{\"name\": {name}, \"aggregates\": {{\"score\": {sc}}}}}")
            } else {
                format!(
                    "{{\"name\": {}, \"aggregates\": {{\"score\": {sc}, \"passRate\": 1.0}}}}",
                    json::dumps(&s(n), &PY_DEFAULT)
                )
            }
        })
        .collect();
    let mut m = json::loads(&format!(
        "{{\"claudeVersion\": \"2.1.311\", \"costUsd\": 4.5, \"startedAt\": \"2026-11-20T05:41:12.000Z\", \"partial\": false, \"partialReason\": null, \"cases\": [{}]}}",
        cases.join(", ")
    ))
    .expect("result");
    let o = m.as_object_mut().expect("object");
    for (k, v) in extra {
        o.insert(k.to_string(), loads(v));
    }
    json::dumps(&m, &PY_DEFAULT)
}

fn baseline_json(scores: &[(&str, &str)], version: &str) -> String {
    let cases: Vec<String> = scores
        .iter()
        .map(|(n, sc)| format!("{}: {sc}", json::dumps(&s(n), &PY_DEFAULT)))
        .collect();
    format!(
        "{{\"schema_version\": 1, \"claude_code_version\": {version}, \"recorded_at\": \"2026-11-13\", \"cost_usd\": 1.0, \"cases\": {{{}}}}}",
        cases.join(", ")
    )
}

fn case_name(rng: &mut Rng) -> String {
    match rng.below(10) {
        0 => rng.string(8),
        1 => rng
            .pick(&["A b", "a--b", "-a", "a-", "", "a|b\n::error::x"])
            .to_string(),
        _ => rng
            .pick(&["a", "b", "route-scout", "guard-g1", "n", "x-1-y"])
            .to_string(),
    }
}

fn score(rng: &mut Rng) -> String {
    rng.pick(&[
        "1.0",
        "0.5",
        "0.0625",
        "0.1875",
        "1",
        "0",
        "-0.0",
        "NaN",
        "Infinity",
        "1e400",
        "\"1.0\"",
        "null",
        "true",
        "0.95",
        "0.9",
        "123456789012345678901234567890",
        "2.5e-4",
    ])
    .to_string()
}

fn generate(tool: Tool, rng: &mut Rng) -> Value {
    match tool {
        Tool::VersionBump => match rng.below(10) {
            0..=2 => lib(
                "parse",
                vec![(
                    "v",
                    if rng.chance(0.85) {
                        s(&version(rng))
                    } else {
                        odd_value(rng)
                    },
                )],
            ),
            3..=5 => lib(
                "compare",
                vec![("a", s(&version(rng))), ("b", s(&version(rng)))],
            ),
            6 => {
                let paths: Vec<String> = (0..rng.index(6))
                    .map(|_| {
                        let parts = [
                            "plugins", "plugins/", "a", "b/", "/", "x.md", "plugin", "é", "",
                        ];
                        (0..1 + rng.index(4))
                            .map(|_| *rng.pick(&parts))
                            .collect::<Vec<_>>()
                            .join(if rng.chance(0.7) { "/" } else { "" })
                    })
                    .collect();
                lib(
                    "changed",
                    vec![("paths", Value::Array(paths.iter().map(|p| s(p)).collect()))],
                )
            }
            7 | 8 => lib(
                "verdict",
                vec![
                    ("plugin", s(&plugin_name(rng))),
                    ("base", odd_value(rng)),
                    ("head", odd_value(rng)),
                    ("base_exists", Value::Bool(rng.chance(0.9))),
                    ("head_exists", Value::Bool(rng.chance(0.9))),
                ],
            ),
            _ => {
                let pool = [
                    "0000", "0", "HEAD", "--repo", ".", "--re", "--repo=.", "-h", "--", "x", "-1",
                    "--=x", "abc", "-h-", "-hh-", "-hh=x", "-hX", "-h=", "--help=x",
                ];
                let args: Vec<&str> = (0..rng.index(5)).map(|_| *rng.pick(&pool)).collect();
                cli(tool, &args, no_env(), no_env(), &[])
            }
        },
        Tool::ReleaseEdit => match rng.below(10) {
            0..=4 => {
                let r = if rng.chance(0.95) {
                    s(&range_text(rng))
                } else {
                    odd_value(rng)
                };
                lib("range", vec![("rng", r), ("v", s(&version(rng)))])
            }
            5 => {
                let pj = ".claude-plugin/plugin.json";
                let v = odd_value(rng);
                let name = plugin_name(rng);
                let body = manifest(rng, &name, &v, None);
                let new = version(rng);
                let sub = *rng.pick(&["plugin-version", "set-version", "set-version"]);
                let args: Vec<&str> = if sub == "plugin-version" {
                    vec![sub, pj]
                } else {
                    vec![sub, pj, &new]
                };
                cli(tool, &args, files(&[(pj, body)]), no_env(), &[pj])
            }
            6 => {
                let plugin = rng
                    .pick(&[
                        "anthology",
                        "anthology-guardrails",
                        "anthology-superpowers",
                        "anthology-",
                        "x",
                    ])
                    .to_string();
                let v = version(rng);
                let date = rng
                    .pick(&[
                        "--date=2026-11-21",
                        "--date=bad",
                        "--date=\u{663}\u{663}\u{663}\u{663}-01-01",
                        "--date=",
                    ])
                    .to_string();
                let cl = files(&[("CHANGELOG.md", changelog(rng))]);
                if rng.chance(0.3) {
                    cli(
                        tool,
                        &["changelog-check", "CHANGELOG.md", &plugin],
                        cl,
                        no_env(),
                        &[],
                    )
                } else {
                    cli(
                        tool,
                        &["changelog-move", "CHANGELOG.md", &plugin, &v, &date],
                        cl,
                        no_env(),
                        &["CHANGELOG.md"],
                    )
                }
            }
            7 => {
                let target = rng
                    .pick(&[
                        "plugins/anthology-superpowers",
                        "plugins/rtk",
                        "plugins/x",
                        "nope",
                    ])
                    .to_string();
                let v = version(rng);
                cli(
                    tool,
                    &["set-source-version", "vendor/sources.json", &target, &v],
                    files(&[("vendor/sources.json", sources(rng))]),
                    no_env(),
                    &["vendor/sources.json"],
                )
            }
            8 => {
                let mut tree: Vec<(String, String)> = vec![(
                    "plugins/anthology/.claude-plugin/plugin.json".into(),
                    "{\"name\": \"anthology\", \"version\": \"0.1.0\"}\n".into(),
                )];
                for name in ["dep", "Dep", "z.y", ".hidden"] {
                    if rng.chance(0.6) {
                        let deps: Vec<String> = (0..rng.index(3))
                            .map(|_| match rng.below(4) {
                                0 => "\"anthology\"".into(),
                                1 => format!(
                                    "{{\"name\": \"anthology\", \"version\": {}}}",
                                    json::dumps(&s(&range_text(rng)), &PY_DEFAULT)
                                ),
                                2 => "{\"name\": \"anthology\"}".into(),
                                _ => format!(
                                    "{{\"name\": \"anthology\", \"version\": {}}}",
                                    json::dumps(&odd_value(rng), &PY_DEFAULT)
                                ),
                            })
                            .collect();
                        let body = format!(
                            "{{\"name\": \"{name}\", \"dependencies\": [{}]}}",
                            deps.join(", ")
                        );
                        tree.push((format!("plugins/{name}/.claude-plugin/plugin.json"), body));
                    }
                }
                let v = version(rng);
                let pairs: Vec<(&str, String)> =
                    tree.iter().map(|(p, b)| (p.as_str(), b.clone())).collect();
                cli(
                    tool,
                    &["check-dependents", ".", "anthology", &v],
                    files(&pairs),
                    no_env(),
                    &[],
                )
            }
            _ => {
                let pool = [
                    "check-version",
                    "1.0.0",
                    "1.0.1",
                    "0",
                    "1",
                    "2",
                    "--",
                    "-h",
                    "--date",
                    "x",
                    "set-version",
                    "--=x",
                    "-=x",
                    "plugin-version",
                    "-1",
                    "-h-",
                    "-hh-",
                    "-hh=x",
                    "-hX",
                    "-hh",
                    "-h=",
                ];
                let args: Vec<&str> = (0..rng.index(6)).map(|_| *rng.pick(&pool)).collect();
                cli(tool, &args, no_env(), no_env(), &[])
            }
        },
        Tool::MemoryDiff => match rng.below(10) {
            0..=3 => {
                let mut raw = Vec::new();
                for _ in 0..rng.index(6) {
                    let field: Vec<u8> = match rng.below(5) {
                        0 => rng
                            .pick(&[&b"A"[..], b"M", b"D", b"R100", b"C75", b"T", b"", b"X"])
                            .to_vec(),
                        1 => vec![0xff, b'a', 0xc3],
                        _ => rng.string(10).into_bytes(),
                    };
                    raw.extend_from_slice(&field);
                    raw.push(0);
                }
                if rng.chance(0.3) {
                    raw.pop();
                }
                lib("parse_ns", vec![("hex", s(&hex(&raw)))])
            }
            4..=7 => {
                let mut name = rng.string(40);
                if rng.chance(0.1) {
                    name = name.repeat(10);
                }
                lib("escape", vec![("s", s(&name))])
            }
            8 => {
                let entries: Vec<Value> = (0..rng.index(4))
                    .map(|_| {
                        Value::Array(vec![
                            s(&rng.string(2)),
                            Value::Array((0..rng.index(3)).map(|_| s(&rng.string(12))).collect()),
                        ])
                    })
                    .collect();
                lib("render", vec![("entries", Value::Array(entries))])
            }
            _ => {
                let pool = ["a", "b", "-x", "--", "HEAD", "base"];
                let args: Vec<&str> = (0..rng.index(4)).map(|_| *rng.pick(&pool)).collect();
                let env = if rng.chance(0.5) {
                    obj(vec![("GITHUB_STEP_SUMMARY", s("s.md"))])
                } else {
                    no_env()
                };
                cli(tool, &args, no_env(), env, &["s.md"])
            }
        },
        Tool::StartupBudget => match rng.below(10) {
            0..=5 => {
                let pool = [
                    "reader", "skilled", "bare", "nope", "--root", ".", "-1", "--", "a b", "gamma",
                    "évil", "-h-", "-hX", "--he=x", "-h=x", "--=x", "-1\n", "--root=.", "--ro",
                    "-r=x", "-h-x", "-hh-", "-hh=x", "-hh", "-hh=",
                ];
                let mut args: Vec<&str> = vec!["--root", "."];
                for _ in 0..rng.index(4) {
                    args.push(*rng.pick(&pool));
                }
                if rng.chance(0.2) {
                    rng.shuffle(&mut args);
                }
                cli(tool, &args, agent_repo(rng, true), no_env(), &[])
            }
            6 | 7 => {
                let mut t = String::from("---\n");
                for _ in 0..rng.index(6) {
                    t.push_str(rng.pick(&[
                        "skills:\n",
                        "skills: \n",
                        "  - a:b\n",
                        "- c\n",
                        "  -  d  \n",
                        "\t- e f\n",
                        "name: x\n",
                        "skills:x\n",
                        "\u{3000}-\u{85}g\n",
                        "  - h\r\n",
                    ]));
                }
                t.push_str(rng.pick(&["---\n", "---", "\n---x\n", ""]));
                t.push_str(&rng.string(10));
                if rng.chance(0.1) {
                    t = t.replacen("---\n", "", 1);
                }
                lib("split", vec![("text", s(&t))])
            }
            _ => {
                let mut body = String::new();
                for _ in 0..rng.index(6) {
                    body.push_str(rng.pick(&[
                        "## A\n",
                        "## Startup\n",
                        "##  x \n",
                        "## \n",
                        "text\n",
                        "## A",
                        "\n",
                        "x## B\n",
                        "## Startup\u{2028}y\n",
                    ]));
                    if rng.chance(0.3) {
                        body.push_str(&rng.string(5));
                    }
                }
                lib("sections", vec![("body", s(&body))])
            }
        },
        Tool::EvalBaseline => {
            let names: Vec<String> = (0..rng.index(4)).map(|_| case_name(rng)).collect();
            let scores: Vec<(String, String)> =
                names.iter().map(|n| (n.clone(), score(rng))).collect();
            let pairs: Vec<(&str, &str)> = scores
                .iter()
                .map(|(a, b)| (a.as_str(), b.as_str()))
                .collect();
            let mut extra: Vec<(&str, &str)> = Vec::new();
            if rng.chance(0.2) {
                extra.push(("partial", "true"));
                extra.push((
                    "partialReason",
                    rng.pick(&[
                        "\"cost ceiling hit\"",
                        "null",
                        "\"\"",
                        "0",
                        "[1, \"x\\ny\"]",
                        "\"x\\n::error::boom\"",
                    ]),
                ));
            }
            if rng.chance(0.2) {
                extra.push((
                    "claudeVersion",
                    rng.pick(&[
                        "\"2.2.0\"",
                        "null",
                        "2.1",
                        "[1]",
                        "\"2.1.311\"",
                        "[NaN]",
                        "NaN",
                        "{\"a\": NaN}",
                    ]),
                ));
            }
            if rng.chance(0.1) {
                extra.push((
                    "startedAt",
                    rng.pick(&["\"2026\"", "5", "\"a\\u0007bTc\"", "\"T\""]),
                ));
            }
            let mut result = result_json(rng, &pairs, &extra);
            if rng.chance(0.05) {
                result = rng
                    .pick(&[
                        "{not json",
                        "[]",
                        "{\"cases\": 1}",
                        "\u{feff}{}",
                        "{\"cases\": [1]}",
                    ])
                    .to_string();
            }
            let base_names: Vec<String> = (0..rng.index(4)).map(|_| case_name(rng)).collect();
            let base_scores: Vec<(String, String)> =
                base_names.iter().map(|n| (n.clone(), score(rng))).collect();
            let base_pairs: Vec<(&str, &str)> = base_scores
                .iter()
                .map(|(a, b)| (a.as_str(), b.as_str()))
                .collect();
            let mut base = baseline_json(
                &base_pairs,
                rng.pick(&[
                    "\"2.1.311\"",
                    "\"2.2.0\"",
                    "null",
                    "2.1",
                    "[NaN]",
                    "NaN",
                    "{\"a\": NaN}",
                ]),
            );
            if rng.chance(0.05) {
                base = rng
                    .pick(&["{}", "[]", "{\"cases\": []}", "nope"])
                    .to_string();
            }
            let mut fs = vec![("result.json", result), ("dir/x", String::new())];
            if rng.chance(0.85) {
                fs.push(("baseline.json", base));
            }
            let sub = *rng.pick(&["compare", "compare", "compare", "record", "candidate"]);
            let result_path = if rng.chance(0.05) {
                "dir"
            } else {
                "result.json"
            };
            let base_path = if rng.chance(0.05) {
                "dir"
            } else {
                "baseline.json"
            };
            let mut args = vec![sub, result_path, base_path];
            // argparse corners: before the command, between and after the positionals.
            const ODD: &[&str] = &[
                "--", "-h-", "-hX", "-h=x", "--help=x", "-1", "--x", "-h-x", "-hh-", "-hh=x", "-hh",
            ];
            if rng.chance(0.05) {
                args.insert(0, *rng.pick(ODD));
            }
            if rng.chance(0.05) {
                let at = 1 + rng.index(args.len());
                args.insert(at, *rng.pick(ODD));
            }
            if sub == "compare" {
                for _ in 0..rng.index(3) {
                    let at = if rng.chance(0.2) { 2 } else { args.len() };
                    args.insert(
                        at.min(args.len()),
                        *rng.pick(&[
                            "--tolerance",
                            "0.1",
                            "--tol=0.5",
                            "--tolerance=-1",
                            "--tolerance= 1_0 ",
                            "--tolerance=nan",
                            "--allow-missing",
                            "--summary",
                            "s.md",
                            "--summary=",
                            "--x",
                            "--tolerance=\u{2000}1\u{2000}",
                            "--tolerance=1\x1c",
                            "--tolerance=-1\n",
                            "--",
                        ]),
                    );
                }
            }
            cli(
                tool,
                &args,
                files(&fs),
                no_env(),
                &["baseline.json", "s.md"],
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_seed_and_generated_case_runs_on_the_rust_side() {
        for tool in [
            Tool::VersionBump,
            Tool::ReleaseEdit,
            Tool::MemoryDiff,
            Tool::StartupBudget,
            Tool::EvalBaseline,
        ] {
            let surface = Dv2(tool);
            for c in surface.seeds() {
                let _ = surface.run_rust(&c);
            }
            let mut rng = Rng::new(7);
            for c in surface.generate(&mut rng, 200) {
                let _ = surface.run_rust(&c);
            }
        }
    }

    #[test]
    fn errno_text_is_masked() {
        assert_eq!(
            errno_text(b"x [Errno 2] No such file or directory: 'a'\n"),
            b"x [Errno 2] <LIBTEXT>: 'a'\n"
        );
    }
}
