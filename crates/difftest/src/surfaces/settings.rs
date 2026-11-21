//! The `settings` difftest surface (ST1): `anthology-settings` against `settings_patch.py`
//! (oracle `port/difftest/oracle/settings.py`, whose docs give the case and result forms).
//!
//! Cases cover every call of the patcher: `plan`, `apply`, `revert`, `reconcile`, `redact`,
//! the redacted diff, `ops_version`, `read_text`, `settings_path`, `plugin_rule_status`,
//! `setup_ops` (with F21 validation) and `write_text`. Generated settings documents come in
//! every layout the patcher must keep byte-stable (indent units, inline and mixed containers,
//! minified, odd spacing, CRLF and lone CR, BOMs, duplicate keys, unusual numbers, Unicode line
//! separators) and broken in many ways (truncated, trailing commas, comments, NaN, non-object
//! roots, trailing data, deep nesting). Stored ops come from a real plan, then are edited the way
//! a user or a broken state.json could. Files are read as raw bytes (UTF-16, Latin-1, broken
//! UTF-8).
//!
//! Both sides print the same JSON (`json.dumps`, ASCII) on stdout, or exit 1 with the error:
//! the exact text for a `ValueError`, `"<class> errno=<n>"` for an `OSError`, and the class name
//! alone for any other exception (marked D-1 by the oracle). Each side runs filesystem cases in
//! its own temporary folder and writes it as `<ROOT>`.

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anthology_core::Env;
use anthology_pycompat::json::{self, Int, Map, PY_DEFAULT, PY_DEFAULT_UNICODE, Value};
use anthology_pycompat::pyerr::PyExc;
use anthology_settings as st;

use crate::case::{Case, Rng, Surface};
use crate::observe::{Observed, from_oracle};
use crate::oracle::Oracle;

/// The surface name (plan §1.4).
pub const NAME: &str = "settings";

/// Whether `difftest --all` runs this surface on macOS too (`ci/jobs/difftest-macos.sh`); set
/// `false` only for a surface that cannot run there, and say why.
pub const ON_MACOS: bool = true;

/// The registered implementation.
pub fn surface() -> Option<Box<dyn Surface>> {
    Some(Box::new(Settings))
}

struct Settings;

impl Surface for Settings {
    fn name(&self) -> &'static str {
        NAME
    }

    fn seeds(&self) -> Vec<Case> {
        seeds()
            .into_iter()
            .enumerate()
            .map(|(i, d)| Case::new(format!("seed:{}", i + 1), d))
            .collect()
    }

    fn generate(&self, rng: &mut Rng, n: usize) -> Vec<Case> {
        (0..n)
            .map(|i| Case::new(format!("gen:{}", i + 1), gen_case(rng)))
            .collect()
    }

    fn run_python(&self, c: &Case, oracle: &mut Oracle) -> Observed {
        passwd_home_as_tilde(&c.data, from_oracle(&oracle.call("settings", &c.data)))
    }

    fn run_rust(&self, c: &Case) -> Observed {
        passwd_home_as_tilde(&c.data, run(&c.data))
    }
}

/// `settings_path` with no HOME falls back to this uid's password-database home, which the
/// oracle image's uid has none of (Python then keeps `~`). That home reads as `~` on both sides,
/// so the live run, the freeze and the golden replay (as a uid with an entry) compare the same.
fn passwd_home_as_tilde(case: &Value, mut o: Observed) -> Observed {
    let home = anthology_core::paths::home(&Env::default());
    let home = home.to_string_lossy();
    if text_of(case, "op") == "settings_path" && home != "~" && home != "/" {
        let from = format!("{{\"path\": \"{home}/");
        if let Some(rest) = o.stdout.strip_prefix(from.as_bytes()) {
            o.stdout = [b"{\"path\": \"~/".as_slice(), rest].concat();
        }
    }
    o
}

// ---------------------------------------------------------------------------------------------
// Small JSON helpers
// ---------------------------------------------------------------------------------------------

fn s(x: &str) -> Value {
    Value::Str(x.to_string())
}

fn int(i: i64) -> Value {
    Value::Int(Int::Small(i))
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

fn field<'a>(v: &'a Value, k: &str) -> Option<&'a Value> {
    v.as_object().and_then(|m| m.get(k))
}

fn text_of(v: &Value, k: &str) -> String {
    field(v, k)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn path_strs(path: &[&str]) -> Value {
    arr(path.iter().map(|p| s(p)).collect())
}

fn op_add(path: &[&str], value: Value, tag: Option<&str>) -> Value {
    let mut pairs = vec![
        ("op", s("add")),
        ("path", path_strs(path)),
        ("value", value),
    ];
    if let Some(t) = tag {
        pairs.push(("tag", s(t)));
    }
    obj(pairs)
}

fn op_set(path: &[&str], value: Value, tag: Option<&str>) -> Value {
    let mut pairs = vec![
        ("op", s("set")),
        ("path", path_strs(path)),
        ("value", value),
    ];
    if let Some(t) = tag {
        pairs.push(("tag", s(t)));
    }
    obj(pairs)
}

fn ops_of(v: Option<&Value>) -> Vec<st::Op> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|o| st::Op::from_value(o).expect("generated desired ops are well formed"))
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------------------------
// The Rust side
// ---------------------------------------------------------------------------------------------

const PLACE: &str = "<ROOT>";
const REAL: &str = "<REAL>";

/// `<ROOT>`/`<REAL>` in every string of `v` replaced by the sandbox folder and its realpath.
fn sub(v: &Value, root: &str, real: &str) -> Value {
    match v {
        Value::Str(x) => Value::Str(x.replace(REAL, real).replace(PLACE, root)),
        Value::Array(a) => Value::Array(a.iter().map(|x| sub(x, root, real)).collect()),
        Value::Object(m) => {
            let mut out = Map::new();
            for (k, x) in m.iter() {
                out.insert(k.to_string(), sub(x, root, real));
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

fn unsub(text: &str, roots: &[String]) -> String {
    let mut t = text.to_string();
    for r in roots {
        t = t.replace(r.as_str(), PLACE);
    }
    t
}

fn env_of(v: Option<&Value>) -> Env {
    let mut env = Env::default();
    if let Some(m) = v.and_then(Value::as_object) {
        for (k, x) in m.iter() {
            if let Some(x) = x.as_str() {
                env = env.with(k, x);
            }
        }
    }
    env
}

fn unhex(h: &str) -> Vec<u8> {
    (0..h.len() / 2)
        .map(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).unwrap_or(0))
        .collect()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn build(fs_spec: &Value, root: &Path, real: &str) {
    let rs = root.to_string_lossy().into_owned();
    for e in fs_spec.as_array().map(Vec::as_slice).unwrap_or_default() {
        let p = root.join(text_of(e, "path"));
        let parent = p.parent().expect("an entry has a parent");
        match text_of(e, "type").as_str() {
            "dir" => {
                let _ = fs::create_dir_all(&p);
            }
            "file" => {
                let _ = fs::create_dir_all(parent);
                let data = match field(e, "text") {
                    Some(Value::Str(t)) => t.replace(REAL, real).replace(PLACE, &rs).into_bytes(),
                    _ => unhex(&text_of(e, "hex")),
                };
                let _ = fs::write(&p, data);
            }
            "link" => {
                let _ = fs::create_dir_all(parent);
                let target = text_of(e, "target").replace(REAL, real).replace(PLACE, &rs);
                let _ = std::os::unix::fs::symlink(target, &p);
            }
            _ => {}
        }
        if let Some(m) = field(e, "mode").and_then(Value::as_i64) {
            let _ = fs::set_permissions(&p, fs::Permissions::from_mode(m as u32));
        }
    }
}

/// `[path, kind, mode, content]` for every entry, sorted (as the oracle's `_tree`).
fn tree(root: &Path, roots: &[String]) -> Value {
    let mut out: Vec<(String, String, String, String)> = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let rel = p
                .strip_prefix(root)
                .expect("under the root")
                .to_string_lossy()
                .into_owned();
            let Ok(md) = fs::symlink_metadata(&p) else {
                continue;
            };
            let mode = format!("{:04o}", md.mode() & 0o7777);
            if md.file_type().is_symlink() {
                let t = fs::read_link(&p).map(|t| t.to_string_lossy().into_owned());
                out.push((
                    rel,
                    "link".into(),
                    String::new(),
                    unsub(&t.unwrap_or_default(), roots),
                ));
            } else if md.is_dir() {
                out.push((rel, "dir".into(), mode, String::new()));
                stack.push(p);
            } else {
                out.push((
                    rel,
                    "file".into(),
                    mode,
                    hex(&fs::read(&p).unwrap_or_default()),
                ));
            }
        }
    }
    out.sort();
    arr(out
        .into_iter()
        .map(|(a, b, c, d)| arr(vec![s(&a), s(&b), s(&c), s(&d)]))
        .collect())
}

const NON_VALUE_ERRORS: [&str; 4] = ["TypeError", "KeyError", "AttributeError", "RecursionError"];

/// An `Err(String)` as the oracle prints it: the class alone for a non-`ValueError`.
fn err_text(e: &str) -> String {
    for class in NON_VALUE_ERRORS {
        if e.starts_with(&format!("{class}: ")) {
            return class.to_string();
        }
    }
    e.to_string()
}

fn exc_text(e: &PyExc) -> String {
    match e {
        PyExc::OSError { errno, .. } => format!("{} errno={errno}", e.type_name()),
        other => other.py_str(),
    }
}

fn fail(stderr: String) -> Observed {
    Observed::text(1, "", &stderr)
}

fn ok(payload: Value, roots: &[String]) -> Observed {
    Observed::text(0, &unsub(&json::dumps(&payload, &PY_DEFAULT), roots), "")
}

fn run(case: &Value) -> Observed {
    let op = text_of(case, "op");
    let sandbox = field(case, "fs").map(|spec| {
        let dir = tempfile::Builder::new()
            .prefix("st1-rust-")
            .tempdir()
            .expect("a temporary folder");
        let real = fs::canonicalize(dir.path())
            .expect("realpath")
            .to_string_lossy()
            .into_owned();
        build(spec, dir.path(), &real);
        (dir, real)
    });
    let (c, roots) = match &sandbox {
        Some((dir, real)) => {
            let root = dir.path().to_string_lossy().into_owned();
            (sub(case, &root, real), vec![real.clone(), root])
        }
        None => (case.clone(), Vec::new()),
    };
    let text = text_of(&c, "text");
    let env = env_of(field(&c, "env"));
    let res: Result<Value, String> = match op.as_str() {
        "plan" => st::plan(&text, &ops_of(field(&c, "ops"))).map(|p| arr(p.stored)),
        "apply" => st::apply(&text, &ops_of(field(&c, "ops")))
            .map(|(t, d)| obj(vec![("text", s(&t)), ("diff", s(&d))])),
        "revert" => {
            let stored = field(&c, "stored")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            st::revert_with_diff(&text, &stored)
                .map(|(t, d)| obj(vec![("text", s(&t)), ("diff", s(&d))]))
        }
        "reconcile" => {
            let stored = field(&c, "stored")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            st::reconcile(&text, &ops_of(field(&c, "ops")), &stored)
                .map(|(t, d, k)| obj(vec![("text", s(&t)), ("diff", s(&d)), ("stored", arr(k))]))
        }
        "redact" => st::redact(&text).map(|t| obj(vec![("text", s(&t))])),
        "diff" => {
            let a = st::redact(&text).and(st::redact(&text_of(&c, "text2")));
            // `_diff` raises exactly when one side does not redact (and the texts differ).
            let d = st::redacted_diff(&text, &text_of(&c, "text2"));
            if text != text_of(&c, "text2")
                && let Err(e) = a
            {
                Err(e)
            } else {
                Ok(obj(vec![("diff", s(&d))]))
            }
        }
        "ops_version" => {
            let limits = st::SettingsLimits {
                ops_version: field(&c, "ops_version")
                    .and_then(Value::as_i64)
                    .unwrap_or(1),
            };
            Ok(obj(vec![(
                "version",
                s(&st::ops_version(&ops_of(field(&c, "ops")), &limits)),
            )]))
        }
        "settings_path" => Ok(obj(vec![(
            "path",
            s(&st::settings_path(&env).to_string_lossy()),
        )])),
        "read_text" => {
            return match st::read_text(Path::new(&text_of(&c, "path"))) {
                Ok(t) => ok(obj(vec![("text", s(&t.unwrap_or_default()))]), &roots),
                Err(e) => fail(unsub(&exc_text(&e), &roots)),
            };
        }
        "plugin_rule_status" => {
            let root = PathBuf::from(text_of(&c, "root"));
            Ok(obj(vec![(
                "status",
                s(st::plugin_rule_status(&env, &root)),
            )]))
        }
        "setup_ops" => {
            let root = PathBuf::from(text_of(&c, "root"));
            let roots_list: Option<Vec<String>> = field(&c, "profile_roots")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .map(|x| x.as_str().unwrap_or_default().to_string())
                        .collect()
                });
            let refs: Option<Vec<&str>> = roots_list
                .as_ref()
                .map(|r| r.iter().map(String::as_str).collect());
            st::setup_ops_with(&root, &env, refs.as_deref()).map(|ops| {
                obj(vec![
                    ("ops", arr(ops.iter().map(st::Op::to_value).collect())),
                    ("rule", s(&st::plugin_read_rule(&root))),
                ])
            })
        }
        "write_text" => {
            let path = PathBuf::from(text_of(&c, "path"));
            return match st::write_text(&path, &text) {
                Ok(()) => {
                    let (dir, _) = sandbox.as_ref().expect("write_text has a sandbox");
                    ok(obj(vec![("tree", tree(dir.path(), &roots))]), &roots)
                }
                Err(e) => fail(exc_text(&e)),
            };
        }
        other => Err(format!("unknown case op {other:?}")),
    };
    match res {
        Ok(v) => ok(v, &roots),
        Err(e) => fail(unsub(&err_text(&e), &roots)),
    }
}

// ---------------------------------------------------------------------------------------------
// Documents
// ---------------------------------------------------------------------------------------------

/// A generated document: objects keep repeated keys, scalars are raw tokens.
#[derive(Clone, Debug)]
enum G {
    Obj(Vec<(String, G)>),
    Arr(Vec<G>),
    Raw(String),
}

const RULES: &[&str] = &[
    "Bash(rtk *)",
    "Bash(anthology-mem save *)",
    "Bash(anthology-mem save:*)",
    "Bash(ls)",
    "Bash(git push *)",
    "Read(.env)",
    "Read(~/.config/chromium)",
    "Read(~/.config/chromium/**)",
    "Read(~/Library/Application Support/Google)",
    "Read(//opt/plugins/**)",
    "Read(//srv/p/**)",
    "Bash(echo é)",
];

const SECRETS: &[&str] = &[
    "hunter2",
    "sk-live-123",
    "Bearer abc.def",
    "http://u:s3cr3t-cookie@proxy",
    "curl https://hooks.example/XYZSECRET",
    "opus",
];

const PLUGIN_RULES: &[&str] = &[
    "Read(//opt/plugins/**)",
    "Read(//srv/p/**)",
    "Read(//w/plugins/**)",
];

fn quoted(rng: &mut Rng, x: &str) -> String {
    let o = if rng.chance(0.5) {
        &PY_DEFAULT
    } else {
        &PY_DEFAULT_UNICODE
    };
    json::dumps(&s(x), o)
}

fn gen_string(rng: &mut Rng) -> String {
    match rng.below(8) {
        0 | 1 => rng.pick(RULES).to_string(),
        2 => rng.pick(SECRETS).to_string(),
        3 => rng
            .pick(&[
                "head", "fresh", "", "x", "\u{2028}", "a\u{85}b", "\u{feff}", "é✓",
            ])
            .to_string(),
        4 => rng.ascii_string(10),
        _ => rng.string(8),
    }
}

/// A raw scalar token as JSON spells it (including spellings `json.dumps` never writes).
fn gen_scalar(rng: &mut Rng) -> String {
    match rng.below(12) {
        0 => "null".into(),
        1 => "true".into(),
        2 => "false".into(),
        3 => rng.range(-300, 300).to_string(),
        4 => rng
            .pick(&[
                "-0",
                "0",
                "1E+5",
                "1e-2",
                "0.50",
                "-0.0",
                "1.5E3",
                "1.5e3",
                "123456789012345678901234567890",
                "-9223372036854775809",
                "9007199254740993",
                "1e400",
                "1.0",
                "2.5e-324",
            ])
            .to_string(),
        5 => {
            let f = rng.float_bits();
            json::dumps(&Value::Float(f), &PY_DEFAULT)
        }
        6 => rng.pick(ESCAPED).to_string(),
        _ => {
            let x = gen_string(rng);
            quoted(rng, &x)
        }
    }
}

/// Strings in spellings `json.dumps` never writes: lone surrogates (D-6 territory, but only
/// decoded values compare, and no op names U+FFFD), escaped rule characters, raw controls
/// escaped, a pair written as escapes.
const ESCAPED: &[&str] = &[
    r#""\ud800""#,
    r#""x\udc00y""#,
    r#""\ud83d\ude00""#,
    r#""Bash(rtk \u002a)""#,
    r#""Bash(rtk *)\u0000""#,
    r#""\u2028\n\t\/""#,
    r#""head\u0020""#,
    r#""\u0068ead""#,
];

fn gen_g(rng: &mut Rng, depth: u32) -> G {
    if depth == 0 || rng.chance(0.45) {
        return G::Raw(gen_scalar(rng));
    }
    if rng.chance(0.5) {
        let n = rng.index(4);
        G::Arr((0..n).map(|_| gen_g(rng, depth - 1)).collect())
    } else {
        let n = rng.index(4);
        G::Obj(
            (0..n)
                .map(|_| {
                    let k = gen_key(rng);
                    (k, gen_g(rng, depth - 1))
                })
                .collect(),
        )
    }
}

fn gen_key(rng: &mut Rng) -> String {
    match rng.below(6) {
        0 => rng
            .pick(&["permissions", "allow", "deny", "worktree", "baseRef", "env"])
            .to_string(),
        1 => rng
            .pick(&["model", "hooks", "command", "API_TOKEN", "[]", "a.b", ""])
            .to_string(),
        2 => rng.string(5),
        _ => rng.ascii_string(6),
    }
}

fn gen_rule_list(rng: &mut Rng) -> G {
    // Now and then a long list with repeats: more than 200 lines once laid out, so the diff's
    // SequenceMatcher drops popular lines (difflib's autojunk).
    let n = if rng.chance(0.04) {
        150 + rng.index(300)
    } else {
        rng.index(5)
    };
    G::Arr(
        (0..n)
            .map(|_| match rng.below(20) {
                0..=15 => {
                    let r = rng.pick(RULES).to_string();
                    G::Raw(quoted(rng, &r))
                }
                16 => G::Raw(rng.pick(ESCAPED).to_string()),
                _ => gen_g(rng, 1),
            })
            .collect(),
    )
}

/// A long `env`-like object of repeated keys and values (popular redacted lines).
fn gen_long_object(rng: &mut Rng) -> G {
    let n = 150 + rng.index(300);
    G::Obj(
        (0..n)
            .map(|_| {
                let k = rng.pick(&["N", "A", "API_TOKEN", "PLAIN"]).to_string();
                (k, G::Raw(gen_scalar(rng)))
            })
            .collect(),
    )
}

/// A settings-shaped document.
fn gen_doc(rng: &mut Rng) -> G {
    let mut members = Vec::new();
    let n = rng.index(6);
    for _ in 0..n {
        let entry = match rng.below(10) {
            0..=2 => {
                let mut perms = Vec::new();
                for key in ["allow", "deny", "ask"] {
                    if rng.chance(0.6) {
                        let v = if rng.chance(0.9) {
                            gen_rule_list(rng)
                        } else {
                            gen_g(rng, 1)
                        };
                        perms.push((key.to_string(), v));
                    }
                }
                if rng.chance(0.1) {
                    perms.push(("defaultMode".into(), G::Raw("\"acceptEdits\"".into())));
                }
                let v = if rng.chance(0.92) {
                    G::Obj(perms)
                } else {
                    gen_g(rng, 1)
                };
                ("permissions".to_string(), v)
            }
            3 | 4 => {
                let v = if rng.chance(0.85) {
                    let mut wt = Vec::new();
                    if rng.chance(0.6) {
                        let b = match rng.below(4) {
                            0 => G::Raw("\"head\"".into()),
                            1 => G::Raw("\"fresh\"".into()),
                            _ => gen_g(rng, 1),
                        };
                        wt.push(("baseRef".to_string(), b));
                    }
                    if rng.chance(0.3) {
                        wt.push(("other".into(), G::Raw(gen_scalar(rng))));
                    }
                    G::Obj(wt)
                } else {
                    gen_g(rng, 1)
                };
                ("worktree".to_string(), v)
            }
            5 if rng.chance(0.08) => ("env".to_string(), gen_long_object(rng)),
            5 => {
                let n = rng.index(4);
                let env = (0..n)
                    .map(|_| {
                        let k = rng
                            .pick(&["API_TOKEN", "PLAIN", "SECRET", "N", "A"])
                            .to_string();
                        (k, G::Raw(gen_scalar(rng)))
                    })
                    .collect();
                ("env".to_string(), G::Obj(env))
            }
            _ => (gen_key(rng), gen_g(rng, 3)),
        };
        members.push(entry);
    }
    // A repeated key now and then: the scanner takes the last one, like json.
    if !members.is_empty() && rng.chance(0.08) {
        let dup = members[rng.index(members.len())].clone();
        let at = rng.index(members.len() + 1);
        members.insert(at, dup);
    }
    G::Obj(members)
}

/// How a document is laid out.
struct Layout {
    unit: Option<String>,
    nl: &'static str,
    colon: &'static str,
    comma: &'static str,
    mixed: f64,
    empty_inner: &'static str,
}

fn gen_layout(rng: &mut Rng) -> Layout {
    let unit = match rng.below(8) {
        0 | 1 => Some("  ".to_string()),
        2 => Some("    ".to_string()),
        3 => Some("\t".to_string()),
        4 => Some(rng.pick(&[" ", "  \t", "\t "]).to_string()),
        _ => None,
    };
    Layout {
        unit,
        nl: rng.pick(&["\n", "\n", "\n", "\r\n", "\r"]),
        colon: rng.pick(&[": ", ": ", ":", " : ", " :", "\n:\n", " :\n  ", ":\r\n"]),
        comma: rng.pick(&[", ", ",", ",", " ,", " , ", "\n,", ",\n ", " ,\n"]),
        mixed: if rng.chance(0.3) { 0.3 } else { 0.0 },
        empty_inner: rng.pick(&["", "", "", " ", "\n"]),
    }
}

fn render_g(rng: &mut Rng, g: &G, indent: &str, lay: &Layout, out: &mut String) {
    let (open, close, n) = match g {
        G::Raw(r) => {
            out.push_str(r);
            return;
        }
        G::Obj(m) => ("{", "}", m.len()),
        G::Arr(a) => ("[", "]", a.len()),
    };
    if n == 0 {
        out.push_str(open);
        out.push_str(lay.empty_inner);
        out.push_str(close);
        return;
    }
    let multi = lay.unit.is_some() && !rng.chance(lay.mixed);
    let inner = format!("{indent}{}", lay.unit.as_deref().unwrap_or(""));
    out.push_str(open);
    for i in 0..n {
        if i > 0 {
            out.push_str(if multi { "," } else { lay.comma });
        }
        if multi {
            out.push_str(lay.nl);
            out.push_str(&inner);
        }
        match g {
            G::Obj(m) => {
                let (k, v) = &m[i];
                out.push_str(&quoted(rng, k));
                out.push_str(lay.colon);
                render_g(rng, v, &inner, lay, out);
            }
            G::Arr(a) => render_g(rng, &a[i], &inner, lay, out),
            G::Raw(_) => unreachable!(),
        }
    }
    if multi {
        out.push_str(lay.nl);
        out.push_str(indent);
    }
    out.push_str(close);
}

fn render_doc(rng: &mut Rng, g: &G) -> String {
    let lay = gen_layout(rng);
    let mut out = String::new();
    if rng.chance(0.08) {
        out.push('\u{feff}');
    }
    if rng.chance(0.05) {
        out.push_str(rng.pick(&[" ", "\n", "\r\n", "\t"]));
    }
    render_g(rng, g, "", &lay, &mut out);
    if rng.chance(0.6) {
        out.push_str(lay.nl);
    }
    if rng.chance(0.03) {
        out.push_str(rng.pick(&[" ", "\n\n", "\t\r\n"]));
    }
    out
}

/// The `deep` cases: `(text, hint)` for documents deeper than the scanner allows.
fn deep_text(rng: &mut Rng) -> (String, String) {
    let n = *rng.pick(&[99usize, 100, 101, 102, 3000, 20_000, 100_000]);
    match rng.below(3) {
        0 => (
            "[".repeat(n),
            format!(
                "invalid:settings.json is not valid JSON: Expecting value: line 1 column {} (char {n})",
                n + 1
            ),
        ),
        1 => (
            format!("{}{}", "[".repeat(n), "]".repeat(n)),
            "valid".into(),
        ),
        _ => (
            format!("{}1{}", "{\"a\":".repeat(n), "}".repeat(n)),
            "valid".into(),
        ),
    }
}

/// A broken text.
fn break_text(rng: &mut Rng, text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    match rng.below(11) {
        0 if !chars.is_empty() => chars[..rng.index(chars.len())].iter().collect(),
        1 => match text.rfind(['}', ']']) {
            Some(i) => format!("{},{}", &text[..i], &text[i..]),
            None => format!("{text},"),
        },
        2 => format!("{text} trailing"),
        3 => format!("// c\n{text}"),
        4 => text.replacen("null", rng.pick(&["NaN", "Infinity", "-Infinity"]), 1),
        5 => rng
            .pick(&[
                "[1, 2]",
                "\"x\"",
                "null",
                "3",
                "true",
                "[]",
                "{\"a\": 1} {}",
                "{\"a\" 1}",
            ])
            .to_string(),
        6 => format!("\u{feff}\u{feff}{text}"),
        7 => text.replacen('"', "\"\u{1}", 1),
        8 => format!("{text}\u{0}"),
        9 => rng
            .pick(&[
                "",
                " ",
                "\n",
                "\u{feff}",
                "\u{feff} \r\n",
                " \u{feff}",
                "\u{3000}",
                "\t\u{feff}\n",
            ])
            .to_string(),
        _ => text.replacen(':', "", 1),
    }
}

/// A settings text: mostly valid documents, some broken, a few deep.
fn gen_text(rng: &mut Rng) -> String {
    let g = gen_doc(rng);
    let t = render_doc(rng, &g);
    if rng.chance(0.12) {
        break_text(rng, &t)
    } else {
        t
    }
}

// ---------------------------------------------------------------------------------------------
// Ops
// ---------------------------------------------------------------------------------------------

/// The shape of `setup_ops` (its deny rules shortened), with a plugin-read rule and a
/// memory-save rule that a release may change.
fn setup_like(rng: &mut Rng) -> Vec<Value> {
    let mem = if rng.chance(0.8) {
        "Bash(anthology-mem save *)"
    } else {
        "Bash(anthology-mem save:*)"
    };
    let mut ops = vec![
        op_add(&["permissions", "allow"], s("Bash(rtk *)"), None),
        op_add(&["permissions", "allow"], s(mem), Some("mem-save")),
        op_add(
            &["permissions", "deny"],
            s("Read(~/.config/chromium)"),
            None,
        ),
        op_add(
            &["permissions", "deny"],
            s("Read(~/.config/chromium/**)"),
            None,
        ),
        op_add(
            &["permissions", "deny"],
            s("Read(~/Library/Application Support/Google)"),
            None,
        ),
        op_add(
            &["permissions", "allow"],
            s(rng.pick(PLUGIN_RULES)),
            Some("plugin-read"),
        ),
        op_set(&["worktree", "baseRef"], s("head"), None),
    ];
    if rng.chance(0.15) {
        rng.shuffle(&mut ops);
    }
    if rng.chance(0.15) {
        let k = rng.index(ops.len());
        ops.remove(k);
    }
    ops
}

const PATH_WORDS: &[&str] = &[
    "permissions",
    "allow",
    "deny",
    "worktree",
    "baseRef",
    "env",
    "x",
    "",
    "a.b",
    "é",
];

fn gen_value(rng: &mut Rng) -> Value {
    match rng.below(10) {
        0 => Value::Null,
        1 => Value::Bool(rng.chance(0.5)),
        2 => int(rng.range(-3, 3)),
        3 => Value::Float(*rng.pick(&[1.0, 0.0, -0.0, 1.5, f64::NAN, 1e22, 0.1])),
        4 => json::loads(rng.pick(&[
            r#"{"a": [1, {"b": null}], "c": "é"}"#,
            "[]",
            "{}",
            r#"["Bash(rtk *)"]"#,
            r#"{"baseRef": "head"}"#,
        ]))
        .unwrap_or(Value::Null),
        5 => s(&rng.string(6)),
        _ => s(rng.pick(RULES)),
    }
}

fn gen_random_ops(rng: &mut Rng) -> Vec<Value> {
    let n = 1 + rng.index(5);
    (0..n)
        .map(|_| {
            let len = rng.index(4);
            let path: Vec<&str> = (0..len).map(|_| *rng.pick(PATH_WORDS)).collect();
            let tag = match rng.below(6) {
                0 => Some("plugin-read"),
                1 => Some("mem-save"),
                2 => Some("other"),
                _ => None,
            };
            let value = gen_value(rng);
            if rng.chance(0.6) {
                op_add(&path, value, tag)
            } else {
                op_set(&path, value, tag)
            }
        })
        .collect()
}

fn gen_ops(rng: &mut Rng) -> Vec<Value> {
    let mut ops = match rng.below(10) {
        0..=5 => setup_like(rng),
        6..=8 => gen_random_ops(rng),
        _ => {
            let mut a = setup_like(rng);
            a.extend(gen_random_ops(rng));
            a
        }
    };
    // The same op twice in one list.
    if !ops.is_empty() && rng.chance(0.1) {
        let k = rng.index(ops.len());
        let dup = ops[k].clone();
        let at = rng.index(ops.len() + 1);
        ops.insert(at, dup);
    }
    ops
}

/// Stored records from a real plan of `ops` over `text` (none when the plan fails).
fn planned(text: &str, ops: &[Value]) -> Option<(String, Vec<Value>)> {
    let typed: Vec<st::Op> = ops.iter().filter_map(st::Op::from_value).collect();
    let p = st::plan(text, &typed).ok()?;
    let (new, _) = st::apply(text, &p.ops).ok()?;
    Some((new, p.stored))
}

/// What a user (or a broken state.json) does to a stored record.
fn mutate_record(rng: &mut Rng, rec: &Value) -> Value {
    let Some(m) = rec.as_object() else {
        return rec.clone();
    };
    let mut m = m.clone();
    let keys: Vec<String> = m.iter().map(|(k, _)| k.to_string()).collect();
    match rng.below(14) {
        0 if !keys.is_empty() => {
            let k = rng.pick(&keys).clone();
            m.remove(&k);
        }
        1 => m.insert(
            "path".into(),
            rng.pick(&[
                s("permissions"),
                obj(vec![("permissions", int(1))]),
                int(3),
                Value::Null,
                arr(vec![int(1)]),
                arr(vec![arr(vec![])]),
            ])
            .clone(),
        ),
        2 => m.insert(
            "created".into(),
            rng.pick(&[
                s("permissions"),
                obj(vec![("permissions", int(1))]),
                int(1),
                Value::Bool(true),
                arr(vec![]),
                arr(vec![arr(vec![s("permissions")])]),
                arr(vec![arr(vec![int(1)])]),
                arr(vec![s("ab")]),
            ])
            .clone(),
        ),
        3 => m.insert(
            "empty".into(),
            rng.pick(&[
                obj(vec![]),
                s("x"),
                arr(vec![]),
                obj(vec![("path", s("permissions")), ("raw", s("[]"))]),
                obj(vec![
                    ("path", arr(vec![])),
                    ("raw", s("{}")),
                    ("whole", Value::Bool(true)),
                ]),
                obj(vec![
                    ("path", arr(vec![])),
                    ("raw", int(5)),
                    ("whole", int(1)),
                ]),
                obj(vec![("path", arr(vec![s("permissions")]))]),
                int(1),
            ])
            .clone(),
        ),
        4 => m.insert(
            "previous".into(),
            rng.pick(&[
                obj(vec![("present", int(1)), ("raw", s("\"x\""))]),
                arr(vec![]),
                s("x"),
                obj(vec![("present", Value::Bool(false))]),
                obj(vec![("present", Value::Bool(true))]),
                obj(vec![("present", Value::Bool(true)), ("raw", int(3))]),
                obj(vec![("present", s("yes")), ("raw", s("not json"))]),
                int(0),
            ])
            .clone(),
        ),
        5 => m.insert(
            "op".into(),
            rng.pick(&[s("set"), s("add"), s("bogus"), int(1), arr(vec![])])
                .clone(),
        ),
        6 => m.insert("value".into(), gen_value(rng)),
        7 => m.insert(
            "tag".into(),
            rng.pick(&[s("plugin-read"), s("mem-save"), int(1), arr(vec![])])
                .clone(),
        ),
        8 => {
            m.insert("tag".into(), s("plugin-read"));
            m.insert(
                "value".into(),
                rng.pick(&[
                    arr(vec![]),
                    obj(vec![]),
                    Value::Float(f64::NAN),
                    s("Read(//old/**)"),
                ])
                .clone(),
            );
        }
        9 => m.insert("raw".into(), s("x")),
        _ => {
            // Retarget the path into or past what the text holds.
            m.insert(
                "path".into(),
                path_strs(&[rng.pick(PATH_WORDS), rng.pick(PATH_WORDS)]),
            );
        }
    }
    Value::Object(m)
}

fn mutate_records(rng: &mut Rng, stored: &mut Vec<Value>) {
    let n = 1 + rng.index(3);
    for _ in 0..n {
        match rng.below(8) {
            0 => stored.push(
                rng.pick(&[s("x"), int(1), Value::Null, arr(vec![]), obj(vec![])])
                    .clone(),
            ),
            1 if !stored.is_empty() => {
                let k = rng.index(stored.len());
                let dup = stored[k].clone();
                stored.push(dup);
            }
            _ if !stored.is_empty() => {
                let k = rng.index(stored.len());
                stored[k] = mutate_record(rng, &stored[k]);
            }
            _ => {}
        }
    }
}

/// A user's edit of a settings text between setup and remove.
fn edit_text(rng: &mut Rng, text: &str) -> String {
    match rng.below(7) {
        0 => text.replacen(
            "\"head\"",
            rng.pick(&["\"fresh\"", "1", "\"head\" ", "null"]),
            1,
        ),
        1 => text.replacen("\"Bash(rtk *)\"", "\"Bash(make *)\"", 1),
        2 => text.replacen("\"Bash(rtk *)\",", "", 1),
        3 => text.replacen("[", "[\"Bash(pwd)\", ", 1),
        4 => match st::redact(text) {
            Ok(_) => text.replacen('{', "{\"extra\": {\"k\": [1]}, ", 1),
            Err(_) => text.to_string(),
        },
        5 => text.replace('\n', "\r\n"),
        _ => break_text(rng, text),
    }
}

// ---------------------------------------------------------------------------------------------
// Filesystem cases
// ---------------------------------------------------------------------------------------------

fn file_text(path: &str, text: &str) -> Value {
    obj(vec![
        ("path", s(path)),
        ("type", s("file")),
        ("text", s(text)),
    ])
}

fn file_hex(path: &str, h: &str) -> Value {
    obj(vec![("path", s(path)), ("type", s("file")), ("hex", s(h))])
}

fn dir(path: &str) -> Value {
    obj(vec![("path", s(path)), ("type", s("dir"))])
}

fn link(path: &str, target: &str) -> Value {
    obj(vec![
        ("path", s(path)),
        ("type", s("link")),
        ("target", s(target)),
    ])
}

fn manifest(parent: &str, body: &str) -> Value {
    file_text(
        &format!("{parent}/anthology/.claude-plugin/plugin.json"),
        body,
    )
}

fn gen_home_env(rng: &mut Rng) -> Value {
    match rng.below(6) {
        0 => obj(vec![("HOME", s("<ROOT>/home/"))]),
        1 => obj(vec![]),
        2 => obj(vec![("HOME", s(""))]),
        _ => obj(vec![("HOME", s("<ROOT>/home"))]),
    }
}

/// Picks for a manifest that names anthology and nests past CPython's json limit (about
/// 10,000 levels: Python crashes with RecursionError, Rust refuses), or 900 deep (both load).
const DEEP_MANIFEST: &str = "<deep manifest>";
const SHALLOW_DEEP_MANIFEST: &str = "<shallow deep manifest>";

fn manifest_body(pick: &str) -> String {
    let nested = |n: usize| {
        format!(
            "{{\"name\": \"anthology\", \"x\": {}{}}}",
            "[".repeat(n),
            "]".repeat(n)
        )
    };
    match pick {
        DEEP_MANIFEST => nested(12_000),
        SHALLOW_DEEP_MANIFEST => nested(900),
        other => other.to_string(),
    }
}

fn gen_setup_ops_case(rng: &mut Rng) -> Value {
    let mut fs_spec = vec![dir("home")];
    let good = r#"{"name": "anthology"}"#;
    let root: String = match rng.below(16) {
        0 => rng
            .pick(&[
                "",
                "plugins/anthology",
                "./x",
                "it's",
                "say \"hi\"",
                "tab\tx",
                "é/anthology",
                "a'\"b",
            ])
            .to_string(),
        1 => "<ROOT>/missing/anthology".into(),
        2 => {
            fs_spec.push(file_text("file", "x"));
            "<ROOT>/file".into()
        }
        3 => {
            fs_spec.push(dir("plugins/anthology"));
            "<ROOT>/plugins/anthology".into()
        }
        4 => {
            let body = rng.pick(&[
                r#"{"name": "something-else"}"#,
                "\u{feff}{\"name\": \"anthology\"}",
                DEEP_MANIFEST,
                SHALLOW_DEEP_MANIFEST,
                "{nope",
                r#"["anthology"]"#,
                r#"{"name": 1}"#,
                r#"{"name": "anthology", "name": "x"}"#,
                r#"{"name": "x", "name": "anthology"}"#,
                "null",
            ]);
            fs_spec.push(manifest("plugins", &manifest_body(body)));
            "<ROOT>/plugins/anthology".into()
        }
        5 => {
            fs_spec.push(file_hex(
                "plugins/anthology/.claude-plugin/plugin.json",
                "7b226e616d65223a2022616e74686f6c6f6779ff227d",
            ));
            "<ROOT>/plugins/anthology".into()
        }
        6 => {
            fs_spec.push(manifest("home", good));
            "<ROOT>/home/anthology".into()
        }
        7 => {
            fs_spec.push(manifest("", good));
            "<ROOT>/anthology".into()
        }
        8 => "/".into(),
        9 => {
            let ch = rng.pick(&["*", "?", "[", "]", "{", "}", "(", ")", "\\"]);
            let parent = format!("plug{ch}ins");
            fs_spec.push(manifest(&parent, good));
            format!("<ROOT>/{parent}/anthology")
        }
        10 => {
            fs_spec.push(manifest("home", good));
            fs_spec.push(link("safe/anthology", "<ROOT>/home/anthology"));
            "<ROOT>/safe/anthology".into()
        }
        11 => {
            fs_spec.push(manifest("plugins", good));
            rng.pick(&[
                "<ROOT>/plugins/anthology/",
                "<ROOT>//plugins/./anthology",
                "<ROOT>/plugins/x/../anthology",
                "<REAL>/plugins/anthology",
            ])
            .to_string()
        }
        12 => {
            fs_spec.push(manifest("real", good));
            fs_spec.push(link("plugins", "real"));
            "<ROOT>/plugins/anthology".into()
        }
        13 => {
            fs_spec.push(link("loop", "loop"));
            "<ROOT>/loop/anthology".into()
        }
        _ => {
            fs_spec.push(manifest("plugins", good));
            "<ROOT>/plugins/anthology".into()
        }
    };
    let mut env = gen_home_env(rng);
    if rng.chance(0.1) {
        // A home below the plugin's parent.
        fs_spec.push(dir("plugins/deep/home"));
        env = obj(vec![("HOME", s("<ROOT>/plugins/deep/home"))]);
    }
    let profile_roots = match rng.below(3) {
        0 => Value::Null,
        1 => arr(vec![
            s("Library/Application Support/Google"),
            s(".config/chromium"),
        ]),
        _ => arr(vec![s(".config/x"), s("é dir")]),
    };
    let mut pairs = vec![
        ("op", s("setup_ops")),
        ("fs", arr(fs_spec)),
        ("root", s(&root)),
        ("env", env),
    ];
    if !profile_roots.is_null() {
        pairs.push(("profile_roots", profile_roots));
    }
    obj(pairs)
}

fn gen_status_case(rng: &mut Rng) -> Value {
    let rule = "Read(/<REAL>/plugins/**)";
    let mut fs_spec = vec![dir("c"), manifest("plugins", r#"{"name": "anthology"}"#)];
    let mut deep = None;
    let content: Option<String> = match rng.below(10) {
        0 => None,
        1 => Some(format!("{{\"permissions\": {{\"allow\": [\"{rule}\"]}}}}")),
        2 => Some(format!(
            "\u{feff}{{\"permissions\": {{\"allow\": [\"Bash(ls)\", \"{rule}\"]}}}}"
        )),
        3 => Some(format!(
            "{{\"permissions\": {{\"allow\": \"xx{rule}yy\"}}}}"
        )),
        4 => Some("{nope".into()),
        5 if rng.chance(0.5) => {
            let (t, hint) = deep_text(rng);
            deep = Some(hint);
            Some(t)
        }
        5 => {
            // The rule, beside a value deeper than CPython's json can load (Python reads the
            // file as unreadable; Rust finds the rule, D-1) or within it (both find it).
            let n = *rng.pick(&[900usize, 12_000]);
            deep = Some("valid-ok".to_string());
            Some(format!(
                "{{\"permissions\": {{\"allow\": [\"{rule}\"]}}, \"x\": {}{}}}",
                "[".repeat(n),
                "]".repeat(n)
            ))
        }
        6 => Some(
            rng.pick(&[
                "",
                " \n",
                "[]",
                "{\"permissions\": []}",
                "{\"permissions\": {\"allow\": [\"Read(//old/**)\"]}}",
            ])
            .to_string(),
        ),
        7 => {
            fs_spec.push(file_hex("c/settings.json", "fffe7b007d00"));
            None
        }
        _ => Some(gen_text(rng)),
    };
    if let Some(t) = content {
        fs_spec.push(file_text("c/settings.json", &t));
    }
    // A "valid-ok" deep file holds this root's rule, so its root stays the plugin's.
    let root = if deep.as_deref() != Some("valid-ok") && rng.chance(0.2) {
        "<ROOT>/elsewhere/anthology"
    } else {
        "<ROOT>/plugins/anthology"
    };
    let mut pairs = vec![
        ("op", s("plugin_rule_status")),
        ("fs", arr(fs_spec)),
        ("root", s(root)),
        (
            "env",
            obj(vec![
                ("HOME", s("<ROOT>")),
                ("CLAUDE_CONFIG_DIR", s("<ROOT>/c")),
            ]),
        ),
    ];
    if let Some(d) = deep {
        pairs.push(("deep", s(&d)));
    }
    obj(pairs)
}

fn gen_read_case(rng: &mut Rng) -> Value {
    let utf16 = |t: &str, be: bool, bom: bool| {
        let mut b: Vec<u8> = Vec::new();
        if bom {
            b.extend(if be { [0xfe, 0xff] } else { [0xff, 0xfe] });
        }
        for u in t.encode_utf16() {
            b.extend(if be { u.to_be_bytes() } else { u.to_le_bytes() });
        }
        hex(&b)
    };
    let doc = gen_text(rng);
    let h = match rng.below(10) {
        0 => utf16(&doc, false, true),
        1 => utf16(&doc, true, true),
        2 => utf16(&doc, false, false),
        8 => utf16(&doc, true, false),
        3 => hex(&doc
            .chars()
            .map(|c| (c as u32 & 0xff) as u8)
            .collect::<Vec<_>>()),
        4 => {
            let b = doc.as_bytes();
            let cut = rng.index(b.len() + 1);
            hex(&b[..cut])
        }
        5 => rng
            .pick(&[
                "eda080",
                "c080",
                "efbbbfff",
                "e282",
                "f4908080",
                "7b0d0a7d0d0a",
                "41ed9fbf42",
                "f0908d",
            ])
            .to_string(),
        _ => hex(doc.as_bytes()),
    };
    let fs_spec = match rng.below(10) {
        0 => vec![dir("settings.json")],
        1 => vec![],
        _ => vec![file_hex("settings.json", &h)],
    };
    obj(vec![
        ("op", s("read_text")),
        ("fs", arr(fs_spec)),
        ("path", s("<ROOT>/settings.json")),
    ])
}

fn gen_settings_path_case(rng: &mut Rng) -> Value {
    let mut env = Vec::new();
    if rng.chance(0.6) {
        env.push((
            "CLAUDE_CONFIG_DIR",
            s(rng.pick(&[
                "/c", "/c/", "", "c//d/./", "//c", "///c", ".", "~/x", "/c/../d", "/é",
            ])),
        ));
    }
    if rng.chance(0.8) {
        env.push((
            "HOME",
            s(rng.pick(&["/h", "/h/", "", "//h", "/h//x/.", "h"])),
        ));
    }
    obj(vec![("op", s("settings_path")), ("env", obj(env))])
}

fn gen_write_case(rng: &mut Rng) -> Value {
    let text = gen_text(rng);
    let (fs_spec, path): (Vec<Value>, &str) = match rng.below(8) {
        0 => {
            let mut f = file_text("settings.json", "{}\r\n");
            if let Value::Object(m) = &mut f {
                m.insert("mode".into(), int(*rng.pick(&[0o640, 0o600, 0o755, 0o444])));
            }
            (vec![f], "<ROOT>/settings.json")
        }
        1 => (
            vec![
                file_text("dotfiles/settings.json", "{}"),
                link("settings.json", "<ROOT>/dotfiles/settings.json"),
            ],
            "<ROOT>/settings.json",
        ),
        2 => (
            vec![link("settings.json", "dotfiles/new/settings.json")],
            "<ROOT>/settings.json",
        ),
        3 => (vec![], "<ROOT>/cfg/deeper/settings.json"),
        4 => (vec![dir("settings.json")], "<ROOT>/settings.json"),
        5 => (vec![file_text("cfg", "x")], "<ROOT>/cfg/settings.json"),
        6 => (
            vec![link("settings.json", "settings.json")],
            "<ROOT>/settings.json",
        ),
        _ => (vec![], "<ROOT>/settings.json"),
    };
    obj(vec![
        ("op", s("write_text")),
        ("fs", arr(fs_spec)),
        ("path", s(path)),
        ("text", s(&text)),
    ])
}

// ---------------------------------------------------------------------------------------------
// Cases
// ---------------------------------------------------------------------------------------------

fn text_case(op: &str, text: &str) -> Vec<(&'static str, Value)> {
    let op: &'static str = match op {
        "plan" => "plan",
        "apply" => "apply",
        "revert" => "revert",
        "reconcile" => "reconcile",
        "redact" => "redact",
        _ => "diff",
    };
    vec![("op", s(op)), ("text", s(text))]
}

fn gen_case(rng: &mut Rng) -> Value {
    let k = rng.below(100);
    if rng.chance(0.03) {
        let (t, hint) = deep_text(rng);
        let op = *rng.pick(&["plan", "apply", "revert", "redact", "reconcile"]);
        let mut pairs = text_case(op, &t);
        pairs.push(("ops", arr(setup_like(rng))));
        pairs.push(("stored", arr(vec![])));
        pairs.push(("deep", s(&hint)));
        return obj(pairs);
    }
    match k {
        0..=17 => {
            let mut pairs = text_case("apply", &gen_text(rng));
            pairs.push(("ops", arr(gen_ops(rng))));
            obj(pairs)
        }
        18..=27 => {
            let mut pairs = text_case("plan", &gen_text(rng));
            pairs.push((
                "ops",
                arr(if rng.chance(0.05) {
                    vec![]
                } else {
                    gen_ops(rng)
                }),
            ));
            obj(pairs)
        }
        28..=49 => {
            let text = gen_text(rng);
            let ops = gen_ops(rng);
            let (mut new, mut stored) =
                planned(&text, &ops).unwrap_or_else(|| (text.clone(), ops.clone()));
            if rng.chance(0.4) {
                new = edit_text(rng, &new);
            }
            if rng.chance(0.35) {
                mutate_records(rng, &mut stored);
            }
            let mut pairs = text_case("revert", &new);
            pairs.push(("stored", arr(stored)));
            obj(pairs)
        }
        50..=71 => {
            let text = gen_text(rng);
            let first = setup_like(rng);
            let (mut t1, mut stored) = match rng.below(3) {
                0 => (text.clone(), vec![]),
                _ => planned(&text, &first).unwrap_or((text.clone(), vec![])),
            };
            if rng.chance(0.3) {
                t1 = edit_text(rng, &t1);
            }
            if rng.chance(0.3) {
                mutate_records(rng, &mut stored);
            }
            let desired = if rng.chance(0.8) {
                setup_like(rng)
            } else {
                gen_ops(rng)
            };
            let mut pairs = text_case("reconcile", &t1);
            pairs.push(("ops", arr(desired)));
            pairs.push(("stored", arr(stored)));
            obj(pairs)
        }
        72..=79 => {
            let text = gen_text(rng);
            let t = if rng.chance(0.5) {
                planned(&text, &gen_ops(rng)).map_or(text, |(n, _)| n)
            } else {
                text
            };
            obj(text_case("redact", &t))
        }
        80..=84 => {
            let a = gen_text(rng);
            let b = if rng.chance(0.6) {
                planned(&a, &gen_ops(rng)).map_or_else(|| gen_text(rng), |(n, _)| n)
            } else if rng.chance(0.2) {
                a.clone()
            } else {
                gen_text(rng)
            };
            let mut pairs = text_case("diff", &a);
            pairs.push(("text2", s(&b)));
            obj(pairs)
        }
        85..=88 => obj(vec![
            ("op", s("ops_version")),
            ("ops", arr(gen_ops(rng))),
            (
                "ops_version",
                int(*rng.pick(&[1, 1, 2, 0, -1, 1_000_000_007])),
            ),
        ]),
        89..=92 => gen_setup_ops_case(rng),
        93..=95 => gen_status_case(rng),
        96 | 97 => gen_read_case(rng),
        98 => gen_settings_path_case(rng),
        _ => gen_write_case(rng),
    }
}

/// The inputs of `tests/test_settings_patch.py` (its fixtures, invalid texts and redaction
/// documents) under the setup op shape, as plan, apply, revert and reconcile cases.
fn seeds() -> Vec<Value> {
    let base = r#"{"model": "opus", "permissions": {"allow": ["Bash(ls)"], "deny": ["Read(.env)"]}, "env": {"A": "1"}}"#;
    let indent = |t: &str, unit: &str| {
        let v = json::loads(t).expect("seed JSON");
        let one = json::dumps(
            &v,
            &json::DumpOpts {
                indent: Some(1),
                sort_keys: false,
                ensure_ascii: false,
                item_sep: ",",
                key_sep: ": ",
            },
        );
        one.split('\n')
            .map(|line| {
                let n = line.len() - line.trim_start_matches(' ').len();
                format!("{}{}", unit.repeat(n), &line[n..])
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let compact = |t: &str| json::dumps(&json::loads(t).expect("seed JSON"), &json::COMPACT_SORTED);
    let base2 = indent(base, "  ") + "\n";
    let texts: Vec<String> = vec![
        base2.clone(),
        indent(base, "  "),
        indent(base, "    ") + "\n",
        indent(base, "\t") + "\n",
        r#"{"model":"opus","permissions":{"allow":["Bash(ls)"],"deny":["Read(.env)"]},"env":{"A":"1"}}"#.to_string() + "\n",
        base.to_string(),
        base2.replace('\n', "\r\n"),
        String::new(),
        "\n".into(),
        "{}".into(),
        "{}\n".into(),
        "{\n}\n".into(),
        indent(r#"{"model": "x"}"#, "  ") + "\n",
        indent(r#"{"permissions": {}}"#, "  ") + "\n",
        indent(r#"{"permissions": {"allow": [], "deny": []}}"#, "  ") + "\n",
        r#"{"permissions":{"allow":[],"deny":[]}}"#.into(),
        indent(r#"{"note": "héllo ✓", "permissions": {"allow": ["Bash(echo é)"]}}"#, "  ") + "\n",
        indent(r#"{"worktree": {}}"#, "  ") + "\n",
        indent(r#"{"worktree": {"baseRef": "fresh"}, "x": 1}"#, "  ") + "\n",
        r#"{"permissions":{"allow":["Bash(ls)"]}}"#.into(),
        indent(r#"{"hooks": {"A": [{"x": [1, 2, {"y": null}]}]}, "z": 1.5e3}"#, "  ") + "\n",
        format!("\u{feff}{base2}"),
        r#"{"permissions": {"allow": ["Bash(ls)"]}}"#.into(),
        compact(r#"{"env": {"API_TOKEN": "hunter2", "PLAIN": "sk-live-123"}, "model": "opus", "permissions": {"allow": ["Bash(ls)"]}, "mcp": {"headers": {"Authorization": "Bearer abc.def"}}, "hooks": {"Stop": [{"command": "curl https://hooks.example/XYZSECRET"}]}, "proxyUrl": "http://u:s3cr3t-cookie@proxy"}"#),
        "{\n  \"model\": \"opus\",\n  \"permissions\": {\"allow\": [\"Bash(ls)\"]},\n  \"env\": {\"API_TOKEN\": \"hunter2\", \"PLAIN\": \"sk-live-123\", \"N\": 3}\n}\n".into(),
        indent(r#"{"permissions": {"allow": ["Bash(ls)"]}, "apiKeyHelper": "/bin/echo sk-live-123"}"#, "  ") + "\n",
        indent(r#"{"permissions": {"allow": ["Bash(rtk *)", "Bash(ls)"]}}"#, "  ") + "\n",
        r#"{"a": 1,}"#.into(),
        "{".into(),
        "[1, 2]".into(),
        "\"x\"".into(),
        "null".into(),
        r#"{"a": 1} trailing"#.into(),
        "// c\n{}".into(),
        r#"{"permissions": []}"#.into(),
        r#"{"permissions": {"allow": "x"}}"#.into(),
        r#"{"worktree": 3}"#.into(),
        r#"{"permissions": {"deny": {}}}"#.into(),
    ];
    let mut rng = Rng::new(0x5e7);
    let ops = || {
        vec![
            op_add(&["permissions", "allow"], s("Bash(rtk *)"), None),
            op_add(
                &["permissions", "allow"],
                s("Bash(anthology-mem save *)"),
                Some("mem-save"),
            ),
            op_add(
                &["permissions", "deny"],
                s("Read(~/Library/Application Support/Google)"),
                None,
            ),
            op_add(
                &["permissions", "deny"],
                s("Read(~/Library/Application Support/Google/**)"),
                None,
            ),
            op_add(
                &["permissions", "deny"],
                s("Read(~/.config/chromium)"),
                None,
            ),
            op_add(
                &["permissions", "deny"],
                s("Read(~/.config/chromium/**)"),
                None,
            ),
            op_add(
                &["permissions", "allow"],
                s("Read(//w/plugins/**)"),
                Some("plugin-read"),
            ),
            op_set(&["worktree", "baseRef"], s("head"), None),
        ]
    };
    let mut out = Vec::new();
    for t in &texts {
        for op in ["plan", "apply", "redact"] {
            let mut pairs = text_case(op, t);
            pairs.push(("ops", arr(ops())));
            out.push(obj(pairs));
        }
        let (new, stored) = planned(t, &ops()).unwrap_or((t.clone(), ops()));
        let mut pairs = text_case("revert", &new);
        pairs.push(("stored", arr(stored.clone())));
        out.push(obj(pairs));
        let mut moved = ops();
        moved[6] = op_add(
            &["permissions", "allow"],
            s("Read(//srv/p/**)"),
            Some("plugin-read"),
        );
        let mut pairs = text_case("reconcile", &new);
        pairs.push(("ops", arr(moved)));
        pairs.push(("stored", arr(stored)));
        out.push(obj(pairs));
    }
    out.push(obj(vec![
        ("op", s("ops_version")),
        ("ops", arr(ops())),
        ("ops_version", int(1)),
    ]));
    out.push(obj(vec![
        ("op", s("ops_version")),
        ("ops", arr(ops())),
        ("ops_version", int(2)),
    ]));
    for _ in 0..40 {
        out.push(gen_setup_ops_case(&mut rng));
        out.push(gen_status_case(&mut rng));
        out.push(gen_read_case(&mut rng));
        out.push(gen_settings_path_case(&mut rng));
        out.push(gen_write_case(&mut rng));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// The generator reaches every op with both outcomes, and most applies change the text.
    #[test]
    fn generated_cases_cover_every_op_and_outcome() {
        let mut rng = Rng::for_surface(20261121, NAME);
        let mut tally: BTreeMap<(String, i32), usize> = BTreeMap::new();
        let mut changed = 0;
        for c in Settings.generate(&mut rng, 3000) {
            let o = run(&c.data);
            let op = text_of(&c.data, "op");
            if op == "apply" && o.exit == 0 {
                let out = json::loads(std::str::from_utf8(&o.stdout).unwrap()).unwrap();
                if text_of(&out, "text") != text_of(&c.data, "text") {
                    changed += 1;
                }
            }
            *tally.entry((op, o.exit)).or_default() += 1;
        }
        eprintln!("{tally:#?}\nchanged applies: {changed}");
        for op in [
            "plan",
            "apply",
            "revert",
            "reconcile",
            "redact",
            "diff",
            "setup_ops",
            "plugin_rule_status",
            "read_text",
            "write_text",
        ] {
            let n = |exit| tally.get(&(op.to_string(), exit)).copied().unwrap_or(0);
            assert!(n(0) > 0, "{op} never succeeds");
            if op != "plugin_rule_status" {
                assert!(n(1) > 0, "{op} never fails");
            }
        }
        assert!(changed > 100, "only {changed} applies changed the text");
    }
}
