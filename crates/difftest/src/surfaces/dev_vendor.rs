//! The `dev.vendor` difftest surface (DV1's oracle `port/difftest/oracle/dev_vendor.py`, which
//! runs `scripts/vendor.py`).
//!
//! Cases (the oracle's docstring has the exact forms): `validate` (sources.json entries),
//! `strip` (the connector stripper on Markdown), `rename_skill`, `rename_plugin`, `checksums`
//! (CHECKSUMS.sha256 parsing), `compare` and `diff` (two trees), and `cli` (the argument parser
//! and the commands on a repo with no sources). Both sides print the same result; a
//! VendorError is exit 1 with `VendorError: <message>`.
//!
//! The generator stays inside what vendor.py handles without raising (plan GC-16 D-1: where
//! Python raises a TypeError on a wrongly typed field, Rust reports; those inputs are not
//! generated). No divergence is declared.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use anthology_dev::vendor;
use anthology_pycompat::json::{self, Int, Map, Value};
use anthology_testkit::tree;

use crate::case::{Case, Rng, Surface};
use crate::observe::{Observed, from_oracle};
use crate::oracle::Oracle;

/// The surface name (plan §1.4).
pub const NAME: &str = "dev.vendor";

/// Whether `difftest --all` runs this surface on macOS too (`ci/jobs/difftest-macos.sh`); set
/// `false` only for a surface that cannot run there, and say why.
pub const ON_MACOS: bool = true;

/// The registered implementation.
pub fn surface() -> Option<Box<dyn Surface>> {
    Some(Box::new(DevVendor))
}

struct DevVendor;

impl Surface for DevVendor {
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
        from_oracle(&oracle.call("dev_vendor", &c.data))
    }

    fn run_rust(&self, c: &Case) -> Observed {
        run(&c.data)
    }
}

// ------------------------------------------------------------------------------- the Rust side

fn get<'a>(case: &'a Value, k: &str) -> &'a Value {
    case.as_object()
        .and_then(|o| o.get(k))
        .unwrap_or_else(|| panic!("dev.vendor: no {k:?}"))
}

fn text<'a>(case: &'a Value, k: &str) -> &'a str {
    get(case, k)
        .as_str()
        .unwrap_or_else(|| panic!("dev.vendor: {k:?} is not a str"))
}

/// Writes a case TREE (`{rel: {"text", "exec"} | {"link"}}`) into `base`, parents 0755.
fn write_tree(base: &Path, spec: &Value) {
    let dir = |p: &Path| {
        fs::create_dir_all(p).expect("mkdir");
        fs::set_permissions(p, fs::Permissions::from_mode(0o755)).expect("chmod");
    };
    dir(base);
    let entries = spec.as_object().expect("dev.vendor: a tree is an object");
    let mut rels: Vec<&str> = entries.iter().map(|(k, _)| k).collect();
    rels.sort();
    for rel in rels {
        let e = entries.get(rel).expect("entry");
        let path = base.join(rel);
        let mut parent = path.parent().expect("parent").to_path_buf();
        let mut missing = Vec::new();
        while !parent.exists() {
            missing.push(parent.clone());
            parent.pop();
        }
        for d in missing.iter().rev() {
            dir(d);
        }
        if let Some(target) = e.as_object().and_then(|o| o.get("link")) {
            std::os::unix::fs::symlink(target.as_str().expect("link"), &path).expect("symlink");
        } else {
            fs::write(&path, text(e, "text")).expect("write");
            let exec = e
                .as_object()
                .and_then(|o| o.get("exec"))
                .is_some_and(|v| matches!(v, Value::Bool(true)));
            let mode = if exec { 0o755 } else { 0o644 };
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).expect("chmod");
        }
    }
}

fn ok(stdout: String) -> Observed {
    Observed::text(0, &stdout, "")
}

fn lines(items: &[String]) -> String {
    items.iter().map(|m| format!("{m}\n")).collect()
}

fn run(case: &Value) -> Observed {
    let tmp = tempfile::Builder::new()
        .prefix("dev-vendor-")
        .tempdir()
        .expect("tempdir");
    let tmp = fs::canonicalize(tmp.path()).expect("canonicalize");
    match op(case, &tmp) {
        Ok(o) => o,
        Err(e) => Observed::text(1, "", &format!("VendorError: {}", e.0)),
    }
}

fn with_files(mut o: Observed, base: &Path) -> Observed {
    o.files = Some(tree::snapshot(base));
    o
}

fn op(case: &Value, tmp: &Path) -> vendor::Result<Observed> {
    Ok(match text(case, "op") {
        "validate" => {
            let sources = get(case, "sources").as_array().expect("sources");
            ok(lines(&vendor::validate_sources(sources)))
        }
        "strip" => ok(vendor::strip_markdown(text(case, "text"))?),
        "rename_skill" => {
            let base = tmp.join("tree");
            let mut spec = Map::new();
            let mut skill = Map::new();
            skill.insert("text".into(), get(case, "skill").clone());
            spec.insert("SKILL.md".into(), Value::Object(skill));
            write_tree(&base, &Value::Object(spec));
            let stats = vendor::rename_skill(&base, text(case, "old"), text(case, "new"))?;
            with_files(
                ok(format!("{}\n", vendor::py::repr(&Value::Object(stats)))),
                &base,
            )
        }
        "rename_plugin" => {
            let base = tmp.join("tree");
            write_tree(&base, get(case, "tree"));
            let stats = vendor::rename_plugin(
                &base,
                text(case, "old"),
                text(case, "new"),
                get(case, "version"),
            )?;
            with_files(
                ok(format!("{}\n", vendor::py::repr(&Value::Object(stats)))),
                &base,
            )
        }
        "checksums" => {
            let data = unhex(text(case, "hex"));
            let listed = vendor::parse_checksums(&data)?;
            ok(listed
                .iter()
                .map(|(rel, digest)| format!("{rel} {digest}\n"))
                .collect())
        }
        op @ ("compare" | "diff") => {
            let (want, have) = (tmp.join("want"), tmp.join("have"));
            write_tree(&want, get(case, "want"));
            let have_spec = get(case, "have");
            if !matches!(have_spec, Value::Null) {
                write_tree(&have, have_spec);
            }
            if op == "compare" {
                ok(lines(&vendor::compare_trees(&want, &have, "t")?))
            } else {
                ok(vendor::diff_text(&want, &have)?)
            }
        }
        "cli" => {
            let root = tmp.join("root");
            fs::create_dir_all(root.join("vendor")).expect("mkdir");
            fs::write(root.join("vendor/sources.json"), "{\"sources\": []}\n").expect("write");
            let mut args: Vec<std::ffi::OsString> = vec!["--root".into(), root.into()];
            for a in get(case, "argv").as_array().expect("argv") {
                args.push(a.as_str().expect("arg").into());
            }
            // With no sources no command writes anything, so relative `--cache`/`--out`
            // paths are never used and the working folder does not matter.
            let out = anthology_dev::vendor::run(&args, &mut std::io::empty());
            Observed {
                exit: out.code,
                stdout: out.stdout,
                stderr: out.stderr,
                files: None,
            }
        }
        other => panic!("dev.vendor: unknown op {other:?}"),
    })
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
        .collect()
}

fn hex(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}

// -------------------------------------------------------------------------------- the cases

fn s(text: &str) -> Value {
    Value::Str(text.to_string())
}

fn obj(pairs: &[(&str, Value)]) -> Value {
    let mut m = Map::new();
    for (k, v) in pairs {
        m.insert((*k).to_string(), v.clone());
    }
    Value::Object(m)
}

fn file(text: &str, exec: bool) -> Value {
    obj(&[("text", s(text)), ("exec", Value::Bool(exec))])
}

fn link(target: &str) -> Value {
    obj(&[("link", s(target))])
}

fn tree_of(entries: &[(&str, Value)]) -> Value {
    obj(entries)
}

const MD_LINES: &[&str] = &[
    "# Title",
    "## Connectors",
    "## Tool integration",
    "## Connected tools",
    "## Connect your tools",
    "## Setup: connect https://example.com",
    "### If no tools are connected",
    "### Without connectors",
    "## Next",
    "### Deeper",
    "If ~~CRM is connected:",
    "**If** ~~chat~~ is connected, post it.",
    "If your tools are connected:",
    "If tools are connected, use them.",
    "- ~~chat~~ (if connected)",
    "- Connect via MCP to get more.",
    "- connect the MCP server",
    "> SUPERCHARGED: connect ~~tools~~ for more",
    "SUPERCHARGED with tools: connect them",
    "See CONNECTORS.md for setup.",
    "Uses ~~project tracker~~ when present.",
    "```",
    "````",
    "~~~",
    "```markdown",
    "  ```",
    "plain text",
    "  - nested item",
    "1. step one",
    "2) step two",
    "* star item",
    "",
    "",
    "    indented code",
    "## Tool İntegration",
    "## TOOL INTEGRATİON",
    "# Heading with trailing hashes ##",
    "#NoSpace",
    "\t## tabbed",
];

fn gen_markdown(rng: &mut Rng) -> String {
    let n = rng.index(14);
    let mut out: Vec<String> = Vec::new();
    for _ in 0..n {
        if rng.chance(0.1) {
            out.push(rng.string(12));
        } else {
            out.push(rng.pick(MD_LINES).to_string());
        }
    }
    let mut text = out.join(*rng.pick(&["\n", "\n", "\n", "\r\n"]));
    if rng.chance(0.7) {
        text.push('\n');
    }
    text
}

const NAMES: &[&str] = &["old", "x", "a-b", "skill", "ol", "old-x", "é"];
const NEW_NAMES: &[&str] = &["new", "y", "c-d", "anthology-old", "n2"];

fn gen_skill(rng: &mut Rng, old: &str) -> String {
    let name_line = match rng.below(7) {
        0 => format!("name: {old}"),
        1 => format!("name: \"{old}\""),
        2 => format!("name:   '{old}'  "),
        3 => "name: other".to_string(),
        4 => "title: no name".to_string(),
        5 => format!("name:\t{old}\t"),
        _ => format!("name: {old}"),
    };
    let body_bits = [
        format!("Run /{old} now."),
        format!("Not /{old}-checklist or a/{old}x."),
        format!("Path x/{old} and word/{old}."),
        format!("name: {old} in the body"),
        format!("/{old}"),
        "plain body".to_string(),
        format!("/{old}é and é/{old}"),
    ];
    let mut body = String::new();
    for _ in 0..rng.index(5) {
        body.push_str(rng.pick(&body_bits));
        body.push('\n');
    }
    match rng.below(10) {
        0 => body,
        1 => format!("---\n{name_line}\n{body}"),
        _ => format!("---\n{name_line}\ndescription: d\n---\n{body}"),
    }
}

fn gen_plugin_tree(rng: &mut Rng, old: &str) -> Value {
    let mut entries: Vec<(String, Value)> = Vec::new();
    if rng.below(10) > 0 {
        let name = if rng.chance(0.85) { old } else { "other" };
        let mut m = Map::new();
        m.insert("name".into(), s(name));
        m.insert("version".into(), s("9.9.9"));
        if rng.chance(0.5) {
            m.insert("description".into(), s("Ünïcode ☃"));
        }
        if rng.chance(0.3) {
            m.insert(
                "keywords".into(),
                Value::Array(vec![s("a"), Value::Int(Int::Small(2))]),
            );
        }
        let pretty = if rng.chance(0.5) {
            json::dumps(&Value::Object(m), &json::INDENT2)
        } else {
            json::dumps(&Value::Object(m), &json::PY_DEFAULT)
        };
        entries.push((".claude-plugin/plugin.json".into(), file(&pretty, false)));
    }
    if rng.chance(0.4) {
        entries.push((
            ".claude-plugin/marketplace.json".into(),
            file("{}\n", false),
        ));
    }
    let bits = [
        format!("Use {old}:thing and {old}:other-skill."),
        format!("Not anthology-{old}:x, /{old}:y, .{old}:z or {old}: prose."),
        format!("{old}:{old}:a"),
        format!("{old}:Upper and {old}:9"),
        "nothing here".to_string(),
    ];
    for (i, rel) in [
        "skills/a/SKILL.md",
        "README.md",
        "hooks/run.sh",
        "deep/x/y.txt",
    ]
    .iter()
    .enumerate()
    {
        if rng.chance(0.6) {
            let mut t = String::new();
            for _ in 0..1 + rng.index(3) {
                t.push_str(rng.pick(&bits));
                t.push('\n');
            }
            if i == 3 && rng.chance(0.3) {
                t.push('\0');
            }
            entries.push(((*rel).to_string(), file(&t, i == 2)));
        }
    }
    if rng.chance(0.2) {
        entries.push(("link.md".into(), link("README.md")));
    }
    let pairs: Vec<(&str, Value)> = entries
        .iter()
        .map(|(k, v)| (k.as_str(), v.clone()))
        .collect();
    tree_of(&pairs)
}

const SAFE_RELS: &[&str] = &["a.md", "b/c.md", "LICENSE", "x/y/z.txt", "é/ü.md", "a b.md"];
const BAD_RELS: &[&str] = &[
    "../x",
    "/etc/passwd",
    "a/../../b",
    "a//b",
    "./a",
    "a\\b",
    "CHECKSUMS.sha256",
    "",
    "a\tb",
    "a/./b",
    "-",
];

fn gen_checksums(rng: &mut Rng) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    for _ in 0..rng.index(5) {
        let digest: String = (0..64)
            .map(|_| char::from(b"0123456789abcdef"[rng.index(16)]))
            .collect();
        let rel = if rng.chance(0.8) {
            rng.pick(SAFE_RELS)
        } else {
            rng.pick(BAD_RELS)
        };
        let line = match rng.below(12) {
            0 => format!("{digest} {rel}"),
            1 => format!("{}  {rel}", &digest[..63]),
            2 => format!("{}  {rel}", digest.to_uppercase()),
            3 => String::new(),
            4 => format!("{digest}  {rel}  "),
            5 => format!("{digest}\t{rel}"),
            _ => format!("{digest}  {rel}"),
        };
        out.extend(line.as_bytes());
        let end: &[u8] = rng.pick(&[
            &b"\n"[..],
            b"\n",
            b"\n",
            b"\r\n",
            b"\r",
            b"\x0b",
            b"\xe2\x80\xa8",
        ]);
        out.extend_from_slice(end);
    }
    if rng.chance(0.05) {
        out.extend(b"\xff\xfe");
    }
    out
}

const TREE_RELS: &[&str] = &[
    "a.md",
    "b/c.md",
    "b/d.sh",
    "x/y/z.txt",
    ".DS_Store",
    "sub/.DS_Store",
    "é.md",
    "LICENSE",
];

const TEXTS: &[&str] = &[
    "",
    "one\n",
    "one",
    "one\ntwo\nthree\n",
    "one\r\ntwo\r\n",
    "a\rb\n",
    "x\u{2028}y\n",
    "line\n\n\nend\n",
    "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n",
    "ü ☃ 😀\n",
];

fn gen_text(rng: &mut Rng) -> String {
    if rng.chance(0.15) {
        let mut t = String::new();
        for _ in 0..rng.index(30) {
            t.push_str(rng.pick(&["a\n", "b\n", "c\n", "d\n", "e"]));
        }
        t
    } else {
        rng.pick(TEXTS).to_string()
    }
}

fn gen_tree(rng: &mut Rng) -> Vec<(String, Value)> {
    let mut entries = Vec::new();
    for rel in TREE_RELS {
        if rng.chance(0.5) {
            let v = if rng.chance(0.08) {
                link(rng.pick(&["a.md", "LICENSE", "nowhere", "b"]))
            } else {
                file(&gen_text(rng), rel.ends_with(".sh") || rng.chance(0.1))
            };
            entries.push(((*rel).to_string(), v));
        }
    }
    entries
}

fn mutate_tree(rng: &mut Rng, want: &[(String, Value)]) -> Vec<(String, Value)> {
    let mut have: Vec<(String, Value)> = want.to_vec();
    for _ in 0..rng.index(4) {
        match rng.below(5) {
            0 if !have.is_empty() => {
                let i = rng.index(have.len());
                have.remove(i);
            }
            1 if !have.is_empty() => {
                let i = rng.index(have.len());
                have[i].1 = file(&gen_text(rng), rng.chance(0.3));
            }
            2 if !have.is_empty() => {
                let i = rng.index(have.len());
                let exec = !have[i]
                    .1
                    .as_object()
                    .and_then(|o| o.get("exec"))
                    .is_some_and(|v| matches!(v, Value::Bool(true)));
                if let Some(t) = have[i].1.as_object().and_then(|o| o.get("text")).cloned() {
                    have[i].1 = obj(&[("text", t), ("exec", Value::Bool(exec))]);
                }
            }
            3 => {
                let rel = rng.pick(TREE_RELS).to_string();
                if !have.iter().any(|(k, _)| *k == rel) {
                    have.push((rel, file(&gen_text(rng), false)));
                }
            }
            _ => {}
        }
    }
    have
}

fn tree_value(entries: &[(String, Value)]) -> Value {
    // A file and a folder of the same name cannot both exist: keep the first.
    let mut m = Map::new();
    let mut taken: Vec<&str> = Vec::new();
    for (rel, v) in entries {
        let clash = taken
            .iter()
            .any(|t| rel.starts_with(&format!("{t}/")) || t.starts_with(&format!("{rel}/")));
        if !clash {
            taken.push(rel);
            m.insert(rel.clone(), v.clone());
        }
    }
    Value::Object(m)
}

const CLI_WORDS: &[&str] = &[
    "fetch",
    "build",
    "check",
    "sync",
    "diff",
    "bogus",
    "che",
    "-h",
    "--help",
    "--h",
    "--cache",
    "--cache=c",
    "--ca",
    "--out",
    "--ou=o",
    "--out=",
    "--ref",
    "--re",
    "--ref=v1",
    "--bogus",
    "-x",
    "--",
    "-1",
    "a",
    "x",
    "",
    "a b",
    "v1",
    "names",
    "-hx",
    "--help=1",
    "--=",
    "-h=",
    "-",
    "--o",
    "--c",
    "--he=x",
    "-1.5",
    "-h-x",
];

fn gen_argv(rng: &mut Rng) -> Vec<Value> {
    let mut argv = Vec::new();
    if rng.chance(0.85) {
        if rng.chance(0.2) {
            argv.push(s(rng.pick(&["--cache", "--ca"])));
            argv.push(s("c"));
        }
        argv.push(s(rng.pick(&["fetch", "build", "check", "sync", "diff"])));
    }
    for _ in 0..rng.index(5) {
        argv.push(s(rng.pick(CLI_WORDS)));
    }
    argv
}

/// Values a sources.json field may hold without making vendor.py raise.
fn gen_field_value(rng: &mut Rng, key: &str) -> Value {
    let strs: &[&str] = match key {
        "name" => &["a", "B", "a b", "", "-a", "a-1", "é", "rtk"],
        "repo" => &[
            "https://github.com/acme/a",
            "http://github.com/acme/a",
            "https://github.com/acme/a.git",
            "https://github.com/acme",
            "git@github.com:acme/a",
            "",
        ],
        "url" => &[
            "https://example.test/.well-known/skills/a/",
            "https://example.test/a",
            "http://example.test/a/",
            "",
        ],
        "commit" => &[
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "abc",
            "",
        ],
        "checksum" => &[
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "abc",
            "",
        ],
        "license" => &["MIT", "Apache-2.0", "GPL-3.0-only", "mit", ""],
        "target" => &[
            "plugins/anthology/knowledge/a",
            "plugins/anthology/knowledge/b",
            "plugins/anthology",
            "../x",
            "/abs",
            ".",
            "",
            "rtk",
            "plugins/other/x",
        ],
        "path" | "license_file" => &["", "LICENSE", "skills/a", "../x", "/abs", "a//b", "a/./b"],
        "portmap" => &[
            "vendor/patches/a.portmap.json",
            "vendor/patches/a.txt",
            "../x.json",
            "vendor/other/a.json",
        ],
        _ => &["x", "", "1.0.0"],
    };
    match rng.below(10) {
        0 if !matches!(
            key,
            "name" | "license" | "repo" | "url" | "commit" | "checksum"
        ) =>
        {
            Value::Int(Int::Small(rng.range(-1, 3)))
        }
        1 => Value::Null,
        2 if !matches!(key, "repo" | "url" | "commit" | "checksum") => Value::Bool(rng.chance(0.5)),
        _ => s(rng.pick(strs)),
    }
}

fn gen_transform(rng: &mut Rng) -> Value {
    let one = |rng: &mut Rng| -> Value {
        match rng.below(8) {
            0 => s("strip-connectors"),
            1 => s("rename-plugin"),
            2 => obj(&[
                ("name", s("rename-plugin")),
                ("old", s("a")),
                ("new", s("b")),
            ]),
            3 => obj(&[
                ("name", s("rename-skill")),
                ("old", s("a")),
                ("new", s("b")),
            ]),
            4 => obj(&[("patch", s("vendor/patches/a.patch"))]),
            5 => obj(&[("patch", s("../x.patch"))]),
            6 => s("nope"),
            _ => obj(&[("name", s("strip-connectors"))]),
        }
    };
    match rng.below(6) {
        0 => s("strip-connectors"),
        1 => Value::Array(Vec::new()),
        _ => Value::Array((0..1 + rng.index(3)).map(|_| one(rng)).collect()),
    }
}

fn gen_entry(rng: &mut Rng, i: usize) -> Value {
    let name = ["a", "b", "c"][i % 3];
    let mut e = Map::new();
    if rng.chance(0.25) {
        e.insert("name".into(), s(name));
        e.insert(
            "url".into(),
            s(&format!("https://example.test/.well-known/skills/{name}/")),
        );
        e.insert("path".into(), s(""));
        e.insert("ref".into(), s("v1"));
        e.insert("checksum".into(), s(&"a".repeat(64)));
    } else {
        e.insert("name".into(), s(name));
        e.insert("repo".into(), s(&format!("https://github.com/acme/{name}")));
        e.insert("path".into(), s(""));
        e.insert("ref".into(), s("main"));
        e.insert("commit".into(), s(&"a".repeat(40)));
    }
    e.insert("license".into(), s("MIT"));
    e.insert("license_file".into(), s("LICENSE"));
    e.insert(
        "target".into(),
        s(&format!("plugins/anthology/knowledge/{name}")),
    );
    e.insert("transform".into(), Value::Array(Vec::new()));
    if rng.chance(0.1) {
        let mut sub = Map::new();
        sub.insert("name".into(), s("rtk"));
        sub.insert("managed_by".into(), s("subtree"));
        sub.insert("license".into(), s("Apache-2.0"));
        sub.insert("target".into(), s("rtk"));
        return Value::Object(sub);
    }
    let keys = [
        "name",
        "repo",
        "url",
        "path",
        "ref",
        "commit",
        "checksum",
        "license",
        "license_file",
        "license_note",
        "target",
        "plugin_version",
        "portmap",
    ];
    for _ in 0..rng.index(4) {
        let k = *rng.pick(&keys);
        if rng.chance(0.15) {
            e.remove(k);
        } else {
            let v = gen_field_value(rng, k);
            e.insert(k.into(), v);
        }
    }
    if rng.chance(0.3) {
        e.insert("transform".into(), gen_transform(rng));
    }
    if rng.chance(0.15) {
        let extra = match rng.below(4) {
            0 => Value::Array(vec![s("NOTICE")]),
            1 => Value::Array(vec![s("NOTICE"), s("docs/NOTICE")]),
            2 => Value::Array(vec![s("../NOTICE")]),
            _ => s("NOTICE"),
        };
        e.insert("license_extra".into(), extra);
    }
    Value::Object(e)
}

fn gen_case(rng: &mut Rng) -> Value {
    match rng.below(8) {
        0 => {
            let n = rng.index(4);
            let sources = (0..n)
                .map(|i| {
                    let k = i + rng.index(2);
                    gen_entry(rng, k)
                })
                .collect();
            obj(&[("op", s("validate")), ("sources", Value::Array(sources))])
        }
        1 => obj(&[("op", s("strip")), ("text", s(&gen_markdown(rng)))]),
        2 => {
            let old = rng.pick(NAMES).to_string();
            let new = rng.pick(NEW_NAMES).to_string();
            let skill = gen_skill(rng, &old);
            obj(&[
                ("op", s("rename_skill")),
                ("skill", s(&skill)),
                ("old", s(&old)),
                ("new", s(&new)),
            ])
        }
        3 => {
            let old = rng.pick(NAMES).to_string();
            let new = rng.pick(NEW_NAMES).to_string();
            let tree = gen_plugin_tree(rng, &old);
            obj(&[
                ("op", s("rename_plugin")),
                ("tree", tree),
                ("old", s(&old)),
                ("new", s(&new)),
                ("version", s(rng.pick(&["0.1.0", "2.0.0-rc.1"]))),
            ])
        }
        4 => obj(&[
            ("op", s("checksums")),
            ("hex", s(&hex(&gen_checksums(rng)))),
        ]),
        5 | 6 => {
            let want = gen_tree(rng);
            let have = if rng.chance(0.05) {
                Value::Null
            } else {
                tree_value(&mutate_tree(rng, &want))
            };
            let op = if rng.chance(0.5) { "compare" } else { "diff" };
            obj(&[("op", s(op)), ("want", tree_value(&want)), ("have", have)])
        }
        _ => obj(&[("op", s("cli")), ("argv", Value::Array(gen_argv(rng)))]),
    }
}

fn seeds() -> Vec<Case> {
    let mut cases = Vec::new();
    let mut add = |id: &str, v: Value| cases.push(Case::new(format!("seed:{id}"), v));
    add(
        "validate-ok",
        obj(&[
            ("op", s("validate")),
            (
                "sources",
                Value::Array(vec![gen_entry(&mut Rng::new(7), 0)]),
            ),
        ]),
    );
    add(
        "strip-sections",
        obj(&[
            ("op", s("strip")),
            (
                "text",
                s(
                    "# T\n\n## Connectors\n\nIf ~~CRM is connected:\n- a\n\n### If no tools are \
                   connected\n- b\n\n## Next\n\nSee CONNECTORS.md.\n",
                ),
            ),
        ]),
    );
    add(
        "strip-unhandled",
        obj(&[("op", s("strip")), ("text", s("Uses ~~tracker~~ here.\n"))]),
    );
    add(
        "rename-skill",
        obj(&[
            ("op", s("rename_skill")),
            ("skill", s("---\nname: old\n---\nRun /old, not /old-x.\n")),
            ("old", s("old")),
            ("new", s("new")),
        ]),
    );
    add(
        "rename-plugin",
        obj(&[
            ("op", s("rename_plugin")),
            (
                "tree",
                tree_of(&[
                    (
                        ".claude-plugin/plugin.json",
                        file("{\"name\": \"old\", \"version\": \"1\"}\n", false),
                    ),
                    ("a.md", file("Use old:thing, not /old:x.\n", false)),
                ]),
            ),
            ("old", s("old")),
            ("new", s("new")),
            ("version", s("0.1.0")),
        ]),
    );
    add(
        "checksums",
        obj(&[
            ("op", s("checksums")),
            (
                "hex",
                s(&hex(format!(
                    "{}  a.md\n{}  b/c.md\n",
                    "1".repeat(64),
                    "2".repeat(64)
                )
                .as_bytes())),
            ),
        ]),
    );
    let want = vec![
        ("a.md".to_string(), file("one\ntwo\n", false)),
        ("b/d.sh".to_string(), file("#!/bin/sh\n", true)),
    ];
    let have = vec![
        ("a.md".to_string(), file("one\n2\n", false)),
        ("new.md".to_string(), file("new", false)),
    ];
    for op in ["compare", "diff"] {
        add(
            op,
            obj(&[
                ("op", s(op)),
                ("want", tree_value(&want)),
                ("have", tree_value(&have)),
            ]),
        );
    }
    for (id, argv) in [
        ("cli-help", vec!["--help"]),
        ("cli-check", vec!["check"]),
        ("cli-sync-missing-ref", vec!["sync", "a"]),
        ("cli-unknown-source", vec!["build", "a"]),
        ("cli-bogus", vec!["bogus"]),
    ] {
        add(
            id,
            obj(&[
                ("op", s("cli")),
                ("argv", Value::Array(argv.into_iter().map(s).collect())),
            ]),
        );
    }
    cases
}
