//! The `dev.rtk-trim` difftest surface (DV4; oracle `port/difftest/oracle/dev_rtk_trim.py`,
//! which runs `scripts/rtk_trim.py` itself).
//!
//! Five modes, each a folder-in, observation-out case (see the oracle's docstring for the data):
//!
//! - `trim`: the real operations over the files they edit, copied from `rtk/` of this
//!   repository and then edited by `mutations`. The mutations mostly *undo* operations (put a
//!   cut text back, swap a replacement for its original, re-insert a dropped item), so the trim
//!   has real work to do and its anchor checks fail in the interesting places. One seed undoes
//!   every operation, which stands in for the pristine upstream tree.
//! - `prim`: `rm`/`sub`/`cut`/`drop` on small random Rust-like files: exact, altered and absent
//!   anchors, `done` markers, ambiguous anchors, items with and without closers.
//! - `lock-guard`: random Cargo.lock pairs (CRLF and lone CR line ends, odd lines), and pairs
//!   with an `rtk/Cargo.toml` that names a path package (the m-14 `../crates/` allowlist, which
//!   Python lacks). When the allowlist allows nothing (a `..` segment, a path outside
//!   `../crates/`, an unreadable or nameless crate manifest, a registry package of that name,
//!   no manifest) Python itself is the oracle and the result must be identical; when it allows
//!   the closure, the oracle's own hand-written reference of the rule is. The finer manifest
//!   shapes (table form, `package =` renames) are covered by `allowlist_test` only.
//! - `trimmed-conflicts`: random `git status --porcelain` texts.
//! - `removed-paths`.
//!
//! Observed: exit code, stdout, stderr and (for `trim` and `prim`) the folder as a tree; in
//! `trim` mode each file's text is its sha256. Rust runs `anthology_dev::rtk_trim::run_in` (the
//! CLI) or `Trim` (the primitives) in-process; Python runs `rtk_trim.main` or `Trim`.
//!
//! The Rust side fails closed next to ANY non-ASCII character beside a dropped item's name
//! (P1 owns Python's `\w` tables): it stops where Python's `\w` may let the trim skip. The
//! generators keep those characters out; the `seed:nonascii-*` seeds show the allowed
//! difference (`port/difftest/divergences/dev.rtk-trim.toml`, D-3, scoped by case).

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use anthology_dev::rtk_trim::{self, Op, Trim};
use anthology_pycompat::json::{Int, Map, Value};
use anthology_testkit::tree::{Tree, TreeEntry, snapshot};
use sha2::{Digest, Sha256};

use crate::case::{Case, Rng, Surface, repo_root};
use crate::observe::{Observed, from_oracle};
use crate::oracle::Oracle;

/// The surface name (plan §1.4).
pub const NAME: &str = "dev.rtk-trim";

/// Whether `difftest --all` runs this surface on macOS too: yes, temporary folders only.
pub const ON_MACOS: bool = true;

/// The registered implementation.
pub fn surface() -> Option<Box<dyn Surface>> {
    Some(Box::new(DevRtkTrim))
}

struct DevRtkTrim;

impl Surface for DevRtkTrim {
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
        from_oracle(&oracle.call("dev_rtk_trim", &c.data))
    }

    fn run_rust(&self, c: &Case) -> Observed {
        run(&c.data)
    }
}

// ------------------------------------------------------------------------------------------
// Value helpers

fn s(x: &str) -> Value {
    Value::Str(x.to_string())
}

fn opt(x: Option<&str>) -> Value {
    x.map_or(Value::Null, s)
}

fn num(n: usize) -> Value {
    Value::Int(Int::Small(i64::try_from(n).unwrap_or(i64::MAX)))
}

fn arr(items: Vec<Value>) -> Value {
    Value::Array(items)
}

fn obj(fields: Vec<(&str, Value)>) -> Value {
    let mut m = Map::new();
    for (k, v) in fields {
        m.insert(k.to_string(), v);
    }
    Value::Object(m)
}

fn field<'a>(v: &'a Value, k: &str) -> &'a Value {
    v.as_object()
        .and_then(|o| o.get(k))
        .unwrap_or_else(|| panic!("dev.rtk-trim: no {k:?}"))
}

fn maybe<'a>(v: &'a Value, k: &str) -> Option<&'a Value> {
    v.as_object().and_then(|o| o.get(k))
}

fn text<'a>(v: &'a Value, k: &str) -> &'a str {
    field(v, k)
        .as_str()
        .unwrap_or_else(|| panic!("dev.rtk-trim: {k:?} is not a string"))
}

fn opt_text<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    field(v, k).as_str()
}

fn list<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    field(v, k)
        .as_array()
        .unwrap_or_else(|| panic!("dev.rtk-trim: {k:?} is not a list"))
}

fn pairs<'a>(v: &'a Value, k: &str) -> Vec<(&'a str, &'a str)> {
    field(v, k)
        .as_object()
        .unwrap_or_else(|| panic!("dev.rtk-trim: {k:?} is not an object"))
        .iter()
        .map(|(name, t)| (name, t.as_str().expect("a string")))
        .collect()
}

fn map_of(entries: &[(&str, &str)]) -> Value {
    obj(entries.iter().map(|&(k, v)| (k, s(v))).collect())
}

// ------------------------------------------------------------------------------------------
// Running a case in Rust

fn ensure_dir(base: &Path, rel_parent: &str) {
    let mut cur = base.to_path_buf();
    for part in rel_parent.split('/').filter(|p| !p.is_empty()) {
        cur.push(part);
        if !cur.is_dir() {
            fs::create_dir(&cur).expect("mkdir");
            fs::set_permissions(&cur, fs::Permissions::from_mode(0o755)).expect("chmod");
        }
    }
}

fn write_file(base: &Path, rel: &str, content: &str) {
    ensure_dir(base, rel.rsplit_once('/').map_or("", |(parent, _)| parent));
    let full = base.join(rel);
    fs::write(&full, content).expect("write");
    fs::set_permissions(&full, fs::Permissions::from_mode(0o644)).expect("chmod");
}

/// The files the operations edit, in first-use order.
fn touched_paths() -> Vec<&'static str> {
    let mut seen: Vec<&'static str> = Vec::new();
    for op in rtk_trim::operations() {
        if let Op::Sub { path, .. } | Op::Cut { path, .. } | Op::Drop { path, .. } = op
            && !seen.contains(&path)
        {
            seen.push(path);
        }
    }
    seen
}

fn mutate(content: &str, m: &Value) -> String {
    let kind = text(m, "kind");
    match kind {
        "replace" => {
            let find = text(m, "find");
            if find.is_empty() {
                content.to_string()
            } else {
                content.replace(find, text(m, "with"))
            }
        }
        "append" => format!("{content}{}", text(m, "text")),
        _ => {
            let mut lines: Vec<String> = content.split('\n').map(str::to_string).collect();
            let at = field(m, "line")
                .as_i64()
                .and_then(|n| usize::try_from(n).ok())
                .expect("a line number");
            match kind {
                "insert" => lines.insert(at.min(lines.len()), text(m, "text").to_string()),
                "delete_line" => {
                    if at < lines.len() {
                        lines.remove(at);
                    }
                }
                "dup_line" => {
                    if at < lines.len() {
                        let line = lines[at].clone();
                        lines.insert(at, line);
                    }
                }
                other => panic!("dev.rtk-trim: unknown mutation kind {other:?}"),
            }
            lines.join("\n")
        }
    }
}

fn hashed(tree: Tree) -> Tree {
    Tree(
        tree.0
            .into_iter()
            .map(|(path, entry)| {
                let entry = match entry {
                    TreeEntry::File { bytes, mode } => {
                        let digest: String = Sha256::digest(&bytes)
                            .iter()
                            .map(|b| format!("{b:02x}"))
                            .collect();
                        TreeEntry::File {
                            bytes: format!("sha256:{digest}").into_bytes(),
                            mode,
                        }
                    }
                    other => other,
                };
                (path, entry)
            })
            .collect(),
    )
}

fn cli(cwd: &Path, args: &[&str], stdin: &str) -> Observed {
    let args: Vec<std::ffi::OsString> = args.iter().map(Into::into).collect();
    let out = rtk_trim::run_in(cwd, &args, &mut stdin.as_bytes());
    Observed {
        exit: out.code,
        stdout: out.stdout,
        stderr: out.stderr,
        files: None,
    }
}

fn prim(cwd: &Path, case: &Value) -> Observed {
    let mut t = Trim::new(cwd.join("rtk"));
    let mut failure = None;
    for op in list(case, "ops") {
        let path = || text(op, "path");
        let result = match text(op, "op") {
            "rm" => {
                let paths: Vec<&str> = list(op, "paths").iter().filter_map(Value::as_str).collect();
                t.rm(&paths)
            }
            "sub" => t.sub(
                path(),
                text(op, "old"),
                text(op, "new"),
                opt_text(op, "done"),
            ),
            "cut" => t.cut(path(), text(op, "old"), text(op, "gone")),
            "drop" => t.drop_item(path(), text(op, "sig"), opt_text(op, "gone")),
            other => panic!("dev.rtk-trim: unknown op {other:?}"),
        };
        if let Err(e) = result {
            failure = Some(e);
            break;
        }
    }
    match failure {
        Some(e) => Observed::text(1, "", &format!("rtk-trim: {e}\n")),
        None => {
            let stdout: String = t.changed.iter().map(|c| format!("{c}\n")).collect();
            Observed::text(0, &stdout, "")
        }
    }
}

fn run(case: &Value) -> Observed {
    let tmp = tempfile::tempdir().expect("temp dir");
    let cwd = tmp.path();
    let rtk = cwd.join("rtk");
    match text(case, "mode") {
        "removed-paths" => cli(cwd, &["--removed-paths"], ""),
        "trimmed-conflicts" => cli(cwd, &["--trimmed-conflicts"], text(case, "status")),
        "lock-guard" => {
            for name in ["before", "after"] {
                fs::write(cwd.join(format!("{name}.lock")), text(case, name)).expect("write");
            }
            if let Some(manifest) = maybe(case, "manifest").and_then(Value::as_str) {
                fs::create_dir(&rtk).expect("mkdir");
                write_file(&rtk, "Cargo.toml", manifest);
            }
            if maybe(case, "crates").is_some() {
                for (rel, content) in pairs(case, "crates") {
                    write_file(cwd, rel, content);
                }
            }
            cli(cwd, &["--lock-guard", "before.lock", "after.lock"], "")
        }
        "prim" => {
            fs::create_dir(&rtk).expect("mkdir");
            for (rel, content) in pairs(case, "files") {
                write_file(&rtk, rel, content);
            }
            let mut out = prim(cwd, case);
            out.files = Some(snapshot(&rtk));
            out
        }
        "trim" => {
            fs::create_dir(&rtk).expect("mkdir");
            let mut base: Vec<(&str, String)> = Vec::new();
            for rel in touched_paths() {
                if let Ok(content) = fs::read_to_string(repo_root().join("rtk").join(rel)) {
                    base.push((rel, content));
                }
            }
            for m in list(case, "mutations") {
                let path = text(m, "path");
                if let Some((_, content)) = base.iter_mut().find(|(rel, _)| *rel == path) {
                    *content = mutate(content, m);
                }
            }
            for (rel, content) in &base {
                write_file(&rtk, rel, content);
            }
            for (rel, content) in pairs(case, "extras") {
                write_file(&rtk, rel, content);
            }
            for (rel, target) in pairs(case, "links") {
                ensure_dir(&rtk, rel.rsplit_once('/').map_or("", |(parent, _)| parent));
                std::os::unix::fs::symlink(target, rtk.join(rel)).expect("symlink");
            }
            let mut out = cli(cwd, &["--root", "rtk"], "");
            out.files = Some(hashed(snapshot(&rtk)));
            out
        }
        other => panic!("dev.rtk-trim: unknown mode {other:?}"),
    }
}

// ------------------------------------------------------------------------------------------
// Case builders

fn trim_case(mutations: Vec<Value>, extras: &[(&str, &str)], links: &[(&str, &str)]) -> Value {
    obj(vec![
        ("mode", s("trim")),
        ("mutations", arr(mutations)),
        ("extras", map_of(extras)),
        ("links", map_of(links)),
    ])
}

fn prim_case(files: &[(&str, &str)], ops: Vec<Value>) -> Value {
    obj(vec![
        ("mode", s("prim")),
        ("files", map_of(files)),
        ("ops", arr(ops)),
    ])
}

fn op_rm(paths: &[&str]) -> Value {
    obj(vec![
        ("op", s("rm")),
        ("paths", arr(paths.iter().map(|p| s(p)).collect())),
    ])
}

fn op_sub(path: &str, old: &str, new: &str, done: Option<&str>) -> Value {
    obj(vec![
        ("op", s("sub")),
        ("path", s(path)),
        ("old", s(old)),
        ("new", s(new)),
        ("done", opt(done)),
    ])
}

fn op_cut(path: &str, old: &str, gone: &str) -> Value {
    obj(vec![
        ("op", s("cut")),
        ("path", s(path)),
        ("old", s(old)),
        ("gone", s(gone)),
    ])
}

fn op_drop(path: &str, sig: &str, gone: Option<&str>) -> Value {
    obj(vec![
        ("op", s("drop")),
        ("path", s(path)),
        ("sig", s(sig)),
        ("gone", opt(gone)),
    ])
}

fn lock_case(before: &str, after: &str) -> Value {
    obj(vec![
        ("mode", s("lock-guard")),
        ("before", s(before)),
        ("after", s(after)),
    ])
}

/// One `[[package]]` of a Cargo.lock, in cargo's layout.
fn lp(name: &str, version: &str, source: bool, deps: &[&str]) -> String {
    let mut out = format!("[[package]]\nname = \"{name}\"\nversion = \"{version}\"\n");
    if source {
        out.push_str("source = \"registry+https://github.com/rust-lang/crates.io-index\"\n");
    }
    if !deps.is_empty() {
        out.push_str("dependencies = [\n");
        for d in deps {
            out.push_str(&format!(" \"{d}\",\n"));
        }
        out.push_str("]\n");
    }
    out
}

fn lock_of(packages: &[String]) -> String {
    format!("version = 4\n\n{}", packages.join("\n"))
}

/// rtk's lock before a sync that adds `anthology-shell`.
fn allow_before() -> String {
    lock_of(&[
        lp("rtk", "1.0.0", false, &["regex"]),
        lp("regex", "1.0.0", true, &[]),
        lp("a", "1.0.0", true, &[]),
    ])
}

/// ... and after: the path package, its closure, and `extra` packages.
fn allow_after(extra: &[String]) -> String {
    let mut packages = vec![
        lp("rtk", "1.0.0", false, &["anthology-shell", "regex"]),
        lp("regex", "1.0.0", true, &[]),
        lp("a", "1.0.0", true, &[]),
        lp("anthology-shell", "0.1.0", false, &["fancy-regex", "regex"]),
        lp("fancy-regex", "0.20.2", true, &["bit-set"]),
        lp("bit-set", "0.8.0", true, &["bit-vec"]),
        lp("bit-vec", "0.8.0", true, &[]),
    ];
    packages.extend_from_slice(extra);
    lock_of(&packages)
}

fn manifest(path: &str) -> String {
    format!(
        "[package]\nname = \"rtk\"\nversion = \"1.0.0\"\n\n[dependencies]\nregex = \"1\"\nanthology-shell = {{ path = \"{path}\" }}\n"
    )
}

fn allow_case(before: &str, after: &str, manifest: Option<&str>, crates: &[(&str, &str)]) -> Value {
    obj(vec![
        ("mode", s("lock-guard")),
        ("before", s(before)),
        ("after", s(after)),
        ("manifest", opt(manifest)),
        ("crates", map_of(crates)),
    ])
}

fn conflicts_case(status: &str) -> Value {
    obj(vec![
        ("mode", s("trimmed-conflicts")),
        ("status", s(status)),
    ])
}

fn m_replace(path: &str, find: &str, with: &str) -> Value {
    obj(vec![
        ("path", s(path)),
        ("kind", s("replace")),
        ("find", s(find)),
        ("with", s(with)),
    ])
}

fn m_append(path: &str, content: &str) -> Value {
    obj(vec![
        ("path", s(path)),
        ("kind", s("append")),
        ("text", s(content)),
    ])
}

fn m_insert(path: &str, line: usize, content: &str) -> Value {
    obj(vec![
        ("path", s(path)),
        ("kind", s("insert")),
        ("line", num(line)),
        ("text", s(content)),
    ])
}

fn m_line(path: &str, kind: &str, line: usize) -> Value {
    obj(vec![
        ("path", s(path)),
        ("kind", s(kind)),
        ("line", num(line)),
    ])
}

/// The mutation that undoes operation `op` (if it has an undo): the anchor text back in place.
fn undo(op: &Op<'_>, rng: Option<&mut Rng>) -> Option<Value> {
    let line = |rng: Option<&mut Rng>| rng.map_or(0, |r| r.index(2000));
    match op {
        Op::Sub { path, old, new, .. } => Some(m_replace(path, new, old)),
        Op::Cut { path, old, .. } => Some(m_insert(path, line(rng), old)),
        Op::Drop { path, sig, .. } => {
            let indent = &sig[..sig.len() - sig.trim_start().len()];
            let block = if sig.trim_end().ends_with([';', ',']) {
                format!("{indent}/// restored\n{sig}")
            } else {
                format!(
                    "{indent}/// restored\n{indent}#[inline]\n{sig}\n{indent}    body();\n{indent}}}"
                )
            };
            Some(m_insert(path, line(rng), &block))
        }
        Op::Rm(_) => None,
    }
}

// ------------------------------------------------------------------------------------------
// Seeds

const LOCK: &str = "version = 4\n\n[[package]]\nname = \"a\"\nversion = \"1.0.0\"\n\n[[package]]\nname = \"b\"\nversion = \"2.0.0\"\ndependencies = [\n \"a\",\n]\n";

const RUST: &str = "use std::io;\n\n/// Docs of keep.\nfn keep() {\n    one();\n}\n\n/// Docs of gone.\n#[inline]\nfn gone(x: u8) -> u8 {\n    if x > 0 {\n        x\n    } else {\n        0\n    }\n}\n\nmod tests {\n    #[test]\n    fn test_gone() {\n        assert_eq!(gone(1), 1);\n    }\n\n    #[test]\n    fn test_keep() {\n        keep();\n    }\n}\n";

fn seeds() -> Vec<Case> {
    let ops = rtk_trim::operations();
    let mut cases: Vec<(String, Value)> = Vec::new();
    let mut add = |id: &str, data: Value| cases.push((format!("seed:{id}"), data));

    add("removed-paths", obj(vec![("mode", s("removed-paths"))]));
    add(
        "conflicts-test",
        conflicts_case(
            "DU rtk/engine/src/lib.rs\nDU rtk/src/core/telemetry.rs\nUU rtk/src/lib.rs\n\
             DU rtk/src/core/telemetry_x.rs\nDU other/engine/x.rs\nDU \"rtk/engine/odd name.rs\"\n M rtk/Cargo.toml",
        ),
    );
    add("conflicts-empty", conflicts_case(""));
    add(
        "conflicts-odd-breaks",
        conflicts_case(
            "DU rtk/engine/a.rs\u{c}DU rtk/plugin\u{85}DU rtk/proxy/x\r\nDU rtk/examples\r",
        ),
    );

    // The real tree as it is: a fixed point.
    add("trim-head", trim_case(vec![], &[], &[]));
    add(
        "trim-extras",
        trim_case(
            vec![],
            &[
                ("engine/src/lib.rs", "x"),
                ("src/core/telemetry.rs", "x"),
                ("hooks/codex/a", "x"),
                ("scripts/x.sh", "x"),
            ],
            &[("plugin", "nowhere"), ("proxy/link", "../engine")],
        ),
    );
    // Every operation undone (the stand-in for the pristine upstream tree), in reverse order.
    let undone: Vec<Value> = ops.iter().rev().filter_map(|op| undo(op, None)).collect();
    add("trim-undo-all", trim_case(undone, &[], &[]));
    add(
        "trim-undo-all-but-drops",
        trim_case(
            ops.iter()
                .rev()
                .filter(|op| !matches!(op, Op::Drop { .. }))
                .filter_map(|op| undo(op, None))
                .collect(),
            &[],
            &[],
        ),
    );
    // Each kind of operation undone alone, one case per file (the first of each kind).
    let mut seen: Vec<String> = Vec::new();
    for op in &ops {
        let key = match op {
            Op::Sub { path, .. } => format!("sub:{path}"),
            Op::Cut { path, .. } => format!("cut:{path}"),
            Op::Drop { path, .. } => format!("drop:{path}"),
            Op::Rm(_) => continue,
        };
        if seen.contains(&key) {
            continue;
        }
        seen.push(key.clone());
        if let Some(m) = undo(op, None) {
            add(&format!("undo-{key}"), trim_case(vec![m], &[], &[]));
        }
    }
    // Failures of the real trim: each seed undoes one operation and then breaks its anchor, or
    // doubles it. `tests::the_failure_seeds_fail_as_named` pins the message of each.
    let first_sub = ops.iter().find(|o| matches!(o, Op::Sub { .. }));
    let first_cut = ops.iter().find(|o| matches!(o, Op::Cut { .. }));
    let lib_drop = ops
        .iter()
        .find(|o| matches!(o, Op::Drop { path, .. } if *path == "src/lib.rs"));
    if let Some(sub @ Op::Sub { path, .. }) = first_sub {
        // The anchor text is restored, then one word of it is changed: the anchor is missing.
        add(
            "trim-changed-anchor",
            trim_case(
                vec![
                    undo(sub, None).expect("a sub undoes"),
                    m_replace(path, "engine", "engin3"),
                ],
                &[],
                &[],
            ),
        );
    }
    if let Some(drop @ Op::Drop { path, .. }) = lib_drop {
        // The dropped item is restored twice: its signature line occurs twice.
        add(
            "trim-duplicated-line",
            trim_case(
                vec![
                    undo(drop, None).expect("a drop undoes"),
                    // Line 2 of the restored block: `/// restored`, `#[inline]`, the signature.
                    m_line(path, "dup_line", 2),
                ],
                &[],
                &[],
            ),
        );
        // The restored item's signature is changed (the name is still there).
        if let Op::Drop { sig, .. } = drop {
            add(
                "trim-fail-drop-signature",
                trim_case(
                    vec![m_insert(path, 0, &format!("{sig} // changed\n}}"))],
                    &[],
                    &[],
                ),
            );
        }
    }
    if let Some(cut @ Op::Cut { path, old, .. }) = first_cut {
        // The cut text is restored twice: the anchor occurs twice.
        add(
            "trim-fail-ambiguous-cut",
            trim_case(
                vec![
                    undo(cut, None).expect("a cut undoes"),
                    undo(cut, None).expect("a cut undoes"),
                ],
                &[],
                &[],
            ),
        );
        // The cut text is restored with one character added: the anchor is gone, its token is not.
        add(
            "trim-fail-cut-token",
            trim_case(
                vec![m_insert(path, 0, &old.replacen('\n', "x\n", 1))],
                &[],
                &[],
            ),
        );
    }
    add(
        "trim-append",
        trim_case(vec![m_append("src/lib.rs", "\n// tail\n")], &[], &[]),
    );

    // The unit tests of tests/test_rtk_trim.py.
    add(
        "prim-sub-once",
        prim_case(
            &[("f.rs", "a\nold line\nb\n")],
            vec![
                op_sub("f.rs", "old line\n", "new line\n", None),
                op_sub("f.rs", "old line\n", "new line\n", None),
            ],
        ),
    );
    add(
        "prim-sub-extends",
        prim_case(
            &[("Cargo.toml", "[lints.rust]\nwarnings = \"deny\"\n")],
            vec![
                op_sub(
                    "Cargo.toml",
                    "[lints.rust]\nwarnings = \"deny\"\n",
                    "[lints.rust]\nwarnings = \"deny\"\nx = 1\n",
                    None,
                );
                2
            ],
        ),
    );
    add(
        "prim-sub-missing",
        prim_case(
            &[("f.rs", "something else\n")],
            vec![op_sub("f.rs", "old line\nmore\n", "new line\n", None)],
        ),
    );
    add(
        "prim-sub-changed-anchor",
        prim_case(
            &[("f.rs", "enum E {\n    A,\n    New,\n    B,\n}\n")],
            vec![op_sub(
                "f.rs",
                "enum E {\n    A,\n    B,\n}\n",
                "enum E {\n    A,\n}\n// trimmed\n",
                None,
            )],
        ),
    );
    add(
        "prim-sub-ambiguous",
        prim_case(
            &[("f.rs", "x\nx\n")],
            vec![op_sub("f.rs", "x\n", "y\n", None)],
        ),
    );
    add(
        "prim-sub-new-inside-old",
        prim_case(
            &[("f.rs", "a\nb\n")],
            vec![op_sub("f.rs", "a\nb\n", "a\n", None)],
        ),
    );
    add(
        "prim-sub-done-marker",
        prim_case(
            &[("f.rs", "if x {\n    old();\n} else {\n")],
            vec![
                op_sub(
                    "f.rs",
                    "if x {\n    old();\n",
                    "if x {\n    mid();\n",
                    Some("mid();"),
                ),
                op_sub(
                    "f.rs",
                    "    mid();\n} else {\n",
                    "    final_call();\n} else {\n",
                    None,
                ),
                op_sub(
                    "f.rs",
                    "if x {\n    old();\n",
                    "if x {\n    mid();\n",
                    Some("mid();"),
                ),
            ],
        ),
    );
    add(
        "prim-sub-done-final",
        prim_case(
            &[("f.rs", "if x {\n    final_call();\n} else {\n")],
            vec![op_sub(
                "f.rs",
                "if x {\n    old();\n",
                "if x {\n    mid();\n",
                Some("final_call();"),
            )],
        ),
    );
    add(
        "prim-sub-no-file",
        prim_case(&[], vec![op_sub("nope.rs", "a", "b", None)]),
    );
    add(
        "prim-cut",
        prim_case(
            &[("f.rs", "a\npub mod telemetry;\nb\n")],
            vec![
                op_cut("f.rs", "pub mod telemetry;\n", "mod telemetry"),
                op_cut("f.rs", "pub mod telemetry;\n", "mod telemetry"),
            ],
        ),
    );
    add(
        "prim-cut-token-present",
        prim_case(
            &[("f.rs", "a\npub(crate) mod telemetry;\nb\n")],
            vec![op_cut("f.rs", "pub mod telemetry;\n", "mod telemetry")],
        ),
    );
    add(
        "prim-cut-ambiguous",
        prim_case(&[("f.rs", "x\nx\n")], vec![op_cut("f.rs", "x\n", "x")]),
    );
    add(
        "prim-drop-item",
        prim_case(
            &[("f.rs", RUST)],
            vec![
                op_drop("f.rs", "fn gone(x: u8) -> u8 {", None),
                op_drop("f.rs", "fn gone(x: u8) -> u8 {", None),
            ],
        ),
    );
    add(
        "prim-drop-indented",
        prim_case(
            &[("f.rs", RUST)],
            vec![op_drop("f.rs", "    fn test_gone() {", None)],
        ),
    );
    add(
        "prim-drop-changed-signature",
        prim_case(
            &[(
                "f.rs",
                &RUST.replace("fn gone(x: u8) -> u8 {", "fn gone(x: u16) -> u16 {"),
            )],
            vec![op_drop("f.rs", "fn gone(x: u8) -> u8 {", None)],
        ),
    );
    add(
        "prim-drop-duplicate",
        prim_case(
            &[("f.rs", "fn a() {\n}\nfn a() {\n}\n")],
            vec![op_drop("f.rs", "fn a() {", None)],
        ),
    );
    add(
        "prim-drop-no-closer",
        prim_case(
            &[("f.rs", "    fn a() {\n        x();\nfn b() {}\n")],
            vec![op_drop("f.rs", "    fn a() {", None)],
        ),
    );
    add(
        "prim-drop-const-fn-quirk",
        prim_case(
            &[("f.rs", "const fn foo() {\n}\n\nfn fn() {\n}\n")],
            vec![op_drop("f.rs", "pub const fn foo() {", None)],
        ),
    );
    add(
        "prim-drop-gone-token",
        prim_case(
            &[("f.rs", "fn a() {\n}\n")],
            vec![op_drop("f.rs", "#[x] weird_signature", Some("weird"))],
        ),
    );
    add(
        "prim-drop-needs-gone",
        prim_case(
            &[("f.rs", "fn a() {\n}\n")],
            vec![op_drop("f.rs", "#[x] weird_signature", None)],
        ),
    );
    add(
        "prim-rm",
        prim_case(
            &[("a.txt", "a"), ("d/sub/b.txt", "b")],
            vec![
                op_rm(&["a.txt", "d", "never-there"]),
                op_rm(&["a.txt", "d"]),
            ],
        ),
    );

    // Non-ASCII beside a dropped name. A combining mark: Python's `\w` does not count it, and
    // Rust fails closed the same way, so both stop. A letter: Python counts it as part of the
    // name (the item is gone), Rust stops (fail closed), which the divergence entry allows.
    add(
        "nonascii-mark",
        prim_case(
            &[("f.rs", "fn gone\u{93e}() {\n}\n")],
            vec![op_drop("f.rs", "fn gone(x: u8) {", None)],
        ),
    );
    add(
        "nonascii-before-keyword",
        prim_case(
            &[("f.rs", "\u{e9}fn gone() {\n}\n")],
            vec![op_drop("f.rs", "fn gone(x: u8) {", None)],
        ),
    );
    add(
        "nonascii-letter",
        prim_case(
            &[("f.rs", "fn gone\u{e9}() {\n}\n")],
            vec![op_drop("f.rs", "fn gone(x: u8) {", None)],
        ),
    );

    // The lock guard.
    add("lock-same", lock_case(LOCK, LOCK));
    add(
        "lock-pruned",
        lock_case(
            LOCK,
            "version = 4\n\n[[package]]\nname = \"a\"\nversion = \"1.0.0\"\n",
        ),
    );
    add(
        "lock-changed",
        lock_case(LOCK, &LOCK.replace("\"2.0.0\"", "\"2.0.1\"")),
    );
    add(
        "lock-crlf",
        lock_case(&LOCK.replace('\n', "\r\n"), &LOCK.replace('\n', "\r")),
    );

    // The `../crates/` allowlist (m-14). Python has none: the oracle runs Python when the
    // allowlist allows nothing and its own reference of the rule when it allows something.
    let (before, closure) = (allow_before(), allow_after(&[]));
    let shell_path = "../crates/anthology-shell";
    let shell = |name: &str| format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n");
    let valid: &[(&str, &str)] = &[(
        "crates/anthology-shell/Cargo.toml",
        &shell("anthology-shell"),
    )];
    add(
        "allow-closure",
        allow_case(&before, &closure, Some(&manifest(shell_path)), valid),
    );
    add(
        "allow-closure-trailing-slash",
        allow_case(
            &before,
            &closure,
            Some(&manifest("../crates/anthology-shell/")),
            valid,
        ),
    );
    add(
        "allow-closure-pruned",
        allow_case(
            &before,
            &closure.replace(&lp("a", "1.0.0", true, &[]), ""),
            Some(&manifest(shell_path)),
            valid,
        ),
    );
    add(
        "allow-closure-version-change",
        allow_case(
            &before.replace(
                &lp("regex", "1.0.0", true, &[]),
                &lp("regex", "0.9.0", true, &[]),
            ),
            &closure,
            Some(&manifest(shell_path)),
            valid,
        ),
    );
    add(
        "allow-unrelated",
        allow_case(
            &before,
            &allow_after(&[lp("evil", "6.6.6", true, &[])]),
            Some(&manifest(shell_path)),
            valid,
        ),
    );
    add(
        "allow-dotdot",
        allow_case(
            &before,
            &closure,
            Some(&manifest("../crates/../x")),
            &[("x/Cargo.toml", &shell("anthology-shell"))],
        ),
    );
    add(
        "allow-outside-crates",
        allow_case(
            &before,
            &closure,
            Some(&manifest("../elsewhere/anthology-shell")),
            &[(
                "elsewhere/anthology-shell/Cargo.toml",
                &shell("anthology-shell"),
            )],
        ),
    );
    add(
        "allow-bare-crates",
        allow_case(&before, &closure, Some(&manifest("../crates/")), valid),
    );
    add(
        "allow-unreadable-crate-manifest",
        allow_case(&before, &closure, Some(&manifest(shell_path)), &[]),
    );
    add(
        "allow-nameless-crate-manifest",
        allow_case(
            &before,
            &closure,
            Some(&manifest(shell_path)),
            &[("crates/anthology-shell/Cargo.toml", "[lib]\npath = \"x\"\n")],
        ),
    );
    add(
        "allow-other-crate-name",
        allow_case(
            &before,
            &closure,
            Some(&manifest(shell_path)),
            &[("crates/anthology-shell/Cargo.toml", &shell("other"))],
        ),
    );
    add(
        "allow-registry-package-of-the-same-name",
        allow_case(
            &before,
            &closure.replace(
                &lp("anthology-shell", "0.1.0", false, &["fancy-regex", "regex"]),
                &lp("anthology-shell", "0.1.0", true, &["fancy-regex", "regex"]),
            ),
            Some(&manifest(shell_path)),
            valid,
        ),
    );
    add(
        "allow-no-rtk-manifest",
        allow_case(&before, &closure, None, valid),
    );

    cases
        .into_iter()
        .map(|(id, data)| Case::new(id, data))
        .collect()
}

// ------------------------------------------------------------------------------------------
// Generators

const NAMES: &[&str] = &[
    "gone",
    "gone2",
    "gone_more",
    "keep",
    "other",
    "test_gone",
    "a",
    "x1",
];
const KEYWORDS: &[&str] = &[
    "fn", "fn", "fn", "struct", "enum", "mod", "const", "static", "type",
];
const CLOSERS: &[&str] = &["}", "}", "}", "},", "});", "};"];
const ATTRS: &[&str] = &[
    "#[inline]",
    "/// Docs.",
    "// note",
    "#[test]",
    "#[derive(Debug)]",
];
const BODIES: &[&str] = &["one();", "x", "if a {", "    }", "let v = vec![1];", ""];

struct Item {
    sig: String,
}

/// A Rust-like file of a few items; returns the text and each item's first line.
fn gen_file(rng: &mut Rng) -> (String, Vec<Item>) {
    let mut out = String::new();
    let mut items = Vec::new();
    for _ in 0..1 + rng.index(5) {
        let indent = if rng.chance(0.3) { "    " } else { "" };
        for _ in 0..rng.index(3) {
            // Sometimes at the wrong indentation, which the leading-attribute scan must stop at.
            let extra = if rng.chance(0.1) { "    " } else { "" };
            out.push_str(&format!("{indent}{extra}{}\n", rng.pick(ATTRS)));
        }
        let keyword = *rng.pick(KEYWORDS);
        let name = *rng.pick(NAMES);
        let sig = match keyword {
            "const" | "static" | "type" => format!("{indent}{keyword} {name}: u8 = 1;"),
            "fn" => format!("{indent}fn {name}(x: u8) -> u8 {{"),
            _ => format!("{indent}{keyword} {name} {{"),
        };
        out.push_str(&sig);
        out.push('\n');
        if !sig.ends_with(';') {
            for _ in 0..rng.index(4) {
                out.push_str(&format!("{indent}    {}\n", rng.pick(BODIES)));
            }
            if rng.chance(0.93) {
                out.push_str(&format!("{indent}{}\n", rng.pick(CLOSERS)));
            }
        }
        for _ in 0..rng.index(3) {
            out.push('\n');
        }
        items.push(Item { sig });
    }
    (out, items)
}

/// A one- to three-line span of `content`, ending in a newline (an anchor that occurs).
fn span(rng: &mut Rng, content: &str) -> String {
    let lines: Vec<&str> = content.split_inclusive('\n').collect();
    if lines.is_empty() {
        return "zz\n".to_string();
    }
    let start = rng.index(lines.len());
    let end = (start + 1 + rng.index(3)).min(lines.len());
    let mut out: String = lines[start..end].concat();
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn alter(rng: &mut Rng, t: &str) -> String {
    match rng.below(4) {
        0 => format!("{t} "),
        1 => format!(" {t}"),
        2 => t.replacen('(', "( ", 1),
        _ => t.replace("fn ", "fn  "),
    }
}

fn gen_prim(rng: &mut Rng) -> Value {
    let (content, items) = gen_file(rng);
    let mut files = vec![("a.rs".to_string(), content.clone())];
    if rng.chance(0.2) {
        files.push(("dir/b.rs".to_string(), "fn b() {\n}\n".to_string()));
    }
    let mut ops = Vec::new();
    for _ in 0..1 + rng.index(4) {
        let path = if rng.chance(0.07) {
            "missing.rs"
        } else {
            "a.rs"
        };
        let op = match rng.below(10) {
            0..=3 => {
                let item = rng.pick(&items);
                let sig = match rng.below(10) {
                    0..=6 => item.sig.clone(),
                    7..=8 => alter(rng, &item.sig),
                    _ => "fn nothere() {".to_string(),
                };
                let gone = if rng.chance(0.15) {
                    Some(*rng.pick(&["gone", "fn gone", "zzzz", "keep"]))
                } else {
                    None
                };
                op_drop(path, &sig, gone)
            }
            4..=6 => {
                let old = match rng.below(5) {
                    0 => "no such text\n".to_string(),
                    1 => "    one();\n".to_string(),
                    _ => span(rng, &content),
                };
                let new = match rng.below(6) {
                    0 => String::new(),
                    1 => format!("{old}// more\n"),
                    2 => old
                        .lines()
                        .next()
                        .map_or(String::new(), |l| format!("{l}\n")),
                    3 => old.clone(),
                    _ => "// replaced\n".to_string(),
                };
                let done = match rng.below(5) {
                    0 => Some("// replaced"),
                    1 => Some(""),
                    2 => Some("fn keep"),
                    _ => None,
                };
                op_sub(path, &old, &new, done)
            }
            7..=8 => {
                let old = if rng.chance(0.15) {
                    "no such text\n".to_string()
                } else {
                    span(rng, &content)
                };
                let gone = *rng.pick(&["gone", "keep", "fn", "zzzz", "x1"]);
                op_cut(path, &old, gone)
            }
            _ => op_rm(&["a.rs", "dir", "dir/b.rs", "never"][rng.index(4)..][..1]),
        };
        // Often the same operation twice: the second must be a no-op (or fail the same way).
        if rng.chance(0.3) {
            ops.push(op.clone());
        }
        ops.push(op);
    }
    obj(vec![
        ("mode", s("prim")),
        (
            "files",
            obj(files.iter().map(|(k, v)| (k.as_str(), s(v))).collect()),
        ),
        ("ops", arr(ops)),
    ])
}

fn gen_trim(rng: &mut Rng) -> Value {
    let ops = rtk_trim::operations();
    let paths = touched_paths();
    let mut mutations = Vec::new();
    let mut extras: Vec<(String, String)> = Vec::new();
    let mut links: Vec<(String, String)> = Vec::new();
    for _ in 0..rng.index(4) {
        match rng.below(8) {
            0..=4 => {
                let op = rng.pick(&ops);
                if let Some(m) = undo(op, Some(rng)) {
                    mutations.push(m);
                }
            }
            5 => {
                let path = *rng.pick(&paths);
                let kind = *rng.pick(&["delete_line", "dup_line"]);
                mutations.push(m_line(path, kind, rng.index(1500)));
            }
            6 => {
                let path = *rng.pick(&paths);
                if rng.chance(0.7) {
                    mutations.push(m_replace(path, "    ", "   "));
                } else {
                    mutations.push(m_append(path, "\nfn appended() {\n}\n"));
                }
            }
            _ => {
                let removed = rtk_trim::removed_paths();
                let path = *rng.pick(&removed);
                if rng.chance(0.5) {
                    extras.push((format!("{path}/inner.rs"), "x".to_string()));
                } else {
                    links.push((path.to_string(), "nowhere".to_string()));
                }
            }
        }
    }
    // A link and a file cannot share a path; drop the file in that case.
    extras.retain(|(p, _)| {
        !links
            .iter()
            .any(|(l, _)| p == l || p.starts_with(&format!("{l}/")))
    });
    obj(vec![
        ("mode", s("trim")),
        ("mutations", arr(mutations)),
        (
            "extras",
            obj(extras.iter().map(|(k, v)| (k.as_str(), s(v))).collect()),
        ),
        (
            "links",
            obj(links.iter().map(|(k, v)| (k.as_str(), s(v))).collect()),
        ),
    ])
}

fn gen_lock_text(rng: &mut Rng, packages: &[(String, String, Vec<String>)]) -> String {
    let nl = *rng.pick(&["\n", "\n", "\n", "\r\n", "\r"]);
    let mut out = format!("# comment{nl}version = 4{nl}{nl}");
    for (name, version, deps) in packages {
        out.push_str(&format!("[[package]]{nl}"));
        let name_line = match rng.below(8) {
            0 => format!("name=\"{name}\""),
            1 => format!("name = \"{name}\" "),
            _ => format!("name = \"{name}\""),
        };
        let version_line = match rng.below(10) {
            0 => format!("version = '{version}'"),
            _ => format!("version = \"{version}\""),
        };
        if rng.chance(0.1) {
            out.push_str(&format!("{version_line}{nl}{name_line}{nl}"));
        } else {
            out.push_str(&format!("{name_line}{nl}{version_line}{nl}"));
        }
        if rng.chance(0.5) {
            out.push_str(&format!(
                "source = \"registry+https://example.invalid/index\"{nl}"
            ));
        }
        if !deps.is_empty() {
            out.push_str(&format!("dependencies = [{nl}"));
            for d in deps {
                out.push_str(&format!(" \"{d}\",{nl}"));
            }
            out.push_str(&format!("]{nl}"));
        }
        if rng.chance(0.1) {
            out.push_str(&format!("name = \"stray\"{nl}"));
        }
        out.push_str(nl);
    }
    if rng.chance(0.2) {
        out.push_str(&format!(
            "[metadata]{nl}name = \"meta\"{nl}version = \"9\"{nl}"
        ));
    }
    out
}

fn gen_lock(rng: &mut Rng) -> Value {
    const POOL: &[&str] = &[
        "a",
        "b",
        "c",
        "rtk",
        "serde",
        "regex",
        "anthology-shell",
        "é",
    ];
    const VERSIONS: &[&str] = &["1.0.0", "1.0.1", "2.0.0", "0.3.1-beta"];
    let mut before: Vec<(String, String, Vec<String>)> = Vec::new();
    for _ in 0..rng.index(6) {
        let name = (*rng.pick(POOL)).to_string();
        let version = (*rng.pick(VERSIONS)).to_string();
        let deps = (0..rng.index(3))
            .map(|_| (*rng.pick(POOL)).to_string())
            .collect();
        before.push((name, version, deps));
    }
    let mut after = before.clone();
    for _ in 0..rng.index(4) {
        match rng.below(4) {
            0 if !after.is_empty() => {
                let i = rng.index(after.len());
                after.remove(i);
            }
            1 if !after.is_empty() => {
                let i = rng.index(after.len());
                after[i].1 = (*rng.pick(VERSIONS)).to_string();
            }
            2 => after.push((
                (*rng.pick(POOL)).to_string(),
                (*rng.pick(VERSIONS)).to_string(),
                vec![],
            )),
            _ => {}
        }
    }
    let b = gen_lock_text(rng, &before);
    let a = gen_lock_text(rng, &after);
    lock_case(&b, &a)
}

fn gen_conflicts(rng: &mut Rng) -> Value {
    let removed = rtk_trim::removed_paths();
    let mut lines = Vec::new();
    for _ in 0..rng.index(8) {
        let code = *rng.pick(&["DU", "DU", "DU", "UU", " M", "??", "AA", "D ", "DU"]);
        let base = *rng.pick(&removed);
        let path = match rng.below(9) {
            0 => format!("rtk/{base}"),
            1 => format!("rtk/{base}/sub/x.rs"),
            2 => format!("rtk/{base}x"),
            3 => format!("other/{base}"),
            4 => format!("\"rtk/{base}\""),
            5 => format!("rtk/{}", rng.ascii_string(12)),
            6 => base.to_string(),
            _ => format!("rtk/{base}/"),
        };
        lines.push(format!("{code} {path}"));
    }
    let sep = *rng.pick(&["\n", "\n", "\r\n", "\r", "\u{c}", "\u{85}"]);
    let mut status = lines.join(sep);
    if rng.chance(0.5) {
        status.push_str(sep);
    }
    conflicts_case(&status)
}

/// A lock pair with an `rtk/Cargo.toml` that names (or fails to name) a `../crates/` path
/// package, and the crate manifest it points at.
fn gen_allow(rng: &mut Rng) -> Value {
    let path = *rng.pick(&[
        "../crates/anthology-shell",
        "../crates/anthology-shell",
        "../crates/anthology-shell",
        "../crates/anthology-shell/",
        "../crates/../x",
        "../crates/",
        "../elsewhere/anthology-shell",
        "crates/anthology-shell",
    ]);
    let mut before = vec![
        lp("rtk", "1.0.0", false, &["regex"]),
        lp("regex", "1.0.0", true, &[]),
        lp("a", "1.0.0", true, &[]),
    ];
    let shell_source = rng.chance(0.1);
    let mut after = vec![
        lp("rtk", "1.0.0", false, &["anthology-shell", "regex"]),
        lp("regex", "1.0.0", true, &[]),
        lp("a", "1.0.0", true, &[]),
        lp(
            "anthology-shell",
            "0.1.0",
            shell_source,
            &["fancy-regex", "regex"],
        ),
        lp("fancy-regex", "0.20.2", true, &["bit-set", "regex"]),
        lp("bit-set", "0.8.0", true, &["bit-vec"]),
        lp("bit-vec", "0.8.0", true, &[]),
    ];
    match rng.below(5) {
        0 => after.push(lp("evil", "6.6.6", true, &[])),
        1 => after.retain(|p| !p.contains("name = \"a\"")),
        2 => before[1] = lp("regex", "0.9.0", true, &[]),
        3 => after.push(lp("bit-vec", "0.9.0", true, &[])),
        _ => {}
    }
    if rng.chance(0.15) {
        before.push(lp("bit-vec", "0.8.0", true, &[]));
    }
    let crate_manifest = match rng.below(8) {
        0 => None,
        1 => Some("[lib]\npath = \"x\"\n".to_string()),
        2 => Some("[package]\nname = \"other\"\n".to_string()),
        _ => Some("[package]\nname = \"anthology-shell\"\nversion = \"0.1.0\"\n".to_string()),
    };
    let rel = format!(
        "{}/Cargo.toml",
        path.trim_start_matches("../").trim_end_matches('/')
    );
    let crates: Vec<(&str, &str)> = crate_manifest
        .iter()
        .map(|m| (rel.as_str(), m.as_str()))
        .collect();
    let manifest_text = if rng.chance(0.1) {
        None
    } else {
        Some(manifest(path))
    };
    allow_case(
        &lock_of(&before),
        &lock_of(&after),
        manifest_text.as_deref(),
        &crates,
    )
}

fn gen_case(rng: &mut Rng) -> Value {
    match rng.below(20) {
        0..=7 => gen_prim(rng),
        8..=10 => gen_lock(rng),
        11..=12 => gen_allow(rng),
        13..=15 => gen_conflicts(rng),
        _ => gen_trim(rng),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(id: &str) -> Observed {
        let want = format!("seed:{id}");
        let case = seeds()
            .into_iter()
            .find(|c| c.id == want)
            .unwrap_or_else(|| panic!("no seed {want}"));
        run(&case.data)
    }

    fn stderr(o: &Observed) -> String {
        String::from_utf8_lossy(&o.stderr).into_owned()
    }

    fn stdout(o: &Observed) -> String {
        String::from_utf8_lossy(&o.stdout).into_owned()
    }

    #[test]
    fn the_failure_seeds_fail_as_named() {
        for (id, message) in [
            ("trim-changed-anchor", "anchor missing: "),
            ("trim-duplicated-line", "signature occurs 2 times"),
            ("trim-fail-drop-signature", "signature changed?"),
            ("trim-fail-ambiguous-cut", "anchor occurs 2 times"),
            ("trim-fail-cut-token", "still present"),
        ] {
            let o = seed(id);
            assert_eq!(o.exit, 1, "{id}: {} {}", stdout(&o), stderr(&o));
            assert!(stderr(&o).contains(message), "{id}: {}", stderr(&o));
        }
    }

    #[test]
    fn the_head_seeds_have_nothing_to_do_and_undo_all_does_everything() {
        for id in ["trim-head", "trim-append"] {
            let o = seed(id);
            assert_eq!((o.exit, stdout(&o).as_str()), (0, "no changes\n"), "{id}");
        }
        let o = seed("trim-undo-all");
        assert_eq!(o.exit, 0, "{}", stderr(&o));
        assert!(stdout(&o).lines().count() > 200, "{}", stdout(&o));
    }

    #[test]
    fn the_allowlist_seeds_allow_only_the_closure() {
        for id in [
            "allow-closure",
            "allow-closure-trailing-slash",
            "allow-closure-pruned",
            "allow-closure-version-change",
        ] {
            let o = seed(id);
            assert_eq!(o.exit, 0, "{id}: {}", stderr(&o));
            assert!(
                stdout(&o)
                    .contains("packages added by the ../crates/ allowlist: anthology-shell 0.1.0"),
                "{id}: {}",
                stdout(&o)
            );
        }
        let o = seed("allow-unrelated");
        assert_eq!(o.exit, 1);
        assert!(
            stderr(&o).ends_with("packages: evil 6.6.6\n"),
            "{}",
            stderr(&o)
        );
        for id in [
            "allow-dotdot",
            "allow-outside-crates",
            "allow-bare-crates",
            "allow-unreadable-crate-manifest",
            "allow-nameless-crate-manifest",
            "allow-other-crate-name",
            "allow-registry-package-of-the-same-name",
            "allow-no-rtk-manifest",
        ] {
            let o = seed(id);
            assert_eq!(o.exit, 1, "{id}: {}", stdout(&o));
            assert!(
                stderr(&o).contains("anthology-shell 0.1.0"),
                "{id}: {}",
                stderr(&o)
            );
        }
    }
}
