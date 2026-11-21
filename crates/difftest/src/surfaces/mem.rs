//! The `mem` difftest surface (MI1): `anthology mem` against `memory_index.main`, oracle
//! `port/difftest/oracle/mem.py`.
//!
//! A case is `{"op": "cli", "data": D, "files": {rel: text}, "steps": [...]}`. Both sides make
//! a fresh folder `<ROOT>` with `home/` (HOME; the config folder is `home/.claude`), `data/`
//! and `plain/` (the working folder, not a git repository), write `files` under it, and name
//! the data folder by `D`: `state` (state.json's `data_dir`), `env` (`CLAUDE_PLUGIN_DATA`),
//! `none`, or `int` (`"data_dir": 5`, a Python TypeError). The environment is PATH, HOME,
//! `LANG=C.UTF-8` and `COLUMNS=80`. A step is `{"args": [...]}` (one run of the command),
//! `{"write": {rel: text}}` or `{"remove": [rel]}`.
//!
//! The result is one transcript on stdout: per run `$ <args>`, `exit N`, `--- stdout`, its
//! stdout, `--- stderr`, its stderr; then `=== db ===` and the database's content (D-5: the
//! SQLite file's bytes are not compared): the schema rows of the index's own tables, and every
//! row of `files` (without `mtime`), `nodes`, `refs`, `edges` and `notes_fts`, sorted.
//! `PRAGMA user_version` is left out (1 in Python, 2 here: F-11). The oracle marks D-1 where
//! Python raised (Rust prints `anthology-mem: <Type>: <message>`).
//!
//! The Linux oracle image has no git, and the working folder is never in a repository, so
//! every case uses personal memory; `save` is the "not inside a git work tree" refusal.

use std::fs;
use std::path::Path;

use anthology_core::Env;
use anthology_memory::index::{self, MemDeps};
use anthology_pycompat::json::{Map, Value};

use crate::case::{Case, Rng, Surface};
use crate::observe::{Observed, from_oracle};
use crate::oracle::Oracle;

/// The surface name (plan §1.4).
pub const NAME: &str = "mem";

/// Whether `difftest --all` runs this surface on macOS too (`ci/jobs/difftest-macos.sh`); set
/// `false` only for a surface that cannot run there, and say why.
pub const ON_MACOS: bool = true;

/// The registered implementation.
pub fn surface() -> Option<Box<dyn crate::case::Surface>> {
    Some(Box::new(Mem))
}

struct Mem;

impl Surface for Mem {
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
        from_oracle(&oracle.call("mem", &c.data))
    }

    fn run_rust(&self, c: &Case) -> Observed {
        run_rust(&c.data)
    }
}

// ---- the Rust side ---------------------------------------------------------------------------

fn field<'a>(v: &'a Value, k: &str) -> Option<&'a Value> {
    v.as_object().and_then(|m| m.get(k))
}

fn write_files(root: &Path, files: Option<&Value>) {
    let Some(m) = files.and_then(Value::as_object) else {
        return;
    };
    for (rel, text) in m.iter() {
        let path = root.join(rel);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).expect("mkdir");
        }
        fs::write(&path, text.as_str().unwrap_or("")).expect("write");
    }
}

fn cell(v: rusqlite::types::ValueRef<'_>) -> String {
    use rusqlite::types::ValueRef;
    match v {
        ValueRef::Null => "NULL".into(),
        ValueRef::Integer(n) => n.to_string(),
        ValueRef::Real(f) => f.to_string(),
        ValueRef::Text(t) | ValueRef::Blob(t) => String::from_utf8_lossy(t)
            .replace('\\', "\\\\")
            .replace('\n', "\\n")
            .replace('\t', "\\t"),
    }
}

const TABLES: [&str; 8] = [
    "files",
    "nodes",
    "refs",
    "edges",
    "notes_fts",
    "edges_src",
    "edges_dst",
    "nodes_name",
];

const DUMPS: [(&str, &str); 5] = [
    (
        "files",
        "SELECT path, size, sha256 FROM files ORDER BY path",
    ),
    (
        "nodes",
        "SELECT id, name, type, scope, file, description, updated FROM nodes ORDER BY id",
    ),
    (
        "refs",
        "SELECT src, target, kind FROM refs ORDER BY src, target, kind",
    ),
    (
        "edges",
        "SELECT src, dst, kind FROM edges ORDER BY src, dst, kind",
    ),
    (
        "fts",
        "SELECT id, name, description, body FROM notes_fts ORDER BY id",
    ),
];

fn dump_db(db: &Path, lines: &mut Vec<String>) -> rusqlite::Result<()> {
    let conn = rusqlite::Connection::open(db)?;
    let marks = vec!["?"; TABLES.len()].join(",");
    let mut stmt = conn.prepare(&format!(
        "SELECT name, sql FROM sqlite_master WHERE name IN ({marks}) ORDER BY name"
    ))?;
    let mut rows = stmt.query(rusqlite::params_from_iter(TABLES.iter()))?;
    while let Some(r) = rows.next()? {
        lines.push(format!(
            "schema {}\t{}",
            cell(r.get_ref(0)?),
            cell(r.get_ref(1)?)
        ));
    }
    for (label, sql) in DUMPS {
        let mut stmt = conn.prepare(sql)?;
        let n = stmt.column_count();
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let cells: Vec<String> = (0..n)
                .map(|i| r.get_ref(i).map(cell))
                .collect::<rusqlite::Result<_>>()?;
            lines.push(format!("{label}\t{}", cells.join("\t")));
        }
    }
    Ok(())
}

fn dump(data: &Path) -> String {
    let mut dbs: Vec<_> = fs::read_dir(data.join("memory-index"))
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "db"))
                .collect()
        })
        .unwrap_or_default();
    if dbs.is_empty() {
        return "<no db>\n".into();
    }
    dbs.sort();
    let mut lines = Vec::new();
    for db in dbs {
        lines.push(format!(
            "db {}",
            db.file_name().unwrap_or_default().to_string_lossy()
        ));
        if let Err(e) = dump_db(&db, &mut lines) {
            lines.push(format!("sqlite error: {e}"));
        }
    }
    format!("{}\n", lines.join("\n"))
}

fn run_rust(case: &Value) -> Observed {
    let dir = tempfile::Builder::new()
        .prefix("mi1-rust-")
        .tempdir()
        .expect("tempdir");
    let tmp = fs::canonicalize(dir.path()).expect("realpath");
    let (home, data, plain) = (tmp.join("home"), tmp.join("data"), tmp.join("plain"));
    for d in [&home, &data, &plain] {
        fs::create_dir(d).expect("mkdir");
    }
    let anth = home.join(".claude/anthology");
    fs::create_dir_all(&anth).expect("mkdir");
    let where_ = field(case, "data")
        .and_then(Value::as_str)
        .unwrap_or("state");
    match where_ {
        "state" => fs::write(
            anth.join("state.json"),
            format!("{{\"data_dir\": \"{}\"}}", data.display()),
        )
        .expect("state"),
        "int" => fs::write(anth.join("state.json"), "{\"data_dir\": 5}").expect("state"),
        _ => {}
    }
    write_files(&tmp, field(case, "files"));
    let path = std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into());
    let mut env = Env::default()
        .with_os("PATH", path)
        .with_os("HOME", &home)
        .with("LANG", "C.UTF-8")
        .with("COLUMNS", "80");
    if where_ == "env" {
        env = env.with_os("CLAUDE_PLUGIN_DATA", &data);
    }
    let deps = MemDeps {
        cwd: Some(plain.clone()),
        ..MemDeps::default()
    };
    let mut transcript = String::new();
    let steps = field(case, "steps")
        .and_then(Value::as_array)
        .map(|a| a.as_slice())
        .unwrap_or(&[]);
    for step in steps {
        if let Some(files) = field(step, "write") {
            write_files(&tmp, Some(files));
            continue;
        }
        if let Some(gone) = field(step, "remove").and_then(Value::as_array) {
            for rel in gone {
                let _ = fs::remove_file(tmp.join(rel.as_str().unwrap_or("")));
            }
            continue;
        }
        let args: Vec<String> = field(step, "args")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let argv: Vec<std::ffi::OsString> = args.iter().map(Into::into).collect();
        let out = index::main(&argv, &env, &deps);
        transcript.push_str(&format!(
            "$ {}\nexit {}\n--- stdout\n{}--- stderr\n{}",
            args.join(" "),
            out.code,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    transcript.push_str("=== db ===\n");
    transcript.push_str(&dump(&data));
    let text = transcript.replace(&tmp.display().to_string(), "<ROOT>");
    Observed::text(0, &text, "")
}

// ---- cases -----------------------------------------------------------------------------------

fn s(v: &str) -> Value {
    Value::Str(v.to_string())
}

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    let mut m = Map::default();
    for (k, v) in pairs {
        m.insert(k.to_string(), v);
    }
    Value::Object(m)
}

fn args(a: &[&str]) -> Value {
    obj(vec![(
        "args",
        Value::Array(a.iter().map(|x| s(x)).collect()),
    )])
}

fn files(pairs: &[(&str, &str)]) -> Value {
    obj(pairs.iter().map(|(k, v)| (*k, s(v))).collect())
}

const P: &str = "home/.claude/anthology/personal-memory/";

fn personal(name: &str) -> String {
    format!("{P}{name}")
}

fn note(name: &str, ty: &str, desc: &str, updated: &str, body: &str) -> String {
    let mut t = format!("---\nname: {name}\ndescription: {desc}\nmetadata:\n  type: {ty}\n");
    if !updated.is_empty() {
        t.push_str(&format!("  modified: {updated}\n"));
    }
    t.push_str(&format!("---\n\n{body}"));
    t
}

fn case(data: &str, fs_: Value, steps: Vec<Value>) -> Value {
    obj(vec![
        ("op", s("cli")),
        ("data", s(data)),
        ("files", fs_),
        ("steps", Value::Array(steps)),
    ])
}

fn seeds() -> Vec<Value> {
    let me = personal("user-me.md");
    let pref = personal("user-pref.md");
    let base_notes = [
        (
            me.as_str(),
            note(
                "user-me",
                "user",
                "About me",
                "",
                "Likes the quokka. See [[user-pref]].\n",
            ),
        ),
        (
            pref.as_str(),
            note("user-pref", "user", "Prefs", "2019-01-01", "Terse.\n"),
        ),
    ];
    let base = || {
        let v: Vec<(&str, &str)> = base_notes.iter().map(|(k, v)| (*k, v.as_str())).collect();
        files(&v)
    };
    let graph = {
        let mut v = Vec::new();
        let texts = [
            ("a.md", note("a", "project", "A note", "", "Links to [[b]]. Contains the word zebra.\n")),
            ("b.md", "---\nname: b\ndescription: A note\nmetadata:\n  type: project\n  relations:\n    depends_on: [c]\n---\n\nPlain b.\n".to_string()),
            ("c.md", "---\nname: c\ndescription: A note\nmetadata:\n  type: project\n  relations:\n    relates_to: [d]\n---\n\nPlain c.\n".to_string()),
            ("d.md", note("d", "project", "A note", "", "Plain d, a zebra crossing.\n")),
            ("e.md", note("e", "project", "A note", "", "Nobody links here.\n")),
            ("old-rule.md", note("old-rule", "project", "Old", "2020-05-01", "Old.\n")),
            ("new-rule.md", "---\nname: new-rule\ndescription: New\nmetadata:\n  type: project\n  modified: 2020-05-02\n  relations:\n    supersedes: [old-rule]\n---\n\nNew.\n".to_string()),
            ("MEMORY.md", "- [a](a.md)\n".to_string()),
            ("notes.txt", "not a note".to_string()),
        ];
        for (f, t) in texts {
            v.push((personal(f), t));
        }
        v
    };
    let graph_files = || {
        let v: Vec<(&str, &str)> = graph
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        files(&v)
    };
    let all_queries = vec![
        args(&["search", "zebra"]),
        args(&["search", "ZEBRA", "crossing", "--json"]),
        args(&["related", "a"]),
        args(&["related", "a", "--depth", "3", "--json"]),
        args(&["related", "c", "--depth", "1"]),
        args(&["orphans"]),
        args(&["orphans", "--json"]),
        args(&["stale"]),
        args(&["stale", "--days", "10", "--json"]),
        args(&["rebuild"]),
        args(&["rebuild", "--json"]),
    ];
    vec![
        case(
            "state",
            base(),
            vec![
                args(&["search", "quokka", "--json"]),
                args(&["search", "quokka"]),
                args(&["related", "user-me", "--depth", "1", "--json"]),
                args(&["orphans", "--json"]),
                args(&["stale", "--json"]),
                args(&["rebuild"]),
            ],
        ),
        case("state", graph_files(), all_queries.clone()),
        case("env", graph_files(), all_queries),
        case("none", base(), vec![args(&["search", "quokka"])]),
        case("int", base(), vec![args(&["orphans"])]),
        case(
            "state",
            base(),
            vec![
                args(&["search", "quokka"]),
                obj(vec![(
                    "write",
                    files(&[(
                        &personal("user-new.md"),
                        &note("user-new", "user", "New", "", "A wombat.\n"),
                    )]),
                )]),
                args(&["search", "wombat", "--json"]),
                obj(vec![(
                    "remove",
                    Value::Array(vec![s(&personal("user-me.md"))]),
                )]),
                args(&["orphans"]),
                args(&["rebuild", "--json"]),
            ],
        ),
        case(
            "state",
            base(),
            vec![
                args(&[]),
                args(&["-h"]),
                args(&["search"]),
                args(&["search", "-h"]),
                args(&["frobnicate"]),
                args(&["related"]),
                args(&["related", "x", "--depth", "x"]),
                args(&["related", "x", "--dep=1"]),
                args(&["stale", "--days", "-5"]),
                args(&["stale", "--days", "1000000000"]),
                args(&["stale", "--days", "99999999999"]),
                args(&["stale", "--days", "800000"]),
                args(&["save", "--agent", "scout"]),
                args(&["save", "-h"]),
                args(&[
                    "save",
                    "--agent",
                    "scout",
                    "--type",
                    "project",
                    "--slug",
                    "x",
                    "--name",
                    "n",
                    "--description",
                    "d",
                    "--body",
                    "b",
                ]),
                args(&[
                    "save",
                    "--agent",
                    "scout",
                    "--type",
                    "project",
                    "--slug",
                    "x",
                    "--name",
                    "n",
                    "--description",
                    "d",
                    "--body",
                    "b",
                    "--rep",
                ]),
            ],
        ),
        case(
            "state",
            base(),
            vec![
                args(&["search", "\""]),
                args(&["search", "\"unbalanced"]),
                args(&["search", "zebra", "AND"]),
                args(&["search", "NEAR("]),
                args(&["search", "a*"]),
                args(&["search", "--", "-quokka"]),
                args(&["search", "col:quokka"]),
                args(&["search", "'; DROP TABLE nodes; --"]),
                args(&["search", "{x}"]),
                args(&["search", "100%", "a_b"]),
            ],
        ),
        // A file in the database's place that is not a database.
        case(
            "state",
            obj(vec![
                (me.as_str(), s(&base_notes[0].1)),
                (
                    "data/memory-index/personal-4a0a339b0c6d.db",
                    s(&"this is not a database at all".repeat(50)),
                ),
            ]),
            vec![args(&["orphans", "--json"])],
        ),
        // Frontmatter shapes: none, CRLF, a BOM, broken, folded, quoted, block lists, top level.
        case(
            "state",
            files(&[
                (&personal("plain.md"), "just text with [[quoted]]\n"),
                (&personal("crlf.md"), "---\r\nname: crlf\r\n---\r\nbody\r\n"),
                (
                    &personal("bom.md"),
                    "\u{feff}---\nname: bom\n---\nbody [[crlf]]\n",
                ),
                (&personal("broken.md"), "---\nname: [unclosed\n  : :\n"),
                (
                    &personal("folded.md"),
                    "---\nname: n\ndescription: >\n  first line\n  second line\n---\nb\n",
                ),
                (
                    &personal("quoted.md"),
                    "---\nname: \"Quoted Name\"\ndescription: 'single'\nmetadata:\n  type: \"reference\"\n  modified: \"2026-01-02\"\n  relations:\n    supersedes:\n      - old-a\n      - \"plain\"\n    relates_to: [z, bom]\n---\nbody\n",
                ),
                (
                    &personal("top.md"),
                    "---\ndescription: d\ntype: user\nupdated: 20190505\n---\nbody [[ghost]] [[ghost|x]] [[crlf#h]]\n",
                ),
                (&personal(".md"), "dot\n"),
                (
                    &personal("week.md"),
                    &note("week", "x", "w", "2019-W05-3", ""),
                ),
            ]),
            vec![
                args(&["orphans"]),
                args(&["related", "top", "--depth", "2"]),
                args(&["related", "quoted"]),
                args(&["stale"]),
                args(&["search", "body"]),
            ],
        ),
    ]
}

const NAMES: &[&str] = &[
    "alpha",
    "beta",
    "gamma",
    "delta",
    "Ünïcode",
    "two words",
    "x",
    "user-me",
];
const TYPES: &[&str] = &[
    "user",
    "feedback",
    "project",
    "reference",
    "other",
    "User # me",
    "",
];
const DATES: &[&str] = &[
    "",
    "2019-01-01",
    "2018-12-31T10:00",
    "20180101",
    "2018-W05-3",
    "garbage",
    "2026-13-01",
    "2099-01-01",
];
const WORDS: &[&str] = &[
    "zebra",
    "crossing",
    "crossings",
    "quokka",
    "wombat",
    "café",
    "naïve",
    "the",
    "AND",
    "OR",
    "NEAR",
    "\"",
    "a*",
    "-x",
    "col:x",
    "日本",
];
const KINDS: &[&str] = &["supersedes", "depends_on", "relates_to", "likes"];

fn gen_note(rng: &mut Rng) -> String {
    let name = *rng.pick(NAMES);
    let mut body = String::new();
    for _ in 0..rng.below(8) {
        if rng.chance(0.3) {
            let t = *rng.pick(NAMES);
            let link = match rng.below(3) {
                0 => format!("[[{t}]]"),
                1 => format!("[[{t}|alias]]"),
                _ => format!("[[{t}#part]]"),
            };
            body.push_str(&link);
        } else {
            body.push_str(rng.pick(WORDS));
        }
        body.push(if rng.chance(0.2) { '\n' } else { ' ' });
    }
    if rng.chance(0.15) {
        return body;
    }
    let mut t = String::from("---\n");
    if rng.chance(0.9) {
        t.push_str(&format!("name: {name}\n"));
    }
    t.push_str(&format!(
        "description: {} {}\n",
        rng.pick(WORDS),
        rng.pick(WORDS)
    ));
    t.push_str("metadata:\n");
    t.push_str(&format!("  type: {}\n", rng.pick(TYPES)));
    let date = *rng.pick(DATES);
    if !date.is_empty() {
        t.push_str(&format!("  modified: {date}\n"));
    }
    if rng.chance(0.5) {
        t.push_str("  relations:\n");
        for _ in 0..=rng.below(2) {
            let targets: Vec<&str> = (0..=rng.below(2)).map(|_| *rng.pick(NAMES)).collect();
            t.push_str(&format!(
                "    {}: [{}]\n",
                rng.pick(KINDS),
                targets.join(", ")
            ));
        }
    }
    t.push_str("---\n\n");
    t.push_str(&body);
    t
}

fn gen_files(rng: &mut Rng) -> Vec<(String, String)> {
    (0..rng.below(7))
        .map(|i| {
            let stem = if rng.chance(0.5) {
                rng.pick(NAMES).replace(' ', "-")
            } else {
                format!("n{i}")
            };
            (personal(&format!("{stem}.md")), gen_note(rng))
        })
        .collect()
}

fn gen_args(rng: &mut Rng) -> Vec<String> {
    let mut a: Vec<String> = match rng.below(6) {
        0 | 1 => {
            let mut v = vec!["search".to_string()];
            for _ in 0..=rng.below(2) {
                v.push(rng.pick(WORDS).to_string());
            }
            if v.iter().any(|x| x.starts_with('-')) {
                v.insert(1, "--".into());
            }
            v
        }
        2 => vec![
            "related".into(),
            rng.pick(NAMES).to_string(),
            "--depth".into(),
            rng.range(-1, 4).to_string(),
        ],
        3 => vec!["orphans".into()],
        4 => vec![
            "stale".into(),
            "--days".into(),
            rng.pick(&["-5", "0", "10", "180", "3000"]).to_string(),
        ],
        _ => vec!["rebuild".into()],
    };
    if rng.chance(0.5) {
        a.push("--json".into());
    }
    a
}

fn generate(rng: &mut Rng) -> Value {
    let init = gen_files(rng);
    let fs_: Vec<(&str, &str)> = init.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let mut steps = Vec::new();
    for _ in 0..=rng.below(3) {
        if rng.chance(0.3) {
            let more = gen_files(rng);
            let m: Vec<(&str, &str)> = more.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            steps.push(obj(vec![("write", files(&m))]));
        }
        if rng.chance(0.15) && !init.is_empty() {
            let gone = &init[rng.index(init.len())].0;
            steps.push(obj(vec![("remove", Value::Array(vec![s(gone)]))]));
        }
        let a = gen_args(rng);
        steps.push(obj(vec![(
            "args",
            Value::Array(a.iter().map(|x| s(x)).collect()),
        )]));
    }
    let data = *rng.pick(&["state", "state", "state", "env"]);
    case(data, files(&fs_), steps)
}
