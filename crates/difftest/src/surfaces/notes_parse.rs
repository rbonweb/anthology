//! The `notes.parse` difftest surface (MS1; oracle `port/difftest/oracle/notes.py`).
//!
//! Cases (the oracle's docstring has the exact output shapes):
//! - `{"op": "parse", "text", "stem"}`: `memory_index.parse_text` against
//!   `anthology_memory::notes::parse_text_ok`, with `is_user_note` and `normalize_type`;
//! - `{"op": "note_info", "name", "data_hex"}`: `memory_sync.note_info` on bytes that need not
//!   be UTF-8;
//! - `{"op": "render", "notes", "links", "budget", "suffix", "exclude"}`: `render_index` and
//!   `regenerate_index` (twice) over notes written into a temporary folder;
//! - `{"op": "remote", "url", "base", "home"}`: `normalize_remote`.
//!
//! Seeds are the inputs of `tests/test_memory_sync.py` (index and remote cases) and the
//! parser's corners. Generated cases splice frontmatter fragments (keys, quoting, flow and
//! block scalars, list items, comments, indents of spaces, tabs and Unicode spaces, CRLF,
//! a BOM, closing-line variants, nesting) with random text; remote URLs combine schemes,
//! credentials, ports, scp forms, local paths and `~` with Unicode whitespace and case.

use std::fs;
use std::path::Path;

use anthology_core::Env;
use anthology_memory::notes::{
    NoteInfo, decode_replace, is_user_note, normalize_type, note_info, parse_text_ok,
};
use anthology_memory::render::{Budget, regenerate_index, render_index};
use anthology_memory::repo::normalize_remote;
use anthology_pycompat::json::{self, Int, Map, Value};
use anthology_pycompat::str::{char_len, char_slice};

use crate::case::{Case, Rng, Surface};
use crate::observe::{Observed, from_oracle};
use crate::oracle::Oracle;

/// The surface name (plan §1.4).
pub const NAME: &str = "notes.parse";

/// Whether `difftest --all` runs this surface on macOS too: a library surface (the render and
/// remote cases use a temporary folder and the file system's realpath, the same on both
/// sides), so yes.
pub const ON_MACOS: bool = true;

/// The registered implementation.
pub fn surface() -> Option<Box<dyn Surface>> {
    Some(Box::new(NotesParse))
}

struct NotesParse;

impl Surface for NotesParse {
    fn name(&self) -> &'static str {
        NAME
    }

    fn seeds(&self) -> Vec<Case> {
        let mut out = Vec::new();
        for (i, text) in seed_texts().into_iter().enumerate() {
            out.push(Case::new(
                format!("seed:parse:{}", i + 1),
                parse_case(&text, "stem"),
            ));
        }
        out.push(Case::new("seed:deep", parse_case(&deep_text(1200), "deep")));
        out.push(Case::new(
            "seed:nested",
            parse_case(&deep_text(60), "nested"),
        ));
        for (i, (name, data)) in seed_bytes().into_iter().enumerate() {
            out.push(Case::new(
                format!("seed:info:{}", i + 1),
                info_case(name, &data),
            ));
        }
        for (i, c) in seed_renders().into_iter().enumerate() {
            out.push(Case::new(format!("seed:render:{}", i + 1), c));
        }
        for (i, url) in seed_urls().into_iter().enumerate() {
            out.push(Case::new(
                format!("seed:remote:{}", i + 1),
                remote_case(url, "/nonexistent-ms1/base", "/nonexistent-ms1/home/"),
            ));
        }
        out
    }

    fn generate(&self, rng: &mut Rng, n: usize) -> Vec<Case> {
        (0..n)
            .map(|i| {
                let data = match rng.below(10) {
                    0..=4 => parse_case(&gen_note(rng), &gen_stem(rng)),
                    5 => {
                        let name = gen_stem(rng) + rng.pick(&[".md", "", "md", ".x"]);
                        info_case(&name, &gen_bytes(rng))
                    }
                    6..=7 => gen_render(rng),
                    _ => gen_remote(rng),
                };
                Case::new(format!("gen:{}", i + 1), data)
            })
            .collect()
    }

    fn run_python(&self, c: &Case, oracle: &mut Oracle) -> Observed {
        from_oracle(&oracle.call("notes", &c.data))
    }

    fn run_rust(&self, c: &Case) -> Observed {
        run(&c.data)
    }
}

// ------------------------------------------------------------------------------------------
// Running a case
// ------------------------------------------------------------------------------------------

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    let mut m = Map::new();
    for (k, v) in pairs {
        m.insert(k.into(), v);
    }
    Value::Object(m)
}

fn s(v: &str) -> Value {
    Value::Str(v.into())
}

fn strs(v: &[String]) -> Value {
    Value::Array(v.iter().map(|x| s(x)).collect())
}

fn parse_case(text: &str, stem: &str) -> Value {
    obj(vec![
        ("op", s("parse")),
        ("text", s(text)),
        ("stem", s(stem)),
    ])
}

fn info_case(name: &str, data: &[u8]) -> Value {
    let hex: String = data.iter().map(|b| format!("{b:02x}")).collect();
    obj(vec![
        ("op", s("note_info")),
        ("name", s(name)),
        ("data_hex", s(&hex)),
    ])
}

fn remote_case(url: &str, base: &str, home: &str) -> Value {
    obj(vec![
        ("op", s("remote")),
        ("url", s(url)),
        ("base", s(base)),
        ("home", s(home)),
    ])
}

fn render_case(
    notes: &[(String, String)],
    links: &[(String, String)],
    budget: (usize, usize),
    suffix: &str,
    exclude: &[&str],
) -> Value {
    let pairs = |v: &[(String, String)]| {
        Value::Array(
            v.iter()
                .map(|(a, b)| Value::Array(vec![s(a), s(b)]))
                .collect(),
        )
    };
    let int = |n: usize| Value::Int(Int::Small(i64::try_from(n).expect("small")));
    obj(vec![
        ("op", s("render")),
        ("notes", pairs(notes)),
        ("links", pairs(links)),
        ("budget", Value::Array(vec![int(budget.0), int(budget.1)])),
        ("suffix", s(suffix)),
        (
            "exclude",
            Value::Array(exclude.iter().map(|e| s(e)).collect()),
        ),
    ])
}

fn field<'a>(v: &'a Value, k: &str) -> &'a Value {
    v.as_object()
        .and_then(|o| o.get(k))
        .unwrap_or_else(|| panic!("notes.parse: no {k:?}"))
}

fn text_field<'a>(v: &'a Value, k: &str) -> &'a str {
    field(v, k)
        .as_str()
        .unwrap_or_else(|| panic!("notes.parse: {k:?} is not text"))
}

fn py_bool(b: bool) -> &'static str {
    if b { "True" } else { "False" }
}

/// `json.dumps(_fields(info))` (the oracle's).
fn fields(info: &NoteInfo, ok: bool) -> String {
    let rel = Value::Array(vec![
        strs(&info.relations.supersedes),
        strs(&info.relations.depends_on),
        strs(&info.relations.relates_to),
    ]);
    let v = Value::Array(vec![
        s(&info.name),
        s(&info.description),
        s(info.type_str()),
        s(info.updated_str()),
        rel,
        s(&info.body),
        Value::Bool(ok),
        Value::Bool(is_user_note(info)),
        s(&normalize_type(Some(info.type_str()))),
    ]);
    json::dumps(&v, &json::PY_DEFAULT) + "\n"
}

fn run(case: &Value) -> Observed {
    let out = match text_field(case, "op") {
        "parse" => {
            let (info, ok) = parse_text_ok(text_field(case, "text"), text_field(case, "stem"));
            fields(&info, ok)
        }
        "note_info" => {
            let name = text_field(case, "name");
            let data = unhex(text_field(case, "data_hex"));
            let info = note_info(name, &data);
            let stem = char_slice(name, 0, char_len(name).saturating_sub(3));
            let (again, ok) = parse_text_ok(&decode_replace(&data), stem);
            assert_eq!(info, again, "note_info is parse_text of the decoded bytes");
            fields(&info, ok)
        }
        "render" => run_render(case),
        "remote" => {
            let env = Env::default().with("HOME", text_field(case, "home"));
            let base = Path::new(text_field(case, "base"));
            normalize_remote(text_field(case, "url"), Some(base), &env) + "\n"
        }
        other => panic!("notes.parse: unknown op {other:?}"),
    };
    Observed::text(0, &out, "")
}

fn unhex(h: &str) -> Vec<u8> {
    (0..h.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&h[i..i + 2], 16).expect("hex"))
        .collect()
}

fn pairs_of(v: &Value) -> Vec<(String, String)> {
    v.as_array()
        .expect("notes.parse: a list of pairs")
        .iter()
        .map(|p| {
            let p = p.as_array().expect("a pair");
            (
                p[0].as_str().expect("text").to_string(),
                p[1].as_str().expect("text").to_string(),
            )
        })
        .collect()
}

fn run_render(case: &Value) -> String {
    let dir = tempfile::Builder::new()
        .prefix("notes-rust-")
        .tempdir()
        .expect("notes.parse: tempdir");
    let d = dir.path();
    for (name, text) in pairs_of(field(case, "notes")) {
        fs::write(d.join(name), text).expect("notes.parse: write");
    }
    for (name, target) in pairs_of(field(case, "links")) {
        std::os::unix::fs::symlink(target, d.join(name)).expect("notes.parse: symlink");
    }
    let budget = field(case, "budget").as_array().expect("budget");
    let num = |v: &Value| match v {
        Value::Int(Int::Small(n)) => usize::try_from(*n).expect("a size"),
        other => panic!("notes.parse: not a size: {other:?}"),
    };
    let budget = Budget {
        lines: num(&budget[0]),
        bytes: num(&budget[1]),
    };
    let exclude: Vec<&str> = field(case, "exclude")
        .as_array()
        .expect("exclude")
        .iter()
        .map(|e| e.as_str().expect("text"))
        .collect();
    let suffix = text_field(case, "suffix");
    let text = render_index(d, budget, suffix, &exclude).expect("notes.parse: render_index");
    let mut out = text.clone().unwrap_or_else(|| "<None>\n".to_string());
    let first = regenerate_index(d, budget, &exclude).expect("notes.parse: regenerate");
    let second = regenerate_index(d, budget, &exclude).expect("notes.parse: regenerate");
    out.push_str(&format!("regen {} {}\n", py_bool(first), py_bool(second)));
    let index = d.join("MEMORY.md");
    let same = text.is_some_and(|t| {
        fs::symlink_metadata(&index).is_ok_and(|m| m.file_type().is_file())
            && fs::read(&index).is_ok_and(|b| b == t.as_bytes())
    });
    out.push_str(&format!("index {}\n", py_bool(same)));
    out
}

// ------------------------------------------------------------------------------------------
// Seeds
// ------------------------------------------------------------------------------------------

fn seed_texts() -> Vec<String> {
    let mut v: Vec<String> = [
        "plain\n",
        "",
        "---\nname: x\n",
        "--- \nname: x\n---\n",
        "\u{feff}---\r\nname: n\r\n---\r\n\r\nbody\r\n",
        "x\ry",
        "---\nname: n\n--- \t\n\n\nbody",
        "---\n---\nb",
        "---\nname: n\n----\n",
        "---\nname: '  it''s  '\ndescription: \"a\\nb \\\"q\\\" c\\\\d\"\nmetadata:\n  type: \"'\"\n---\n",
        "---\nname: 'x\"\n---\n",
        "---\ntype: top\nupdated: u1\nmodified: m1\nmetadata:\n  updated: u2\n---\n",
        "---\nmetadata:\n  type: ' '\ntype: t\n---\n",
        "---\ndescription: >-\n  one\n    two\n\n  three\nname: n\nmetadata:\n  relations:\n    depends_on:\n    - x\n    -  'y'\n    - \n  type: t\n---\n",
        "---\nname: >x\ndescription: [a, b\n---\n",
        "---\n- a\n- b\n---\n",
        "---\nmetadata:\n  relations:\n    supersedes: [a]\n    ' supersedes ': []\n    relates_to: one\n    depends_on: [b]\n    \"depends_on\": [c, ' ', d]\n    other: [z]\n---\n",
        "---\n# a comment\n\n   \nname: n\n  stray: deeper\n- item\nname2\n\tdescription: tab\n---\n",
        "---\nname: [unclosed\n  : :\n",
        "---\nname: a\ndescription: \"first\\nsecond   spaced\"\n---\nx\n",
        "---\nmetadata:\n  type: user # me\n---\n",
        "---\nmetadata:\n  type: USER\n---\n",
        "---\nmetadata:\n- a\n  type: user\n---\n",
        "---\nmetadata:\n  relations:\n    supersedes:\n      - nested\n---\n",
        "---\nname:\n- a\n---\n",
        "---\nname: |\n---\n",
        "---\nname: |2+\n  x\n  y\n---\n",
        "---\nname: \u{a0}nb\u{a0}\n\u{2028}type: z\n---\n",
        "---\nname: 'É'\nmetadata:\n  type: Σ\n---\n",
        "---\n \u{3000}name: x\n---\n",
        "---\n  name: indented\n  type: t\nname: top\n---\n",
        "---\r\nname: a\r\n---\r\nb",
        "---\nname: a\n---",
        "---\nname: a\n---\n---\nname: b\n---\n",
    ]
    .iter()
    .map(|t| t.to_string())
    .collect();
    for (name, type_, desc, updated) in [
        ("project-new", "project", "New thing", Some("2026-03-01")),
        ("Pretty Name", "feedback", "d", Some("2026-05-05")),
        ("user-me", "user", "About me", None),
    ] {
        let mut t = format!("---\nname: {name}\ndescription: {desc}\nmetadata:\n  type: {type_}\n");
        if let Some(u) = updated {
            t.push_str(&format!("  modified: {u}\n"));
        }
        t.push_str("  relations:\n    supersedes: [a, b]\n    relates_to: [c]\n---\n\nBody.\n");
        v.push(t);
    }
    v
}

/// A frontmatter nested `depth` levels deep under `metadata`.
fn deep_text(depth: usize) -> String {
    let mut t = String::from("---\nname: deep\nmetadata:\n");
    for d in 1..depth {
        t.push_str(&" ".repeat(d));
        t.push_str("k:\n");
    }
    t.push_str(&" ".repeat(depth));
    t.push_str("type: user\n---\nbody\n");
    t
}

fn seed_bytes() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("n.md", b"---\nname: a\xffb\n---\nbody \xe2\x82\n".to_vec()),
        ("x.md", b"no frontmatter".to_vec()),
        ("ab", b"x".to_vec()),
        (
            "s.md",
            b"\xed\xa0\x80 \xc0\x80 \xf0\x80\x80 \xf4\x90\x80\x80 \xe2\x82\xac \xf0\x9f\x98"
                .to_vec(),
        ),
        ("bom.md", b"\xef\xbb\xbf---\nname: b\n---\n".to_vec()),
    ]
}

fn note_text(name: &str, type_: &str, desc: &str, updated: Option<&str>) -> String {
    let mut t = format!("---\nname: {name}\ndescription: {desc}\nmetadata:\n  type: {type_}\n");
    if let Some(u) = updated {
        t.push_str(&format!("  modified: {u}\n"));
    }
    t.push_str("---\n\nBody.\n");
    t
}

fn seed_renders() -> Vec<Value> {
    let n = |f: &str, name: &str, t: &str, d: &str, u: Option<&str>| {
        (f.to_string(), note_text(name, t, d, u))
    };
    let mut out = Vec::new();
    let groups = vec![
        n(
            "project-old.md",
            "project-old",
            "project",
            "Old thing",
            Some("2026-01-01"),
        ),
        n(
            "project-new.md",
            "project-new",
            "project",
            "New thing",
            Some("2026-03-01"),
        ),
        n(
            "user-me.md",
            "user-me",
            "user",
            "About me",
            Some("2026-02-01"),
        ),
        n(
            "feedback-x.md",
            "feedback-x",
            "feedback",
            "Do x",
            Some("2026-02-02"),
        ),
        n("ref-a.md", "ref-a", "reference", "Where a", None),
        n("zed.md", "zed", "zeta", "Custom type", None),
        n("alpha.md", "alpha", "aardvark", "Other custom", None),
    ];
    out.push(render_case(&groups, &[], (150, 20000), "", &[]));
    out.push(render_case(&groups, &[], (150, 20000), "", &["user"]));
    out.push(render_case(
        &groups,
        &[],
        (4, 20000),
        " (full index: x)",
        &["USER # x"],
    ));
    let only_user = vec![n("u.md", "u", "User", "me", None)];
    out.push(render_case(&only_user, &[], (150, 20000), "", &["user"]));
    out.push(render_case(&[], &[], (150, 20000), "", &[]));
    let mut three = Vec::new();
    for i in 0..300 {
        let t = ["feedback", "project", "reference"][i % 3];
        let date = format!("2026-{:02}-{:02}", (i / 28) % 12 + 1, i % 28 + 1);
        three.push(n(
            &format!("{t}-{i:03}.md"),
            &format!("{t}-{i:03}"),
            t,
            &format!("Description number {i}"),
            Some(&date),
        ));
    }
    out.push(render_case(&three, &[], (150, 20000), "", &[]));
    out.push(render_case(
        &three,
        &[],
        (40, 2500),
        " (full index: /x/MEMORY.md)",
        &[],
    ));
    let mut sixty = Vec::new();
    for t in 0..60 {
        for k in 0..2 {
            sixty.push(n(
                &format!("t{t:02}-{k}.md"),
                &format!("t{t:02}-{k}"),
                &format!("type{t:02}"),
                "d",
                Some(&format!("2026-01-{:02}", k + 1)),
            ));
        }
    }
    out.push(render_case(&sixty, &[], (30, 20000), "", &[]));
    out.push(render_case(&sixty, &[], (150, 600), "", &[]));
    out.push(render_case(&sixty, &[], (1, 10), "", &[]));
    let odd = vec![
        (
            "a.md".to_string(),
            "---\nname: a\ndescription: \"first\\nsecond   spaced\"\n---\nx\n".to_string(),
        ),
        (
            "b.md".to_string(),
            note_text("b", "project", &"w".repeat(500), None),
        ),
        (
            "broken.md".to_string(),
            "---\nname: [unclosed\n  : :\n".to_string(),
        ),
        ("MEMORY.md".to_string(), "stale\n".to_string()),
        ("notes.txt".to_string(), "x".to_string()),
        (
            "sp ace%[x].md".to_string(),
            note_text("sp", "project", "", None),
        ),
        (
            "é ü.md".to_string(),
            note_text("Ünï", "project", "d\u{2028}e", None),
        ),
    ];
    let links = vec![
        ("link.md".to_string(), "a.md".to_string()),
        ("dangling.md".to_string(), "missing.md".to_string()),
    ];
    out.push(render_case(&odd, &links, (150, 20000), "", &[]));
    out
}

fn seed_urls() -> Vec<&'static str> {
    vec![
        "git@github.com:Acme/Repo.git",
        "https://github.com/acme/repo",
        "ssh://git@github.com/acme/repo.git/",
        "https://user:tok@github.com/acme/repo",
        "http://github.com/ACME/REPO.git",
        "git://github.com/acme/repo",
        "ssh://git@github.com:22/acme/repo.git",
        "https://github.com:443/acme/repo/",
        "github.com:acme/repo",
        "https://me:s3cret-token@git.example.invalid/a/b.git",
        "ssh://git@host.example.invalid:2222/a/b.git",
        "/srv/x.git",
        "file:///srv/x.git",
        "FILE:///srv/x.git",
        "git@host.example.invalid:/srv/x.git",
        "./rel/x.git",
        "../up/x",
        "~",
        "~/x.git",
        "~bob/x",
        "plain",
        "  https://h/a//b///c.git//  ",
        "https://[::1]:80/a",
        "https://h:1:2/a",
        "https://h:/a",
        "host:a\nb",
        "u@h@x:y",
        "/",
        "x.git",
        "https://h",
        "",
    ]
}

// ------------------------------------------------------------------------------------------
// Generation
// ------------------------------------------------------------------------------------------

const KEYS: &[&str] = &[
    "name",
    "description",
    "type",
    "updated",
    "modified",
    "metadata",
    "relations",
    "supersedes",
    "depends_on",
    "relates_to",
    " name",
    "'name'",
    "\"type\"",
    "'supersedes '",
    "x",
    "",
    "#c",
    "- item",
    "-",
    "name2",
    "Type",
];

const VALUES: &[&str] = &[
    "",
    "plain",
    " spaced  value ",
    "'single ''q'' '",
    "\"dq \\n \\\" \\\\ x\"",
    "'",
    "\"",
    "''",
    "\"\"",
    "[a, b]",
    "[a, 'b', \"c\"",
    "[ , x ,]",
    "[]",
    "[x]]",
    ">",
    "|",
    ">-",
    "|+2",
    ">x",
    "user",
    "User # me",
    " USER#",
    "feedback",
    "project",
    "reference",
    "2026-01-02",
    "a: b",
    "#notcomment",
    "é ü",
    "\u{a0}nb\u{a0}",
    "x\u{3000}",
    "ſ",
    "İ",
    "Σ",
];

const INDENTS: &[&str] = &[
    "", "", "", " ", "  ", "  ", "   ", "    ", "      ", "\t", " \t", "\u{a0}", "\u{3000}",
];

fn gen_stem(rng: &mut Rng) -> String {
    match rng.below(5) {
        0 => String::new(),
        1 => rng.string(6),
        _ => rng
            .pick(&["note", "user-me", "project-x", "é", "a b"])
            .to_string(),
    }
}

fn gen_line(rng: &mut Rng) -> String {
    let indent = rng.pick(INDENTS).to_string();
    let key = rng.pick(KEYS).to_string();
    let rest = match rng.below(6) {
        0 => String::new(),
        1 => ":".to_string(),
        2 => format!(":{}", rng.pick(VALUES)),
        3 => format!(": {}", rng.string(8)),
        _ => format!(": {}", rng.pick(VALUES)),
    };
    let mut line = indent + &key + &rest;
    if rng.chance(0.05) {
        line.push(rng.unicode_char());
    }
    line
}

/// A well-formed note with random values, Claude Code's shape or a top-level one.
fn gen_shaped(rng: &mut Rng) -> String {
    let mut t = format!(
        "---\nname: {}\ndescription: {}\n",
        rng.pick(VALUES),
        rng.pick(VALUES)
    );
    if rng.chance(0.3) {
        t.push_str(&format!(
            "type: {}\nupdated: {}\n",
            rng.pick(VALUES),
            rng.pick(VALUES)
        ));
    } else {
        let ind = rng.pick(&["  ", " ", "    "]).to_string();
        t.push_str(&format!("metadata:\n{ind}type: {}\n", rng.pick(VALUES)));
        if rng.chance(0.5) {
            t.push_str(&format!("{ind}modified: {}\n", rng.pick(VALUES)));
        }
        if rng.chance(0.6) {
            t.push_str(&format!("{ind}relations:\n"));
            for _ in 0..rng.below(4) {
                let kind = rng.pick(&[
                    "supersedes",
                    "depends_on",
                    "relates_to",
                    " supersedes",
                    "other",
                ]);
                if rng.chance(0.5) {
                    t.push_str(&format!("{ind}{ind}{kind}: {}\n", rng.pick(VALUES)));
                } else {
                    t.push_str(&format!("{ind}{ind}{kind}:\n"));
                    for _ in 0..rng.below(3) {
                        t.push_str(&format!("{ind}{ind}- {}\n", rng.pick(VALUES)));
                    }
                }
            }
        }
    }
    t.push_str(rng.pick(&["---\n", "---", "--- \t\n", "----\n", ""]));
    t.push_str(&rng.string(20));
    t
}

fn gen_note(rng: &mut Rng) -> String {
    if rng.chance(0.35) {
        let mut t = gen_shaped(rng);
        if rng.chance(0.1) {
            t = t.replace('\n', "\r\n");
        }
        return t;
    }
    let mut t = String::new();
    t.push_str(rng.pick(&[
        "---\n",
        "---\n",
        "---\n",
        "\u{feff}---\n",
        "---\r\n",
        "--- \n",
        "",
        "x\n---\n",
    ]));
    let depth = rng.below(4) == 0;
    for i in 0..rng.below(12) {
        if depth {
            t.push_str(&" ".repeat(usize::try_from(i).unwrap_or(0)));
            t.push_str(rng.pick(KEYS));
            t.push_str(":\n");
        } else {
            t.push_str(&gen_line(rng));
            t.push_str(rng.pick(&["\n", "\n", "\n", "\r\n", "\n\n"]));
        }
    }
    t.push_str(rng.pick(&[
        "---\n", "---\n", "---", "--- \t\n", " ---\n", "----\n", "---x\n", "",
    ]));
    t.push_str(rng.pick(&["", "\n", "\n\n", "\r\n"]));
    t.push_str(&rng.string(16));
    t
}

fn gen_bytes(rng: &mut Rng) -> Vec<u8> {
    let mut data = gen_note(rng).into_bytes();
    for _ in 0..rng.below(5) {
        let at = rng.index(data.len() + 1);
        let junk: &[u8] = rng.pick(&[
            &b"\xff"[..],
            b"\x80",
            b"\xe2\x82",
            b"\xed\xa0\x80",
            b"\xc0\x80",
            b"\xf0\x80\x80",
            b"\xf4\x90\x80\x80",
            b"\xf0\x9f\x98",
            b"\xc3",
            b"\xef\xbb\xbf",
        ]);
        data.splice(at..at, junk.iter().copied());
    }
    data
}

fn gen_render(rng: &mut Rng) -> Value {
    let types = [
        "user",
        "feedback",
        "project",
        "reference",
        "zeta",
        "User # x",
        "aard",
        "",
    ];
    let mut notes = Vec::new();
    let count = match rng.below(4) {
        0 => rng.below(4),
        1 | 2 => rng.below(25),
        _ => rng.below(120),
    };
    for i in 0..count {
        let file = match rng.below(12) {
            0 => format!("n{i}.txt"),
            1 => format!("{} {i}.md", rng.ascii_string(4).replace('/', "_")),
            2 => format!("é{i}.md"),
            _ => format!("n{i:03}.md"),
        };
        let text = if rng.chance(0.15) {
            gen_note(rng)
        } else {
            let date = if rng.chance(0.3) {
                None
            } else {
                Some(format!(
                    "2026-{:02}-{:02}",
                    rng.below(12) + 1,
                    rng.below(28) + 1
                ))
            };
            let desc = match rng.below(5) {
                0 => String::new(),
                1 => "x".repeat(usize::try_from(rng.below(250)).unwrap_or(0)),
                2 => rng.string(30),
                _ => format!("desc {i}"),
            };
            let name = if rng.chance(0.2) {
                format!("dup{}", rng.below(3))
            } else {
                format!("note {i}")
            };
            note_text(&name, rng.pick(&types), &desc, date.as_deref())
        };
        notes.push((file, text));
    }
    let names: Vec<String> = notes.iter().map(|n| n.0.clone()).collect();
    let mut links = Vec::new();
    if rng.chance(0.2) && !names.is_empty() {
        links.push(("link.md".to_string(), rng.pick(&names).clone()));
    }
    if rng.chance(0.1) {
        notes.push(("MEMORY.md".to_string(), "stale\n".to_string()));
    }
    let budget = match rng.below(5) {
        0 => (150, 20000),
        1 => (40, 2500),
        2 => (
            usize::try_from(rng.below(40)).unwrap_or(0),
            usize::try_from(rng.below(3000)).unwrap_or(0),
        ),
        _ => (
            usize::try_from(rng.below(160)).unwrap_or(0),
            usize::try_from(rng.below(21000)).unwrap_or(0),
        ),
    };
    let suffix = rng
        .pick(&["", "", " (full index: /x/MEMORY.md)", " é"])
        .to_string();
    let exclude: Vec<&str> = match rng.below(4) {
        0 => vec!["user"],
        1 => vec![" User # c"],
        2 => vec!["user", "zeta"],
        _ => vec![],
    };
    render_case(&notes, &links, budget, &suffix, &exclude)
}

fn gen_remote(rng: &mut Rng) -> Value {
    let ws = ["", "", " ", "\t", "\u{a0}", "\u{2003}", "\n"];
    let url = match rng.below(6) {
        0 | 1 => {
            let scheme = rng.pick(&[
                "https", "http", "ssh", "git", "file", "FILE", "Https", "ftp", "git+ssh",
            ]);
            let cred = rng.pick(&["", "git@", "user:tok@", "a@b@", "u:@"]);
            let host = rng.pick(&[
                "github.com",
                "GitHub.COM",
                "h.example.invalid",
                "[::1]",
                "İhost",
                "h",
            ]);
            let port = rng.pick(&["", ":22", ":443", ":80", ":2222", ":", ":1:2"]);
            let path = rng.pick(&[
                "/acme/repo",
                "/Acme/Repo.git",
                "/a//b///",
                "/x.git/",
                "",
                "/",
                "//a",
            ]);
            format!("{scheme}://{cred}{host}{port}{path}")
        }
        2 => {
            let user = rng.pick(&["", "git@", "a b@", "x@y@"]);
            let host = rng.pick(&["github.com", "h", "Hé", "h/x"]);
            let path = rng.pick(&["acme/repo.git", "/srv/x.git", "", "a\nb", "a:b", "~/x"]);
            format!("{user}{host}:{path}")
        }
        3 => {
            let head = rng.pick(&["/", "./", "../", "~", "~/", "~bob/", "", "x/"]);
            let tail = rng.pick(&["x", "x.git", "a/../b.git", "srv//x/", ".git", ""]);
            format!("{head}{tail}")
        }
        _ => rng.string(20),
    };
    let url = format!("{}{url}{}", rng.pick(&ws), rng.pick(&ws));
    let base = rng
        .pick(&["/nonexistent-ms1/base", "/nonexistent-ms1/b/../c/", "/"])
        .to_string();
    let home = rng
        .pick(&["/nonexistent-ms1/home/", "", "/", "/h//"])
        .to_string();
    remote_case(&url, &base, &home)
}
