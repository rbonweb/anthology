//! The `memory.portable` difftest surface (MX1): `anthology_memory::portable` (`export`,
//! `plan_import`, `apply_import`, `main`) against `memory_portable.py`, oracle
//! `port/difftest/oracle/memory_portable.py`.
//!
//! A case is `{"files": {rel: text}, "bytes": {rel: hex}, "steps": [...]}`. Both sides make a
//! fresh folder `<ROOT>` with two machines, `a/user` and `b/user`, and `plain/`, the working
//! folder; then write `files` (UTF-8) and `bytes` under it. The archives the generator builds
//! (with `anthology_testkit::mx`, the way `test_memory_portable.py` builds them) arrive as
//! `bytes` under `in/`, so both sides read the identical file. A step runs on machine
//! `"home"` (`a` by default: HOME `<ROOT>/<m>/user`, `LANG=C.UTF-8`, PATH) and is one of:
//!
//! - `{"export": {"out": rel | null, "today": d}}`: `export(<ROOT>/plain, out, today)`;
//! - `{"plan": {"archive": rel, "today": d}}`: `plan_import`; a plan it returns is kept;
//! - `{"apply": {"plan"?: P, "edits"?: [[path, value] | [path]]}}`: `apply_import` of `P` (or
//!   of a copy of the kept plan, `null` if none), after each edit sets (or deletes) the value at
//!   a path of keys and indices (a path that does not exist is skipped);
//! - `{"cli": [args]}`: `main(args)` with the working folder `<ROOT>/plain`;
//! - `{"write": {rel: text}}`, `{"bytes": {rel: hex}}`, `{"remove": [rel]}`,
//!   `{"symlink": {rel: target}}`, `{"mkdir": [rel]}`.
//!
//! `<ROOT>` in `P`, the edits and the CLI arguments is the folder; `<TODAY>` in CLI arguments is
//! each side's own `date.today()`.
//!
//! The result is one transcript on stdout: per call step `## <n> export|plan|apply`, then the
//! result as `json.dumps(indent=2, sort_keys=True)` or `error <Type>: <message>`; per CLI step
//! `## <n> $ <args>`, `exit N`, `--- stdout`, its stdout, `--- stderr`, its stderr. Then
//! `=== tree ===` and every entry under `<ROOT>`, sorted: `d <mode> <rel>`, `l <rel> ->
//! <target>`, `o <rel>`, or `f <mode> <rel> <content>`, where the content of an input archive
//! (under `in/`) is its sha256, of any other `*.tar.gz` (an export) the gzip header's 10 bytes
//! and the length and sha256 of the DECOMPRESSED tar stream (D-5: the deflate bytes are not
//! compared, the tar bytes are, exactly), and of anything else its text (JSON string) or hex.
//! `<ROOT>` and today's date (`<TODAY>`, also inside the tar bytes) are normalized, and so is
//! the tar filter's random temporary folder in a `File name too long` text ([`too_long`]).
//!
//! The Linux oracle image has no git and `plain/` is never in a repository: every repo or
//! agent note takes the "not an approved repo" path (skipped, or a forged repo plan refused),
//! and Rust runs with a `Git` that is missing too.
//!
//! Divergences: the oracle marks D-1 where Python raises an exception the port does not name
//! (a hand-forged plan of the wrong shape), D-4 where argparse output was shown (the parser is
//! named `anthology memory`, as the port's CLI) and D-7 where the port's bound on GNU sparse
//! numbers refused the archive; the D-2 hook hides zlib's own reason after `Error -3 while
//! decompressing data: ` ([`LIBTEXT_PREFIXES`]); the D-5 hook replaces an exported archive's
//! own sha256 (the tree's `raw:<hex>`, and wherever a plan prints it) by `<export N>`.
//!
//! Case ids carry their category, `seed:<n>-<label>` and `gen:<n>-<category>` (`flow`, `cli`,
//! `archive`, `corrupt`, `forged`, and `tarhdr`: the uncompressed tar mutated header by header,
//! then gzipped), so divergence entries are case-scoped.

use std::ffi::OsString;
use std::fs;
use std::io::Read as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use anthology_core::Env;
use anthology_core::git::{Git, GitOut};
use anthology_memory::portable::{self, Error};
use anthology_pycompat::json::{self, Int, Map, Value};
use anthology_testkit::mx::{self, Spec};

use crate::case::{Case, Rng, Surface};
use crate::normalize::Normalizer;
use crate::observe::{Observed, from_oracle};
use crate::oracle::Oracle;

/// The surface name (plan §1.4).
pub const NAME: &str = "memory.portable";

/// Whether `difftest --all` runs this surface on macOS too (`ci/jobs/difftest-macos.sh`); set
/// `false` only for a surface that cannot run there, and say why.
pub const ON_MACOS: bool = true;

/// The registered implementation.
pub fn surface() -> Option<Box<dyn crate::case::Surface>> {
    Some(Box::new(Portable))
}

/// What the D-2 hook keeps of a line (GC-16 D-2): zlib's own reason after `Error -3 while
/// decompressing data: `, raised by a read of the data (bare) or of a header (`tarfile` wraps
/// it as `zlib error: `). gzip's own texts and every tarfile text are compared.
const LIBTEXT_PREFIXES: [&str; 2] = [
    "not a readable memory archive: Error -3 while decompressing data: ",
    "not a readable memory archive: zlib error: Error -3 while decompressing data: ",
];

/// The D-2 hook.
fn d2() -> Normalizer {
    LIBTEXT_PREFIXES
        .iter()
        .fold(Normalizer::new(), |n, p| n.d2_prefix(p))
}

/// `tarfile.data_filter`'s ENAMETOOLONG text names a path under a random `/tmp/tmpXXXXXXXX`
/// (8 characters), which the port prints with the X's; and the errno is the host's (Linux 36,
/// macOS 63), while the Rust side may run on macOS against the Linux oracle. On macOS the
/// oracle's folder is `/private/tmp/tmp…`: the oracle runs with no `TMPDIR`, so `tempfile`
/// picks `/tmp`, and `data_filter` realpaths it through macOS's `/tmp -> private/tmp` link.
/// Both sides.
fn too_long(o: &mut Observed) {
    const AT: &str = "File name too long: '/tmp/tmp";
    let text = String::from_utf8_lossy(&o.stdout)
        .replace(
            "[Errno 63] File name too long",
            "[Errno 36] File name too long",
        )
        .replace("File name too long: '/private/tmp/tmp", AT);
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(i) = rest.find(AT) {
        out.push_str(&rest[..i + AT.len()]);
        rest = &rest[i + AT.len()..];
        let random = rest.as_bytes().get(..9).is_some_and(|b| {
            b[8] == b'/'
                && b[..8]
                    .iter()
                    .all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
        });
        if random {
            out.push_str("XXXXXXXX");
            rest = &rest[8..];
        }
    }
    out.push_str(rest);
    o.stdout = out.into_bytes();
}

struct Portable;

impl Surface for Portable {
    fn name(&self) -> &'static str {
        NAME
    }

    fn seeds(&self) -> Vec<Case> {
        seeds()
            .into_iter()
            .enumerate()
            .map(|(i, (label, v))| Case::new(format!("seed:{}-{label}", i + 1), v))
            .collect()
    }

    fn generate(&self, rng: &mut Rng, n: usize) -> Vec<Case> {
        (0..n)
            .map(|i| {
                let (cat, v) = generate(rng);
                Case::new(format!("gen:{}-{cat}", i + 1), v)
            })
            .collect()
    }

    fn run_python(&self, c: &Case, oracle: &mut Oracle) -> Observed {
        let mut o = from_oracle(&oracle.call("memory_portable", &c.data));
        if d5_export_shas(&mut o) {
            oracle.mark("D-5");
        }
        too_long(&mut o);
        d2().apply_python(&mut o, oracle);
        o
    }

    fn run_rust(&self, c: &Case) -> Observed {
        let mut o = run_rust(&c.data);
        d5_export_shas(&mut o);
        too_long(&mut o);
        d2().apply(&mut o);
        o
    }
}

/// The D-5 hook: an exported archive's own sha256 (the tree line's `raw:<hex>`, and wherever
/// a plan or the CLI shows it) depends on the deflate bytes, so it becomes `<export N>`, N the
/// archive's place in the tree. Its header and decompressed tar stream are still compared.
/// Returns whether it changed anything.
fn d5_export_shas(o: &mut Observed) -> bool {
    let mut text = String::from_utf8_lossy(&o.stdout).into_owned();
    let tree_at = text.find("=== tree ===\n").unwrap_or(text.len());
    let shas: Vec<String> = text[tree_at..]
        .lines()
        .filter(|l| l.starts_with("f "))
        .filter_map(|l| l.rsplit_once(" raw:").map(|(_, h)| h.to_string()))
        .collect();
    for (n, h) in shas.iter().enumerate() {
        text = text
            .replace(&format!(" raw:{h}"), "")
            .replace(h, &format!("<export {}>", n + 1));
    }
    o.stdout = text.into_bytes();
    !shas.is_empty()
}

// ---- the Rust side ---------------------------------------------------------------------------

/// The oracle image has no git: every call fails as a missing git does.
struct NoGit;

impl Git for NoGit {
    fn run(&self, _env: &Env, _cwd: &Path, _args: &[&str]) -> Option<GitOut> {
        None
    }
}

fn field<'a>(v: &'a Value, k: &str) -> Option<&'a Value> {
    v.as_object().and_then(|m| m.get(k))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).expect("hex"))
        .collect()
}

fn sha(b: &[u8]) -> String {
    anthology_testkit::release::sha256_hex(b)
}

fn replace_bytes(b: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i..].starts_with(from) {
            out.extend_from_slice(to);
            i += from.len();
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    out
}

fn write_files(root: &Path, files: Option<&Value>, binary: bool) {
    let Some(m) = files.and_then(Value::as_object) else {
        return;
    };
    for (rel, v) in m.iter() {
        let path = root.join(rel);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).expect("mkdir");
        }
        let s = v.as_str().unwrap_or("");
        let bytes = if binary {
            unhex(s)
        } else {
            s.as_bytes().to_vec()
        };
        fs::write(&path, bytes).expect("write");
    }
}

fn file_step(root: &Path, step: &Value) -> bool {
    let root_s = root.display().to_string();
    if let Some(f) = field(step, "write") {
        write_files(root, Some(f), false);
    } else if let Some(f) = field(step, "bytes") {
        write_files(root, Some(f), true);
    } else if let Some(gone) = field(step, "remove").and_then(Value::as_array) {
        for rel in gone {
            let _ = fs::remove_file(root.join(rel.as_str().unwrap_or("")));
        }
    } else if let Some(links) = field(step, "symlink").and_then(Value::as_object) {
        for (rel, target) in links.iter() {
            let path = root.join(rel);
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir).expect("mkdir");
            }
            let target = target.as_str().unwrap_or("").replace("<ROOT>", &root_s);
            std::os::unix::fs::symlink(target, path).expect("symlink");
        }
    } else if let Some(dirs) = field(step, "mkdir").and_then(Value::as_array) {
        for rel in dirs {
            fs::create_dir_all(root.join(rel.as_str().unwrap_or(""))).expect("mkdir");
        }
    } else {
        return false;
    }
    true
}

/// `<ROOT>` in every string of `v` replaced by the folder.
fn subst(v: &Value, root: &str) -> Value {
    match v {
        Value::Str(s) => Value::Str(s.replace("<ROOT>", root)),
        Value::Array(a) => Value::Array(a.iter().map(|x| subst(x, root)).collect()),
        Value::Object(m) => {
            let mut out = Map::new();
            for (k, x) in m.iter() {
                out.insert(k.to_string(), subst(x, root));
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

/// The container a path step leads to (Python's `cur[key]`); `None` where Python raises
/// KeyError, IndexError or TypeError.
fn step_into<'a>(cur: &'a mut Value, key: &Value) -> Option<&'a mut Value> {
    match (cur, key) {
        (Value::Object(m), Value::Str(k)) => m.get_mut(k),
        (Value::Array(a), Value::Int(Int::Small(i))) if *i >= 0 => a.get_mut(*i as usize),
        _ => None,
    }
}

/// `_edit` of the oracle: `[path, value]` sets, `[path]` deletes, a missing path is skipped.
fn edit(plan: &mut Value, edits: Option<&Value>) {
    for e in edits
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        let Some(e) = e.as_array() else { continue };
        let Some(path) = e.first().and_then(Value::as_array) else {
            continue;
        };
        let Some((last, init)) = path.split_last() else {
            continue;
        };
        let mut cur = Some(&mut *plan);
        for k in init {
            cur = cur.and_then(|c| step_into(c, k));
        }
        let Some(cur) = cur else { continue };
        match (cur, last, e.get(1)) {
            (Value::Object(m), Value::Str(k), Some(v)) => m.insert(k.clone(), v.clone()),
            (Value::Object(m), Value::Str(k), None) => {
                m.remove(k);
            }
            (Value::Array(a), Value::Int(Int::Small(i)), v)
                if *i >= 0 && (*i as usize) < a.len() =>
            {
                match v {
                    Some(v) => a[*i as usize] = v.clone(),
                    None => {
                        a.remove(*i as usize);
                    }
                }
            }
            _ => {}
        }
    }
}

fn err_line(e: &Error) -> String {
    let (ty, msg) = match e {
        Error::Rejected(m) => ("ImportRejected".to_string(), m.clone()),
        Error::Refused(m) => ("ExportRefused".to_string(), m.clone()),
        Error::Value(m) => ("ValueError".to_string(), m.clone()),
        Error::Os(x) => (x.type_name().to_string(), x.py_str()),
    };
    format!("error {ty}: {msg}\n")
}

fn result_text(r: &Result<Map, Error>) -> String {
    match r {
        Ok(m) => format!(
            "{}\n",
            json::dumps(&Value::Object(m.clone()), &json::INDENT2_SORTED)
        ),
        Err(e) => err_line(e),
    }
}

fn content(rel: &str, data: &[u8], today: &str) -> String {
    if rel.starts_with("in/") {
        return format!("sha256:{}", sha(data));
    }
    if rel.ends_with(".tar.gz") {
        let mut tar = Vec::new();
        return match flate2::read::MultiGzDecoder::new(data).read_to_end(&mut tar) {
            Ok(_) => {
                let tar = replace_bytes(&tar, today.as_bytes(), b"<TODAY>");
                format!(
                    "gzip:{} tar:{}:{} raw:{}",
                    hex(&data[..10.min(data.len())]),
                    tar.len(),
                    sha(&tar),
                    sha(data)
                )
            }
            Err(_) => format!("sha256:{}", sha(data)),
        };
    }
    match std::str::from_utf8(data) {
        Ok(t) => format!(
            "text:{}",
            json::dumps(&Value::Str(t.to_string()), &json::PY_DEFAULT_UNICODE)
        ),
        Err(_) => format!("hex:{}", hex(data)),
    }
}

/// `os.walk` of `root` (symlinks listed, not followed), every entry, sorted.
fn tree(root: &Path, today: &str) -> String {
    fn walk(dir: &Path, rel: &str, out: &mut Vec<String>) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let r = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            let p = e.path();
            if fs::symlink_metadata(&p).is_ok_and(|m| m.is_dir()) {
                walk(&p, &r, out);
            }
            out.push(r);
        }
    }
    let mut rels = Vec::new();
    walk(root, "", &mut rels);
    rels.sort();
    let mut lines = String::new();
    for rel in rels {
        let p = root.join(&rel);
        let Ok(m) = fs::symlink_metadata(&p) else {
            continue;
        };
        let mode = format!("{:04o}", m.permissions().mode() & 0o7777);
        let line = if m.file_type().is_symlink() {
            let t = fs::read_link(&p).unwrap_or_default();
            format!("l {rel} -> {}", t.display())
        } else if m.is_dir() {
            format!("d {mode} {rel}")
        } else if m.is_file() {
            let data = fs::read(&p).unwrap_or_default();
            format!("f {mode} {rel} {}", content(&rel, &data, today))
        } else {
            format!("o {rel}")
        };
        lines.push_str(&line);
        lines.push('\n');
    }
    lines
}

fn run_rust(case: &Value) -> Observed {
    let dir = tempfile::Builder::new()
        .prefix("mx1-rust-")
        .tempdir()
        .expect("tempdir");
    let tmp = fs::canonicalize(dir.path()).expect("realpath");
    let root_s = tmp.display().to_string();
    let today = anthology_core::clock::today_local().iso();
    for d in ["a/user", "b/user", "plain"] {
        fs::create_dir_all(tmp.join(d)).expect("mkdir");
    }
    let cwd = tmp.join("plain");
    write_files(&tmp, field(case, "files"), false);
    write_files(&tmp, field(case, "bytes"), true);
    let path = std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into());
    let mut out = String::new();
    let mut last: Option<Map> = None;
    let steps = field(case, "steps")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    for (n, step) in steps.iter().enumerate() {
        let n = n + 1;
        if file_step(&tmp, step) {
            continue;
        }
        let machine = field(step, "home").and_then(Value::as_str).unwrap_or("a");
        let env = Env::default()
            .with_os("PATH", &path)
            .with_os("HOME", tmp.join(machine).join("user"))
            .with("LANG", "C.UTF-8");
        let today_arg = |a: &Value| {
            field(a, "today")
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        if let Some(a) = field(step, "export") {
            let outdir: Option<PathBuf> =
                field(a, "out").and_then(Value::as_str).map(|r| tmp.join(r));
            let r = portable::export(
                &cwd,
                outdir.as_deref(),
                today_arg(a).as_deref(),
                &env,
                &NoGit,
            );
            out.push_str(&format!("## {n} export\n{}", result_text(&r)));
        } else if let Some(a) = field(step, "plan") {
            let archive = tmp.join(field(a, "archive").and_then(Value::as_str).unwrap_or(""));
            let r = portable::plan_import(&archive, &cwd, today_arg(a).as_deref(), &env, &NoGit);
            if let Ok(m) = &r {
                last = Some(m.clone());
            }
            out.push_str(&format!("## {n} plan\n{}", result_text(&r)));
        } else if let Some(a) = field(step, "apply") {
            let mut plan = match field(a, "plan") {
                Some(p) => subst(p, &root_s),
                None => last.clone().map_or(Value::Null, Value::Object),
            };
            let edits = field(a, "edits").map(|e| subst(e, &root_s));
            edit(&mut plan, edits.as_ref());
            let r = portable::apply_import(&plan, &NoGit);
            out.push_str(&format!("## {n} apply\n{}", result_text(&r)));
        } else if let Some(args) = field(step, "cli").and_then(Value::as_array) {
            let args: Vec<String> = args
                .iter()
                .map(|x| {
                    x.as_str()
                        .unwrap_or("")
                        .replace("<ROOT>", &root_s)
                        .replace("<TODAY>", &today)
                })
                .collect();
            let argv: Vec<OsString> = args.iter().map(Into::into).collect();
            let o = portable::main(&argv, &cwd, &env, &NoGit);
            out.push_str(&format!(
                "## {n} $ {}\nexit {}\n--- stdout\n{}--- stderr\n{}",
                args.join(" "),
                o.code,
                o.stdout,
                o.stderr
            ));
        } else {
            panic!("unknown step {}", json::dumps(step, &json::PY_DEFAULT));
        }
    }
    out.push_str("=== tree ===\n");
    out.push_str(&tree(&tmp, &today));
    let text = out.replace(&root_s, "<ROOT>").replace(&today, "<TODAY>");
    Observed::text(0, &text, "")
}

// ---- cases -----------------------------------------------------------------------------------

fn s(v: &str) -> Value {
    Value::Str(v.to_string())
}

fn int(n: i64) -> Value {
    Value::Int(Int::Small(n))
}

fn arr(items: Vec<Value>) -> Value {
    Value::Array(items)
}

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    let mut m = Map::new();
    for (k, v) in pairs {
        m.insert(k.to_string(), v);
    }
    Value::Object(m)
}

/// A JSON path of keys (strings) and indices (`#n`).
fn path(parts: &[&str]) -> Value {
    arr(parts
        .iter()
        .map(|p| match p.strip_prefix('#') {
            Some(i) => int(i.parse().expect("index")),
            None => s(p),
        })
        .collect())
}

const PM: &str = "user/.claude/anthology/personal-memory/";

/// The personal-memory folder of a machine.
fn pm(machine: &str, name: &str) -> String {
    format!("{machine}/{PM}{name}")
}

fn note_text(name: &str, ty: &str, desc: &str, body: &str) -> String {
    format!("---\nname: {name}\ndescription: {desc}\nmetadata:\n  type: {ty}\n---\n\n{body}")
}

/// Secret-looking strings assembled at run time, so no literal sits in the repository.
fn secrets() -> Vec<String> {
    vec![
        format!("id {}{}", "AK", "IAABCDEFGHIJKLMNOP"),
        format!("{}_{}", "ghp", "a1".repeat(20)),
        format!("{}b-{}", "xox", "1234567890-abcdef"),
        format!("{}-{}", "sk", "abcdefghij0123456789"),
        format!("{}word: {}", "pass", "hunter22hunter"),
        format!("{}-ant-{}", "sk", "abcdefghij0123456789"),
    ]
}

fn email() -> String {
    format!("{}@{}", "alice", "mail.acme.zz")
}

/// The archive `build` writes, as hex.
fn archive_hex(build: impl FnOnce(&Path) -> PathBuf) -> String {
    let dir = tempfile::tempdir().expect("tempdir");
    let p = build(&dir.path().join("x.tar.gz"));
    hex(&fs::read(p).expect("read the archive"))
}

fn bad_hex(kind: &str) -> String {
    let dir = tempfile::tempdir().expect("tempdir");
    hex(&fs::read(mx::bad(dir.path(), kind)).expect("read the archive"))
}

fn files(pairs: &[(String, String)]) -> Value {
    let mut m = Map::new();
    for (k, v) in pairs {
        m.insert(k.clone(), s(v));
    }
    Value::Object(m)
}

fn one(k: &str, v: Value) -> Value {
    obj(vec![(k, v)])
}

fn on(machine: &str, k: &str, v: Value) -> Value {
    obj(vec![(k, v), ("home", s(machine))])
}

fn plan_step(machine: &str, archive: &str, today: &str) -> Value {
    on(
        machine,
        "plan",
        obj(vec![("archive", s(archive)), ("today", s(today))]),
    )
}

fn apply_last(machine: &str, edits: Vec<Value>) -> Value {
    let a = if edits.is_empty() {
        obj(vec![])
    } else {
        obj(vec![("edits", arr(edits))])
    };
    on(machine, "apply", a)
}

fn cli(machine: &str, args: &[&str]) -> Value {
    on(machine, "cli", arr(args.iter().map(|a| s(a)).collect()))
}

fn case(fs_: Value, bytes: Value, steps: Vec<Value>) -> Value {
    obj(vec![
        ("files", fs_),
        ("bytes", bytes),
        ("steps", arr(steps)),
    ])
}

fn seed_personal() -> Vec<(String, String)> {
    vec![
        (
            pm("a", "user-me.md"),
            note_text("user-me", "user", "About me", "Me.\n"),
        ),
        (
            pm("a", "feedback-x.md"),
            note_text("feedback-x", "feedback", "Do x", "X.\n"),
        ),
    ]
}

/// The edits of `test_forged_plan_entries_are_refused` and more forged shapes.
fn forged_edits() -> Vec<Vec<Value>> {
    let set = |p: &[&str], v: Value| arr(vec![path(p), v]);
    let del = |p: &[&str]| arr(vec![path(p)]);
    let repo_entry = obj(vec![
        ("member", s("repo/memory/a.md")),
        ("scope", s("repo")),
        ("agent", Value::Null),
        ("name", s("a.md")),
    ]);
    vec![
        vec![set(&["new", "#0", "name"], s("../escape.md"))],
        vec![set(&["new", "#0", "name"], s("MEMORY.md"))],
        vec![set(&["new", "#0", "name"], s("foo.md\n"))],
        vec![set(&["new", "#0", "name"], int(5))],
        vec![set(&["new", "#0", "scope"], s("elsewhere"))],
        vec![
            set(&["new", "#0", "member"], s("personal/never-in-archive.md")),
            set(&["new", "#0", "name"], s("never-in-archive.md")),
        ],
        vec![set(&["home"], s("<ROOT>/not-anthology"))],
        vec![set(&["home"], s("<ROOT>/b/user/.claude/anthology/"))],
        vec![set(&["home"], int(5))],
        vec![set(&["archive"], s("<ROOT>/missing.tar.gz"))],
        vec![set(&["archive"], s("<ROOT>/plain"))],
        vec![set(&["sha256"], s(&"0".repeat(64)))],
        vec![del(&["sha256"])],
        vec![
            set(&["repo_root"], s("<ROOT>")),
            set(&["new"], arr(vec![repo_entry.clone()])),
        ],
        vec![set(&["new"], arr(vec![repo_entry]))],
        vec![
            set(&["new", "#0", "scope"], s("agent")),
            set(&["new", "#0", "agent"], s("ag")),
        ],
        vec![
            set(&["new", "#0", "scope"], s("agent")),
            set(&["new", "#0", "agent"], int(5)),
        ],
        vec![set(&["new", "#0", "scope"], s("repo"))],
        vec![set(&["new"], s("personal/x.md"))],
        vec![set(&["new"], arr(vec![int(5)]))],
        vec![set(&["new"], obj(vec![("personal/a.md", int(1))]))],
        vec![set(&["new"], Value::Null)],
        vec![set(&["conflicts"], Value::Null)],
        vec![set(&["blocked"], int(5))],
        vec![set(&["blocked"], arr(vec![int(5), s("x")]))],
        vec![set(
            &["blocked"],
            arr(vec![obj(vec![("file", s("personal/user-me.md"))])]),
        )],
        vec![set(&["date"], int(5))],
        vec![set(&["date"], s("2026-04-05T10:00:00"))],
        vec![set(&["date"], Value::Null)],
        vec![del(&["date"])],
        vec![del(&["new", "#0", "member"])],
        vec![set(&["identical"], arr(vec![]))],
        vec![
            set(&["identical"], path(&[])),
            set(&["conflicts"], obj(vec![])),
        ],
        vec![set(&["new", "#0", "dest"], s("/etc/passwd"))],
    ]
}

fn seeds() -> Vec<(String, Value)> {
    let mut out: Vec<(String, Value)> = Vec::new();
    let export_out = "out/anthology-memory-2026-04-05.tar.gz";
    // ExportTest / ImportTest: export, plan and apply into an empty machine, then again.
    out.push((
        "round-trip".into(),
        case(
            files(&seed_personal()),
            obj(vec![]),
            vec![
                one(
                    "export",
                    obj(vec![("out", s("out")), ("today", s("2026-04-05"))]),
                ),
                one(
                    "export",
                    obj(vec![("out", s("out")), ("today", s("2026-04-05"))]),
                ),
                one(
                    "export",
                    obj(vec![("out", Value::Null), ("today", s("2026-04-05"))]),
                ),
                plan_step("b", export_out, "2026-04-06"),
                apply_last("b", vec![]),
                plan_step("b", export_out, "2026-04-07"),
                apply_last("b", vec![]),
            ],
        ),
    ));
    // Identical skipped, conflict kept side by side, the imported name made unique.
    let mut b_notes = seed_personal()
        .into_iter()
        .map(|(k, v)| (k.replacen("a/", "b/", 1), v))
        .collect::<Vec<_>>();
    b_notes[1].1 = "my own different note\n".into();
    b_notes.push((
        pm("b", "feedback-x.imported-2026-04-06.md"),
        "an older different import\n".into(),
    ));
    let mut all = seed_personal();
    all.extend(b_notes);
    out.push((
        "conflicts".into(),
        case(
            files(&all),
            obj(vec![]),
            vec![
                one(
                    "export",
                    obj(vec![("out", s("out")), ("today", s("2026-04-05"))]),
                ),
                plan_step("b", export_out, "2026-04-06"),
                one(
                    "write",
                    files(&[(pm("b", "user-new.md"), "appeared\n".into())]),
                ),
                apply_last("b", vec![]),
                plan_step("b", export_out, "2026-04-07"),
                apply_last("b", vec![]),
            ],
        ),
    ));
    // Only plain notes are exported; refusals inside a git work tree and onto a file.
    let mut odd = seed_personal();
    for (name, text) in [
        ("MEMORY.md", "- generated\n"),
        ("notes.txt", "x"),
        (".hidden.md", "x"),
        ("has space.md", "x"),
        ("sub/deep.md", "x"),
        ("-dash.md", "x"),
        ("\u{dc}n\u{ef}.md", "x"),
    ] {
        odd.push((pm("a", name), text.into()));
    }
    odd.push((
        format!("a/{PM}{}.md", "l".repeat(140)),
        "long name\n".into(),
    ));
    odd.push(("outside.md".into(), "secret outside\n".into()));
    odd.push(("work/.git/HEAD".into(), "ref\n".into()));
    odd.push(("afile".into(), "x".into()));
    let mut bad_utf8 = Map::new();
    bad_utf8.insert(pm("a", "bad.md"), s("fffe206e6f74207574662d38"));
    out.push((
        "export-shapes".into(),
        case(
            files(&odd),
            Value::Object(bad_utf8),
            vec![
                one(
                    "symlink",
                    obj(vec![(&pm("a", "link.md"), s("<ROOT>/outside.md"))]),
                ),
                one(
                    "export",
                    obj(vec![("out", s("out")), ("today", s("2026-04-05"))]),
                ),
                one(
                    "export",
                    obj(vec![("out", s("work")), ("today", s("2026-04-05"))]),
                ),
                one(
                    "export",
                    obj(vec![
                        ("out", s("work/sub/deeper")),
                        ("today", s("2026-04-05")),
                    ]),
                ),
                one(
                    "export",
                    obj(vec![("out", s("afile")), ("today", s("2026-04-05"))]),
                ),
                plan_step("b", export_out, "2026-04-06"),
                apply_last("b", vec![]),
            ],
        ),
    ));
    // Secrets: labels only, in export, plan and the CLI.
    let mut sec = Vec::new();
    for (i, text) in secrets().iter().enumerate() {
        sec.push((
            pm("a", &format!("user-k{i}.md")),
            note_text(&format!("user-k{i}"), "user", "k", &format!("{text}\n")),
        ));
    }
    out.push((
        "secrets".into(),
        case(
            files(&sec),
            obj(vec![]),
            vec![
                one(
                    "export",
                    obj(vec![("out", s("out")), ("today", s("2026-04-05"))]),
                ),
                plan_step("b", export_out, "2026-04-06"),
                cli("a", &["export", "--out", "<ROOT>/cli-out"]),
                cli("a", &["export", "--json"]),
                cli(
                    "b",
                    &["import", "<ROOT>/cli-out/anthology-memory-<TODAY>.tar.gz"],
                ),
                cli(
                    "b",
                    &[
                        "import",
                        "<ROOT>/cli-out/anthology-memory-<TODAY>.tar.gz",
                        "--json",
                    ],
                ),
            ],
        ),
    ));
    // The CLI: dry run, --yes, the sha256 binding, errors and argparse.
    let good = archive_hex(|p| {
        mx::make_archive(
            p,
            &[
                ("personal/user-p.md", Spec::File(b"personal\n".to_vec())),
                ("repo/memory/project-r.md", Spec::File(b"repo\n".to_vec())),
                (
                    "repo/agent-memory/anthology-w/feedback-w.md",
                    Spec::File(b"agent\n".to_vec()),
                ),
            ],
            true,
            false,
        )
    });
    let good_sha = sha(&unhex(&good));
    let upper = good_sha.to_uppercase();
    let mut inb = Map::new();
    inb.insert("in/m.tar.gz".into(), s(&good));
    let arc = "<ROOT>/in/m.tar.gz";
    out.push((
        "cli".into(),
        case(
            obj(vec![]),
            Value::Object(inb.clone()),
            vec![
                cli("b", &["import", arc, "--dry-run"]),
                cli("b", &["import", arc]),
                cli("b", &["import", arc, "--yes", "--dry-run"]),
                cli("b", &["import", arc, "--yes", "--sha256", &"0".repeat(64)]),
                cli("b", &["import", arc, "--yes", "--sha256", &upper]),
                cli("b", &["import", arc, "--yes", "--sha256", &good_sha]),
                cli("b", &["import", arc, "--yes", "--json"]),
                cli("b", &["import", "<ROOT>/nope.tar.gz"]),
                cli("b", &["import", "<ROOT>/plain"]),
                cli("b", &[]),
                cli("b", &["-h"]),
                cli("b", &["import", "-h"]),
                cli("b", &["export", "-h"]),
                cli("b", &["import"]),
                cli("b", &["frobnicate"]),
                cli("b", &["export", "--bogus"]),
                cli("b", &["import", arc, "--sha256"]),
                cli("b", &["export", "--out", "<ROOT>/plain/x"]),
            ],
        ),
    ));
    // Folder members are allowed; every manifest shape.
    let dirs = archive_hex(|p| {
        mx::make_archive(
            p,
            &[
                ("personal", Spec::Dir),
                ("personal/a.md", Spec::File(b"a\n".to_vec())),
                ("repo/agent-memory/anthology-w", Spec::Dir),
            ],
            true,
            false,
        )
    });
    let mut m = Map::new();
    m.insert("in/d.tar.gz".into(), s(&dirs));
    out.push((
        "folders".into(),
        case(
            obj(vec![]),
            Value::Object(m),
            vec![
                plan_step("b", "in/d.tar.gz", "2026-04-06"),
                apply_last("b", vec![]),
            ],
        ),
    ));
    for (i, manifest) in MANIFESTS.iter().enumerate() {
        let a = archive_hex(|p| mx::manifest_variant(p, manifest.as_bytes()));
        let mut m = Map::new();
        m.insert("in/m.tar.gz".into(), s(&a));
        out.push((
            format!("manifest-{i}"),
            case(
                obj(vec![]),
                Value::Object(m),
                vec![
                    plan_step("b", "in/m.tar.gz", "2026-04-06"),
                    apply_last("b", vec![]),
                ],
            ),
        ));
    }
    // MaliciousArchiveTest: every kind through plan, a hand-forged plan, and the CLI.
    for kind in mx::KINDS {
        let mut m = Map::new();
        m.insert("in/bad.tar.gz".into(), s(&bad_hex(kind)));
        let forged = obj(vec![
            ("archive", s("<ROOT>/in/bad.tar.gz")),
            ("sha256", s(&sha(&unhex(&bad_hex(kind))))),
            ("date", s("2026-04-06")),
            ("home", s("<ROOT>/b/user/.claude/anthology")),
            ("repo_root", Value::Null),
            (
                "new",
                arr(vec![obj(vec![
                    ("member", s("personal/good.md")),
                    ("scope", s("personal")),
                    ("agent", Value::Null),
                    ("name", s("good.md")),
                ])]),
            ),
            ("identical", arr(vec![])),
            ("conflicts", arr(vec![])),
        ]);
        let cat = if matches!(kind, "not-gzip" | "empty-file" | "truncated") {
            "corrupt"
        } else {
            "archive"
        };
        out.push((
            format!("{cat}-{kind}"),
            case(
                obj(vec![]),
                Value::Object(m),
                vec![
                    plan_step("b", "in/bad.tar.gz", "2026-04-06"),
                    on("b", "apply", one("plan", forged)),
                    cli("b", &["import", "<ROOT>/in/bad.tar.gz", "--yes"]),
                ],
            ),
        ));
    }
    // Forged plans of a good archive (test_forged_plan_entries_are_refused and more).
    for (i, edits) in forged_edits().into_iter().enumerate() {
        out.push((
            format!("forged-{i}"),
            case(
                obj(vec![]),
                Value::Object(inb.clone()),
                vec![
                    plan_step("b", "in/m.tar.gz", "2026-04-06"),
                    apply_last("b", edits),
                ],
            ),
        ));
    }
    out.push((
        "forged-shapes".into(),
        case(
            obj(vec![]),
            Value::Object(inb.clone()),
            vec![
                on("b", "apply", one("plan", int(5))),
                on("b", "apply", one("plan", arr(vec![]))),
                on("b", "apply", obj(vec![])),
            ],
        ),
    ));
    // Destinations that are symlinks or files.
    for (label, step) in [
        (
            "personal-symlink",
            one(
                "symlink",
                obj(vec![(
                    "b/user/.claude/anthology/personal-memory",
                    s("<ROOT>/outside"),
                )]),
            ),
        ),
        (
            "anthology-symlink",
            one(
                "symlink",
                obj(vec![("b/user/.claude/anthology", s("<ROOT>/outside"))]),
            ),
        ),
        (
            "personal-file",
            one(
                "write",
                files(&[(
                    "b/user/.claude/anthology/personal-memory".into(),
                    "x".into(),
                )]),
            ),
        ),
        (
            "note-symlink",
            one(
                "symlink",
                obj(vec![(&pm("b", "user-p.md"), s("<ROOT>/outside/t.md"))]),
            ),
        ),
        (
            "index-symlink",
            one(
                "symlink",
                obj(vec![(&pm("b", "MEMORY.md"), s("<ROOT>/outside/t.md"))]),
            ),
        ),
    ] {
        out.push((
            format!("dest-{label}"),
            case(
                files(&[("outside/t.md".into(), "precious\n".into())]),
                Value::Object(inb.clone()),
                vec![
                    step,
                    plan_step("b", "in/m.tar.gz", "2026-04-06"),
                    apply_last("b", vec![]),
                ],
            ),
        ));
    }
    // MX1 review M-8 (D-8): a forged date that would put `/` or `..` into the `.imported-`
    // name of a conflict. Python builds the name from it; the port refuses the plan. Appended
    // last so the other seeds keep their ids.
    for (label, date) in [("slash", "../../esc"), ("dotdot", "a..b")] {
        out.push((
            format!("forged-date-{label}"),
            case(
                files(&[(pm("b", "user-p.md"), "my own different note\n".into())]),
                Value::Object(inb.clone()),
                vec![
                    plan_step("b", "in/m.tar.gz", "2026-04-06"),
                    apply_last("b", vec![arr(vec![path(&["date"]), s(date)])]),
                ],
            ),
        ));
    }
    // macOS only: names at the tar filter's cut-off there (a path of 1024 bytes under
    // `/private/tmp/tmpXXXXXXXX`), which the generator's Linux lengths never reach. A note of
    // 986 `n`s fits and one of 987 does not; an agent folder of 975 `a`s fits with its note, one
    // of 980 is refused by its full path, one of 981 by the folder. Linux runs no extra case.
    #[cfg(target_os = "macos")]
    for (label, name) in [985, 986, 987]
        .map(|k| (format!("n{k}"), format!("personal/{}.md", "n".repeat(k))))
        .into_iter()
        .chain([975, 980, 981].map(|k| {
            let agent = "a".repeat(k);
            (format!("a{k}"), format!("repo/agent-memory/{agent}/x.md"))
        }))
    {
        let a = archive_hex(|p| {
            mx::make_archive(
                p,
                &[(name.as_str(), Spec::File(b"x\n".to_vec()))],
                true,
                false,
            )
        });
        let mut bytes = Map::new();
        bytes.insert("in/x.tar.gz".into(), s(&a));
        out.push((
            format!("macos-cutoff-{label}"),
            case(
                obj(vec![]),
                Value::Object(bytes),
                vec![plan_step("b", "in/x.tar.gz", "2026-04-06")],
            ),
        ));
    }
    out
}

const MANIFESTS: &[&str] = &[
    r#"{"format": 1}"#,
    r#"{"format": 99}"#,
    r#"{"format": "1"}"#,
    r#"{"format": 1.0}"#,
    r#"{"format": true}"#,
    r#"{"format": 1, "x": NaN}"#,
    r#"{"format": 1, "format": 2}"#,
    r#"{"format": 2, "format": 1}"#,
    "[1]",
    "1",
    "null",
    "not json",
    "",
    "\u{feff}{\"format\": 1}",
    "{\"format\": 1}\n\n",
    "{\"format\": 1} x",
    "{\"format\": 1e0}",
    "{\"format\": 10000000000000000000000}",
];

// ---- the generator ---------------------------------------------------------------------------

/// Note file names. No two differ only in case: macOS folders are case-insensitive, Linux's
/// (the oracle's) are not, so `MEMORY.md` next to `memory.md` would differ by platform.
const NOTE_NAMES: &[&str] = &[
    "user-me.md",
    "feedback-x.md",
    "project-r.md",
    "reference-a.md",
    "a.b-c_d.md",
    "x.md",
    "MEMORY.md",
    "notes.txt",
    ".hidden.md",
    "has space.md",
    "-dash.md",
    "x.imported-2026-04-06.md",
];

const TYPES: &[&str] = &[
    "user",
    "feedback",
    "project",
    "reference",
    "other",
    "User # me",
];
const DAYS: &[&str] = &["2026-04-05", "2026-04-06", "2026-04-07"];

fn gen_text(rng: &mut Rng) -> String {
    let secrets = secrets();
    let mut body = String::new();
    for _ in 0..rng.below(4) {
        match rng.below(7) {
            0 => body.push_str(rng.pick(&secrets)),
            1 => body.push_str(&email()),
            2 => body.push_str("DB at 10.1.2.3"),
            3 => body.push_str(&rng.string(12)),
            _ => body.push_str(rng.pick(&["Use Docker.", "zebra", "caf\u{e9}", "line\r\nnext"])),
        }
        body.push('\n');
    }
    if rng.chance(0.2) {
        return body;
    }
    let name = rng.ascii_string(8).replace(['\n', ':'], "");
    note_text(&name, rng.pick(TYPES), "d", &body)
}

fn gen_name(rng: &mut Rng) -> String {
    if rng.chance(0.08) {
        return format!("{}.md", "l".repeat(90 + rng.index(80)));
    }
    rng.pick(NOTE_NAMES).to_string()
}

/// Notes on a machine: text files, sometimes a non-UTF-8 one (into `bytes`).
fn gen_notes(rng: &mut Rng, machine: &str, fs_: &mut Vec<(String, String)>, bytes: &mut Map) {
    for _ in 0..rng.below(5) {
        let name = gen_name(rng);
        if rng.chance(0.05) {
            bytes.insert(pm(machine, &name), s("fffe00"));
        } else {
            fs_.push((pm(machine, &name), gen_text(rng)));
        }
    }
}

/// export from `a`, plan and apply into `b` (or `a`), with changes in between.
fn gen_flow(rng: &mut Rng) -> Value {
    let mut fs_ = Vec::new();
    let mut bytes = Map::new();
    gen_notes(rng, "a", &mut fs_, &mut bytes);
    let mut steps = Vec::new();
    let day = *rng.pick(DAYS);
    let (out, archive) = match rng.below(10) {
        0 | 1 => (
            Value::Null,
            format!("a/user/.claude/anthology/exports/anthology-memory-{day}.tar.gz"),
        ),
        2 => {
            fs_.push(("work/.git/HEAD".into(), "x".into()));
            (s("work/sub"), "work/sub/x.tar.gz".to_string())
        }
        3 => {
            fs_.push(("afile".into(), "x".into()));
            (s("afile"), "afile/x.tar.gz".to_string())
        }
        _ => (s("out"), format!("out/anthology-memory-{day}.tar.gz")),
    };
    steps.push(one(
        "export",
        obj(vec![("out", out.clone()), ("today", s(day))]),
    ));
    if rng.chance(0.2) {
        steps.push(one("export", obj(vec![("out", out), ("today", s(day))])));
    }
    let target = if rng.chance(0.85) { "b" } else { "a" };
    if target == "b" {
        // Existing notes on the second machine: copies, conflicts, earlier imports.
        for (k, v) in fs_.clone() {
            if k.starts_with("a/") && rng.chance(0.3) {
                let text = if rng.chance(0.5) { v } else { "mine\n".into() };
                fs_.push((k.replacen("a/", "b/", 1), text));
            }
        }
        gen_notes(rng, "b", &mut fs_, &mut bytes);
    }
    for _ in 0..=rng.below(2) {
        steps.push(plan_step(target, &archive, rng.pick(DAYS)));
        if rng.chance(0.2) {
            let name = gen_name(rng);
            steps.push(one(
                "write",
                files(&[(pm(target, &name), "appeared\n".into())]),
            ));
        }
        if rng.chance(0.85) {
            steps.push(apply_last(target, vec![]));
        }
    }
    case(files(&fs_), Value::Object(bytes), steps)
}

const MEMBER_NAMES: &[&str] = &[
    "personal/a.md",
    "personal/b.md",
    "personal/user-p.md",
    "repo/memory/project-r.md",
    "repo/agent-memory/anthology-w/feedback-w.md",
    "repo/agent-memory/x.y/n.md",
    "personal/MEMORY.md",
    "personal/Memory.MD",
    "personal/x.txt",
    "personal/.x.md",
    "personal/sub/x.md",
    "personal/../x.md",
    "personal/./x.md",
    "personal//x.md",
    "personal\\x.md",
    "/etc/evil.md",
    "../evil.md",
    "other/x.md",
    "manifest.json/x",
    "personal/foo\n.md",
    "personal/a.md\n",
    "personal/\u{fc}.md",
    "personal/a\u{0}b.md",
    "repo/agent-memory/.hid/x.md",
    "repo/agent-memory/-a/x.md",
    "repo/settings.json",
];
const DIR_NAMES: &[&str] = &[
    "personal",
    "personal/",
    "repo",
    "repo/memory",
    "repo/agent-memory",
    "repo/agent-memory/anthology-w",
    "repo/agent-memory/.hid",
    "other",
    "manifest.json",
];

fn gen_member(rng: &mut Rng) -> (String, Spec) {
    let k = rng.below(20);
    if k < 3 {
        return (rng.pick(DIR_NAMES).to_string(), Spec::Dir);
    }
    let name = if rng.chance(0.08) {
        format!("personal/{}.md", "n".repeat(90 + rng.index(80)))
    } else {
        rng.pick(MEMBER_NAMES).to_string()
    };
    let spec = match k {
        3 => Spec::Symlink(
            rng.pick(&["/etc/passwd", "personal/a.md", "../x"])
                .to_string(),
        ),
        4 => Spec::Hardlink("personal/a.md".into()),
        5 => rng.pick(&[Spec::Char, Spec::Block, Spec::Fifo]).clone(),
        6 => Spec::Zero(rng.below(2000)),
        7 => Spec::File(vec![0xff, 0xfe, b'x']),
        _ => Spec::File(gen_text(rng).into_bytes()),
    };
    (name, spec)
}

/// An archive of random members (or one of the malicious kinds), planned, applied, imported;
/// `corrupt` for the kinds that are not a readable archive.
fn gen_archive(rng: &mut Rng) -> (&'static str, Value) {
    let mut cat = "archive";
    let a = if rng.chance(0.25) {
        let kind = *rng.pick(&mx::KINDS);
        if matches!(kind, "not-gzip" | "empty-file" | "truncated") {
            cat = "corrupt";
        }
        bad_hex(kind)
    } else if rng.chance(0.1) {
        let manifest = *rng.pick(MANIFESTS);
        archive_hex(|p| mx::manifest_variant(p, manifest.as_bytes()))
    } else {
        let members: Vec<(String, Spec)> = (0..=rng.below(5)).map(|_| gen_member(rng)).collect();
        let manifest = rng.chance(0.9);
        archive_hex(|p| {
            let m: Vec<(&str, Spec)> = members
                .iter()
                .map(|(n, s)| (n.as_str(), s.clone()))
                .collect();
            mx::make_archive(p, &m, manifest, false)
        })
    };
    (cat, archive_case(rng, a))
}

fn archive_case(rng: &mut Rng, a: String) -> Value {
    let mut bytes = Map::new();
    bytes.insert("in/x.tar.gz".into(), s(&a));
    let mut fs_ = Vec::new();
    let mut nb = Map::new();
    gen_notes(rng, "b", &mut fs_, &mut nb);
    for (k, v) in nb.iter() {
        bytes.insert(k.to_string(), v.clone());
    }
    let mut steps = vec![plan_step("b", "in/x.tar.gz", rng.pick(DAYS))];
    if rng.chance(0.7) {
        steps.push(apply_last("b", vec![]));
    }
    if rng.chance(0.5) {
        let mut args = vec!["import", "<ROOT>/in/x.tar.gz"];
        if rng.chance(0.6) {
            args.push("--yes");
        }
        if rng.chance(0.3) {
            args.push("--json");
        }
        steps.push(cli("b", &args));
    }
    case(files(&fs_), Value::Object(bytes), steps)
}

/// A valid archive, then truncated, byte-flipped, or with bytes added.
fn gen_corrupt(rng: &mut Rng) -> Value {
    let members: Vec<(String, Spec)> = (0..=rng.below(3))
        .map(|i| {
            (
                format!("personal/n{i}.md"),
                Spec::File(gen_text(rng).into_bytes()),
            )
        })
        .collect();
    let mut b = unhex(&archive_hex(|p| {
        let m: Vec<(&str, Spec)> = members
            .iter()
            .map(|(n, s)| (n.as_str(), s.clone()))
            .collect();
        mx::make_archive(p, &m, true, rng.chance(0.5))
    }));
    match rng.below(5) {
        0 => b.truncate(rng.index(b.len())),
        1 | 2 => {
            for _ in 0..=rng.below(3) {
                let i = rng.index(b.len());
                b[i] ^= 1 << rng.below(8);
            }
        }
        3 => {
            let i = rng.index(b.len());
            b[i] = rng.below(256) as u8;
        }
        _ => {
            let extra: Vec<u8> = (0..=rng.below(40)).map(|_| rng.below(256) as u8).collect();
            b.extend(extra);
        }
    }
    archive_case(rng, hex(&b))
}

// ---- tarhdr: the uncompressed tar mutated, then gzipped --------------------------------------

/// A header's checksum field set (octal, as tarfile writes it).
fn set_chksum(h: &mut [u8]) {
    h[148..156].copy_from_slice(b"        ");
    let sum: u32 = h.iter().map(|&b| u32::from(b)).sum();
    h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
}

/// A ustar header block (its checksum is set later, [`th_mutate`]).
fn th_header(name: &[u8], typ: u8, size: u64) -> Vec<u8> {
    let mut h = vec![0u8; 512];
    let n = name.len().min(100);
    h[..n].copy_from_slice(&name[..n]);
    for (at, len, v) in [
        (100, 8, 0o644),
        (108, 8, 0),
        (116, 8, 0),
        (124, 12, size),
        (136, 12, 0),
        (329, 8, 0),
        (337, 8, 0),
    ] {
        h[at..at + len].copy_from_slice(format!("{v:0w$o}\0", w = len - 1).as_bytes());
    }
    h[156] = typ;
    h[257..265].copy_from_slice(b"ustar\x0000");
    h
}

fn th_pad(mut v: Vec<u8>) -> Vec<u8> {
    v.resize(v.len().div_ceil(512) * 512, 0);
    v
}

/// One PAX record, `"%d %s=%s\n"`; sometimes with a wrong length or no newline.
fn th_record(rng: &mut Rng, k: &[u8], v: &[u8]) -> Vec<u8> {
    let body = [b" ", k, b"=", v, b"\n"].concat();
    let mut n = body.len() + 1;
    while n.to_string().len() + body.len() != n {
        n += 1;
    }
    let mut rec = [n.to_string().into_bytes(), body].concat();
    if rng.chance(0.05) {
        match rng.below(3) {
            0 => rec[0] = b'9',
            1 => *rec.last_mut().expect("record") = b' ',
            _ => rec.insert(0, b'0'),
        }
    }
    rec
}

/// A number field's text: mostly `good`; else negative, garbage, or more than 4300 digits.
fn th_number(rng: &mut Rng, good: u64) -> Vec<u8> {
    if rng.chance(0.7) {
        return good.to_string().into_bytes();
    }
    match rng.below(11) {
        0 => b"x".to_vec(),
        1 => b"-1".to_vec(),
        2 => "1".repeat(4301 + rng.index(800)).into_bytes(),
        3 => format!("{}x", "7".repeat(rng.index(400))).into_bytes(),
        4 => Vec::new(),
        5 => format!(" {good} ").into_bytes(),
        6 => b"99999999999999999999999".to_vec(),
        7 => vec![0xff, b'1'],
        8 => format!("1_{good}").into_bytes(),
        9 => [b"5".repeat(4400), b"\0".to_vec()].concat(),
        _ => format!("{good}\u{3000}").into_bytes(),
    }
}

/// A member name: a note, a folder, an unsafe one, or one long enough for a GNU longname or
/// the tar filter's ENAMETOOLONG.
fn th_name(rng: &mut Rng) -> Vec<u8> {
    match rng.below(12) {
        0 => format!(
            "personal/{}.md",
            "n".repeat(*rng.pick(&[95, 150, 300, 4066, 4067, 4100]))
        )
        .into_bytes(),
        1 => format!("repo/agent-memory/{}/x.md", "a".repeat(4080)).into_bytes(),
        2 => rng.pick(DIR_NAMES).as_bytes().to_vec(),
        3 => b"personal/\xff.md".to_vec(),
        _ => rng.pick(MEMBER_NAMES).as_bytes().to_vec(),
    }
}

/// PAX records for a member of `size` data bytes.
fn th_pax_records(rng: &mut Rng, size: u64) -> Vec<u8> {
    let mut out = Vec::new();
    for _ in 0..=rng.below(3) {
        let rec = match rng.below(12) {
            0 | 1 => {
                let name = th_name(rng);
                th_record(rng, b"path", &name)
            }
            2 | 3 => th_num_record(rng, b"size", size),
            4 => th_num_record(rng, b"GNU.sparse.size", size),
            5 => th_num_record(rng, b"GNU.sparse.realsize", size),
            6 => {
                let map = [th_number(rng, 0), b",".to_vec(), th_number(rng, size)].concat();
                th_record(rng, b"GNU.sparse.map", &map)
            }
            7 => {
                let name = th_name(rng);
                th_record(rng, b"GNU.sparse.name", &name)
            }
            8 => {
                const CHARSETS: [&[u8]; 3] = [b"BINARY", b"ISO-IR 10646 2000 UTF-8", b"x"];
                let v = rng.pick(&CHARSETS);
                th_record(rng, b"hdrcharset", v)
            }
            9 => {
                const KEYS: [&[u8]; 2] = [b"GNU.sparse.offset", b"GNU.sparse.numbytes"];
                let k = rng.pick(&KEYS);
                th_num_record(rng, k, size)
            }
            10 => {
                let c = vec![b'c'; rng.index(700)];
                th_record(rng, b"comment", &c)
            }
            _ => th_record(rng, b"mtime", b"1.5"),
        };
        out.extend(rec);
    }
    out
}

/// An extended header (`x`, `g`, `L` or `K`) carrying `data`.
fn th_ext(headers: &mut Vec<usize>, out: &mut Vec<u8>, typ: u8, data: Vec<u8>) {
    let name: &[u8] = if matches!(typ, b'L' | b'K') {
        b"././@LongLink"
    } else {
        b"PaxHeader/x"
    };
    headers.push(out.len());
    out.extend(th_header(name, typ, data.len() as u64));
    out.extend(th_pad(data));
}

/// A PAX record of a number ([`th_number`] around `good`).
fn th_num_record(rng: &mut Rng, k: &[u8], good: u64) -> Vec<u8> {
    let v = th_number(rng, good);
    th_record(rng, k, &v)
}

/// One member: its extended headers (random PAX records, GNU longnames, a sparse format, a
/// deep chain), its header and its data. Each header's place goes into `headers`.
fn th_member(rng: &mut Rng, headers: &mut Vec<usize>, out: &mut Vec<u8>, name: &[u8], data: &[u8]) {
    let size = data.len() as u64;
    let mut name = name.to_vec();
    let mut data = data.to_vec();
    let mut typ = if data.is_empty() && DIR_NAMES.iter().any(|d| d.as_bytes() == name) {
        b'5'
    } else {
        b'0'
    };
    // A name too long for the header goes in a GNU longname or a PAX path, mostly.
    if name.len() > 100 && rng.chance(0.6) {
        if rng.chance(0.5) {
            th_ext(headers, out, b'L', [name.clone(), vec![0]].concat());
        } else {
            let rec = th_record(rng, b"path", &name);
            th_ext(headers, out, b'x', rec);
        }
    }
    match rng.below(20) {
        0 | 1 => {
            let recs = th_pax_records(rng, size);
            th_ext(headers, out, b'x', recs);
        }
        2 => {
            let recs = th_pax_records(rng, size);
            th_ext(headers, out, b'g', recs);
        }
        3 | 4 => {
            let long = if rng.chance(0.5) {
                th_name(rng)
            } else {
                name.clone()
            };
            let mut d = long.clone();
            if rng.chance(0.8) {
                d.push(0);
            }
            let typ = *rng.pick(b"LLLK");
            th_ext(headers, out, typ, d);
            if rng.chance(0.7) {
                name = long;
            }
        }
        5 => {
            // PAX 0.0: GNU.sparse.size and offset/numbytes pairs.
            let mut recs = th_num_record(rng, b"GNU.sparse.size", size + 2);
            recs.extend(th_num_record(rng, b"GNU.sparse.offset", 1));
            recs.extend(th_num_record(rng, b"GNU.sparse.numbytes", size));
            th_ext(headers, out, b'x', recs);
        }
        6 => {
            // PAX 0.1: GNU.sparse.map.
            let map = format!("0,{},{},1", size / 2, size / 2 + 3);
            let mut recs = th_record(rng, b"GNU.sparse.map", map.as_bytes());
            recs.extend(th_num_record(rng, b"GNU.sparse.realsize", size + 4));
            th_ext(headers, out, b'x', recs);
        }
        7 => {
            // PAX 1.0: the map heads the data, one number per line.
            let mut recs = th_record(rng, b"GNU.sparse.major", b"1");
            recs.extend(th_record(rng, b"GNU.sparse.minor", b"0"));
            recs.extend(th_record(rng, b"GNU.sparse.name", &name));
            recs.extend(th_num_record(rng, b"GNU.sparse.realsize", size + 2));
            th_ext(headers, out, b'x', recs);
            let mut map = Vec::new();
            for good in [2, 0, size / 2, size / 2 + 2] {
                map.extend(th_number(rng, good));
                map.push(b'\n');
            }
            map.extend(format!("{}\n", size - size / 2).into_bytes());
            let map = if rng.chance(0.8) { th_pad(map) } else { map };
            data = [map, data].concat();
            name = b"GNUSparseFile.0/x".to_vec();
        }
        8 => typ = b'S',
        9 => {
            // A deep chain of longname or PAX headers. Python's recursion limit falls near 329
            // headers, and the oracle's own stack moves it, so the band around it is left out.
            let depth = *rng.pick(&[2usize, 3, 20, 120, 280, 400, 1500]);
            let pax = rng.chance(0.5);
            let link: &[u8] = if name.len() < 200 {
                &name
            } else {
                b"personal/a.md"
            };
            let link = [link, b"\0"].concat();
            for _ in 0..depth {
                if pax {
                    let rec = th_record(rng, b"comment", b"c");
                    th_ext(headers, out, b'x', rec);
                } else {
                    th_ext(headers, out, b'L', link.clone());
                }
            }
        }
        _ => {}
    }
    if rng.chance(0.08) {
        typ = *rng.pick(b"0\x005SxgLK127XA");
    }
    headers.push(out.len());
    let mut h = th_header(&name, typ, data.len() as u64);
    if typ == b'S' {
        // Old GNU sparse: four (offset, numbytes) slots, isextended, realsize; an extension
        // block (no checksum) follows when isextended is set.
        h[386..398].copy_from_slice(b"00000000000\0");
        h[398..410].copy_from_slice(format!("{:011o}\0", data.len()).as_bytes());
        h[483..495].copy_from_slice(format!("{:011o}\0", data.len() + 1).as_bytes());
        if rng.chance(0.4) {
            h[482] = 1;
            let mut ext = vec![0u8; 512];
            ext[..12].copy_from_slice(&th_field(rng, 12));
            ext[504] = u8::from(rng.chance(0.2));
            h.extend(ext);
        }
    }
    out.extend(h);
    out.extend(th_pad(data));
}

/// A number field's bytes: octal, base-256 (positive or negative), or garbage.
fn th_field(rng: &mut Rng, len: usize) -> Vec<u8> {
    let mut f = vec![0u8; len];
    match rng.below(5) {
        0 => {
            f[0] = 0x80;
            f[len - 1] = rng.below(256) as u8;
        }
        1 => f.iter_mut().for_each(|b| *b = 0xff),
        2 => f[..3].copy_from_slice(b"9x "),
        _ => {
            f[..len - 1].copy_from_slice(format!("{:0w$o}", rng.below(64), w = len - 1).as_bytes())
        }
    }
    f
}

/// Sets the checksums of the header blocks at `headers`, then mutates up to three of them
/// (a long chain stays mostly intact): base-256 sizes and checksums, type bytes, number
/// fields, random bytes. A mutated header's checksum is recomputed about 70% of the time;
/// otherwise it keeps the one it had before, or gets a broken one.
fn th_mutate(rng: &mut Rng, tar: &mut [u8], headers: &[usize]) {
    for &at in headers {
        set_chksum(&mut tar[at..at + 512]);
    }
    for _ in 0..rng.below(4) {
        let at = *rng.pick(headers);
        let h = &mut tar[at..at + 512];
        let recompute = rng.chance(0.7);
        let mut b256_sum = false;
        match rng.below(7) {
            0 => {
                // base-256 size, the same value or a huge or negative one
                let size = nti_octal(&h[124..136]);
                let v: i128 = match rng.below(3) {
                    0 => i128::from(size),
                    1 => 1 << 80,
                    _ => -5,
                };
                let bytes = v.to_be_bytes();
                h[124] = if v < 0 { 0xff } else { 0x80 };
                h[125..136].copy_from_slice(&bytes[16 - 11..]);
            }
            1 => b256_sum = true,
            2 => h[156] = *rng.pick(b"0\x005SxgLK127XA"),
            3 => {
                let at = *rng.pick(&[100usize, 108, 116, 136, 329, 337]);
                let len = if at == 136 { 12 } else { 8 };
                h[at..at + len].copy_from_slice(&th_field(rng, len));
            }
            4 => {
                let i = rng.index(512);
                h[i] = rng.below(256) as u8;
            }
            5 => h[124..136].copy_from_slice(&th_field(rng, 12)),
            _ => h[345..355].copy_from_slice(b"personal\0\0"),
        }
        if recompute || b256_sum {
            set_chksum(h);
        }
        if b256_sum {
            let sum = u64::from(
                u32::from_str_radix(std::str::from_utf8(&h[148..154]).expect("octal"), 8)
                    .expect("octal"),
            );
            h[148] = 0x80;
            h[149..156].copy_from_slice(&sum.to_be_bytes()[1..]);
        } else if !recompute && rng.chance(0.2) {
            h[148..156].copy_from_slice(&th_field(rng, 8));
        }
    }
}

/// An octal size field's value (0 if it is not one).
fn nti_octal(f: &[u8]) -> u64 {
    let t: String = f
        .iter()
        .take_while(|&&b| b != 0)
        .map(|&b| char::from(b))
        .collect();
    u64::from_str_radix(t.trim(), 8).unwrap_or(0)
}

/// The tar stream built and mutated header by header, sometimes truncated, then gzipped.
fn gen_tarhdr(rng: &mut Rng) -> Value {
    let mut tar = Vec::new();
    let mut headers = Vec::new();
    if rng.chance(0.9) {
        th_member(rng, &mut headers, &mut tar, b"manifest.json", mx::MANIFEST);
    }
    for _ in 0..=rng.below(3) {
        let name = th_name(rng);
        let data = if DIR_NAMES.iter().any(|d| d.as_bytes() == name) || rng.chance(0.1) {
            Vec::new()
        } else {
            gen_text(rng).into_bytes()
        };
        th_member(rng, &mut headers, &mut tar, &name, &data);
    }
    th_mutate(rng, &mut tar, &headers);
    match rng.below(10) {
        0 => {}
        1 => tar.extend([0u8; 512]),
        2 => tar.extend((0..rng.below(600)).map(|_| rng.below(256) as u8)),
        _ => tar.extend([0u8; 1024]),
    }
    if rng.chance(0.1) {
        tar.truncate(rng.index(tar.len()));
    }
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    std::io::Write::write_all(&mut gz, &tar).expect("gzip");
    let mut bytes = Map::new();
    bytes.insert("in/x.tar.gz".into(), s(&hex(&gz.finish().expect("gzip"))));
    let mut fs_ = Vec::new();
    gen_notes(rng, "b", &mut fs_, &mut bytes);
    // A plan, and sometimes the CLI's import; no `apply` of a failed plan (`null`, D-1), so
    // a case can carry the oracle's D-7 mark.
    let mut steps = vec![plan_step("b", "in/x.tar.gz", rng.pick(DAYS))];
    if rng.chance(0.5) {
        let mut args = vec!["import", "<ROOT>/in/x.tar.gz"];
        if rng.chance(0.7) {
            args.push("--yes");
        }
        if rng.chance(0.3) {
            args.push("--json");
        }
        steps.push(cli("b", &args));
    }
    case(files(&fs_), Value::Object(bytes), steps)
}

/// A good archive's plan applied with forged edits.
fn gen_forged(rng: &mut Rng) -> Value {
    let a = archive_hex(|p| {
        mx::make_archive(
            p,
            &[
                ("personal/user-me.md", Spec::File(b"me\n".to_vec())),
                ("personal/a.md", Spec::File(b"a\n".to_vec())),
                ("repo/memory/project-r.md", Spec::File(b"r\n".to_vec())),
            ],
            true,
            false,
        )
    });
    let mut bytes = Map::new();
    bytes.insert("in/x.tar.gz".into(), s(&a));
    let all = forged_edits();
    let mut edits = Vec::new();
    for _ in 0..=rng.below(2) {
        edits.extend(rng.pick(&all).iter().cloned());
    }
    let steps = vec![
        plan_step("b", "in/x.tar.gz", rng.pick(DAYS)),
        apply_last("b", edits),
    ];
    case(obj(vec![]), Value::Object(bytes), steps)
}

/// CLI runs: export and import with every flag, argparse errors.
fn gen_cli(rng: &mut Rng) -> Value {
    let mut fs_ = Vec::new();
    let mut bytes = Map::new();
    gen_notes(rng, "a", &mut fs_, &mut bytes);
    let a = archive_hex(|p| {
        mx::make_archive(
            p,
            &[("personal/user-p.md", Spec::File(gen_text(rng).into_bytes()))],
            true,
            false,
        )
    });
    let sha_in = sha(&unhex(&a));
    bytes.insert("in/x.tar.gz".into(), s(&a));
    let mut steps = Vec::new();
    for _ in 0..=rng.below(4) {
        let mut args: Vec<String> = match rng.below(8) {
            0 => vec!["export".into()],
            1 | 2 => vec!["export".into(), "--out".into(), "<ROOT>/out".into()],
            3 => vec![
                "import".into(),
                "<ROOT>/out/anthology-memory-<TODAY>.tar.gz".into(),
            ],
            4 => vec![],
            _ => vec!["import".into(), "<ROOT>/in/x.tar.gz".into()],
        };
        let is_import = args.first().is_some_and(|a| a == "import");
        if rng.chance(0.4) {
            args.push("--json".into());
        }
        if is_import {
            if rng.chance(0.5) {
                args.push("--yes".into());
            }
            if rng.chance(0.2) {
                args.push("--dry-run".into());
            }
            if rng.chance(0.3) {
                args.push("--sha256".into());
                args.push(match rng.below(3) {
                    0 => sha_in.clone(),
                    1 => sha_in.to_uppercase(),
                    _ => "0".repeat(64),
                });
            }
        }
        if rng.chance(0.08) {
            args.insert(
                rng.index(args.len() + 1),
                rng.pick(&["-h", "--bogus", "--out", "x"]).to_string(),
            );
        }
        let machine = if rng.chance(0.5) { "a" } else { "b" };
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        steps.push(cli(machine, &refs));
    }
    case(files(&fs_), Value::Object(bytes), steps)
}

fn generate(rng: &mut Rng) -> (&'static str, Value) {
    match rng.below(12) {
        0 | 1 => ("flow", gen_flow(rng)),
        2 | 3 => ("cli", gen_cli(rng)),
        4..=6 => gen_archive(rng),
        7 | 8 => ("corrupt", gen_corrupt(rng)),
        9 => ("forged", gen_forged(rng)),
        _ => ("tarhdr", gen_tarhdr(rng)),
    }
}
