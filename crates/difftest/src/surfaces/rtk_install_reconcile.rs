//! The `rtk_install.reconcile` difftest surface (RT2; oracle
//! `port/difftest/oracle/rtk_install.py`).
//!
//! A case is one `anthology rtk-install`-style run with an injected fetch:
//!
//! ```text
//! {"lock": <rtk.lock JSON value>, "uname": [SYSTEM, MACHINE],
//!  "blob": HEX, "serve": "lock" | "elsewhere" | "error", "error": [TYPE, MESSAGE],
//!  "pre": [{"path": REL, "kind": "file" | "dir" | "symlink", "data": HEX, "mode": N,
//!           "age": DAYS | null, "target": TEXT}],
//!  "state": TEXT | null, "max_binary_bytes": N | null, "max_tar_members": N | null}
//! ```
//!
//! Both sides build `<tmp>/data` from `pre` (in order; a symlink target may start with `<tmp>`),
//! write the prior state's `rtk_path`, validate the lock (an invalid lock is exit 2 with
//! `rtk_install: <msg>`, as the CLI prints it), then run `reconcile(data, lock, fetch, 5)` for
//! the given platform. The fake fetch serves `blob` at the lock's release URL (`lock`), at no
//! URL (`elsewhere`: Python's `KeyError`), or raises `TYPE(MESSAGE)` (`error`). The result is
//! printed as text: the three result fields, each fetch call, every entry under `data/`
//! (kind, mode, a sha256 prefix or a link target), and state.json's `rtk_path`, with the
//! temporary folder written as `<tmp>`.
//!
//! D-2: the rest of the line after `cannot read the rtk tarball: ` (Python's `tarfile`/`gzip`
//! versus the `tar`/`flate2` crates). The OS text of `[Errno N] <text>` is compared as is
//! (`pycompat`'s errno tables).

use std::fs;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;
use std::time::{Duration, SystemTime};

use anthology_core::Env;
use anthology_core::platform::Target;
use anthology_pycompat::json::{Int, Map, Value};
use anthology_pycompat::pyerr::PyExc;
use anthology_runtime::fetch::Fetch;
use anthology_runtime::rtk_install::{self, RtkDeps, RtkLimits};
use anthology_testkit::release::sha256_hex;
use anthology_testkit::rt_b::{RawKind, raw_tar_gz};

use crate::case::{Case, Rng, Surface};
use crate::normalize::Normalizer;
use crate::observe::{Observed, from_oracle};
use crate::oracle::Oracle;

/// The surface name (plan §1.4).
pub const NAME: &str = "rtk_install.reconcile";

/// Whether `difftest --all` runs this surface on macOS too (`ci/jobs/difftest-macos.sh`): it
/// does; nothing here is Linux-only.
pub const ON_MACOS: bool = true;

/// The registered implementation.
pub fn surface() -> Option<Box<dyn Surface>> {
    Some(Box::new(Reconcile))
}

struct Reconcile;

/// The D-2 prefix whose rest of line is library text (module docs).
const D2_TARBALL: &str = "cannot read the rtk tarball: ";

fn normalizer() -> Normalizer {
    Normalizer::new().d2_prefix(D2_TARBALL)
}

/// `mkdtemp`'s random `.tmp-XXXXXXXX` names differ between any two runs: `.tmp-<RANDOM>`.
/// Not a divergence; a left-over staging folder still shows as an entry.
fn temp_names(b: &[u8]) -> Vec<u8> {
    const PREFIX: &[u8] = b".tmp-";
    let name = |c: u8| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_';
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i..].starts_with(PREFIX) {
            let start = i + PREFIX.len();
            let len = b[start..].iter().take_while(|&&c| name(c)).count();
            if len == 8 {
                out.extend_from_slice(b".tmp-<RANDOM>");
                i = start + len;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// The D-2 rule on one side (after the temp-name rule); true when it changed it.
fn apply_d2(o: &mut Observed) -> bool {
    o.stdout = temp_names(&o.stdout);
    o.stderr = temp_names(&o.stderr);
    let before = o.clone();
    normalizer().apply(o);
    *o != before
}

impl Surface for Reconcile {
    fn name(&self) -> &'static str {
        NAME
    }

    fn seeds(&self) -> Vec<Case> {
        seed_cases()
            .into_iter()
            .enumerate()
            .map(|(i, v)| Case::new(format!("seed:{}", i + 1), v))
            .collect()
    }

    fn generate(&self, rng: &mut Rng, n: usize) -> Vec<Case> {
        (0..n)
            .map(|i| Case::new(format!("gen:{}", i + 1), gen_case(rng)))
            .collect()
    }

    fn run_python(&self, c: &Case, oracle: &mut Oracle) -> Observed {
        let mut o = from_oracle(&oracle.call("rtk_install", &c.data));
        if apply_d2(&mut o) {
            oracle.mark("D-2");
        }
        o
    }

    fn run_rust(&self, c: &Case) -> Observed {
        let mut o = run(&c.data);
        apply_d2(&mut o);
        o
    }
}

// --- running a case --------------------------------------------------------------------------

fn field<'a>(v: &'a Value, k: &str) -> &'a Value {
    static NULL: Value = Value::Null;
    v.as_object().and_then(|o| o.get(k)).unwrap_or(&NULL)
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
        .collect()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn int(v: &Value) -> Option<i64> {
    match v {
        Value::Int(Int::Small(n)) => Some(*n),
        _ => None,
    }
}

/// The case's fake fetch.
struct CaseFetch {
    serve: String,
    blob: Vec<u8>,
    url: String,
    error: Option<(String, String)>,
    calls: std::cell::RefCell<Vec<(String, f64)>>,
}

impl Fetch for CaseFetch {
    fn fetch(&self, url: &str, dest: &mut dyn Write, timeout: f64) -> Result<(), PyExc> {
        self.calls.borrow_mut().push((url.to_string(), timeout));
        match self.serve.as_str() {
            "error" => {
                let (t, m) = self.error.clone().unwrap_or_default();
                Err(PyExc::Other(leak_type(&t), m))
            }
            "lock" if url == self.url => {
                dest.write_all(&self.blob)?;
                Ok(())
            }
            _ => Err(PyExc::Other("KeyError", format!("'{url}'"))),
        }
    }
}

/// The exception type names a case may raise.
fn leak_type(t: &str) -> &'static str {
    match t {
        "OSError" => "OSError",
        "URLError" => "URLError",
        "TimeoutError" => "TimeoutError",
        "InstallError" => "InstallError",
        _ => "RuntimeError",
    }
}

fn set_age(path: &Path, days: f64) {
    let when = SystemTime::now() - Duration::from_secs_f64(days * 86400.0);
    if let Ok(f) = fs::File::open(path) {
        let _ = f.set_times(fs::FileTimes::new().set_accessed(when).set_modified(when));
    }
}

fn build_pre(tmp: &Path, data: &Path, pre: &[Value]) {
    let mut ages = Vec::new();
    for item in pre {
        let rel = field(item, "path").as_str().expect("path");
        let path = data.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("mkdir -p");
        }
        match field(item, "kind").as_str() {
            Some("file") => {
                fs::write(&path, unhex(field(item, "data").as_str().unwrap_or(""))).expect("file");
            }
            Some("dir") => fs::create_dir_all(&path).expect("dir"),
            Some("symlink") => {
                let target = field(item, "target")
                    .as_str()
                    .unwrap_or("")
                    .replace("<tmp>", &tmp.to_string_lossy());
                std::os::unix::fs::symlink(target, &path).expect("symlink");
                continue;
            }
            other => panic!("rtk_install.reconcile: kind {other:?}"),
        }
        if let Some(m) = int(field(item, "mode")) {
            fs::set_permissions(&path, fs::Permissions::from_mode(m as u32)).expect("chmod");
        }
        if let Value::Float(_) | Value::Int(_) = field(item, "age") {
            let days = match field(item, "age") {
                Value::Float(f) => *f,
                v => int(v).unwrap_or(0) as f64,
            };
            ages.push((path, days));
        }
    }
    // Children first, so setting a folder's age is not undone by its contents.
    for (path, days) in ages.iter().rev() {
        set_age(path, *days);
    }
}

/// `"%g" % timeout` for the timeouts the surface uses.
fn g(t: f64) -> String {
    if t.fract() == 0.0 && t.abs() < 1e15 {
        format!("{}", t as i64)
    } else {
        format!("{t}")
    }
}

/// Every entry under `dir`, sorted, as `<rel> <kind> <mode> <detail>`.
fn listing(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let rel = p
                .strip_prefix(dir)
                .expect("under dir")
                .to_string_lossy()
                .into_owned();
            let Ok(m) = fs::symlink_metadata(&p) else {
                continue;
            };
            let mode = m.mode() & 0o7777;
            if m.file_type().is_symlink() {
                let t = fs::read_link(&p).map(|t| t.to_string_lossy().into_owned());
                out.push(format!("{rel} link -> {}", t.unwrap_or_default()));
            } else if m.is_dir() {
                out.push(format!("{rel} dir {mode:04o}"));
                stack.push(p);
            } else {
                let sha = fs::read(&p).map(|b| sha256_hex(&b)).unwrap_or_default();
                out.push(format!(
                    "{rel} file {mode:04o} {}",
                    &sha[..16.min(sha.len())]
                ));
            }
        }
    }
    out.sort();
    out
}

fn run(case: &Value) -> Observed {
    let dir = tempfile::tempdir().expect("tempdir");
    let tmp = fs::canonicalize(dir.path()).expect("realpath");
    let data = tmp.join("data");
    fs::create_dir_all(&data).expect("data");
    let env = Env::default()
        .with_os("CLAUDE_CONFIG_DIR", tmp.join("config"))
        .with_os("HOME", tmp.join("home"));
    build_pre(
        &tmp,
        &data,
        field(case, "pre").as_array().map_or(&[], Vec::as_slice),
    );
    let shown = |s: &str| s.replace(&*tmp.to_string_lossy(), "<tmp>");
    if let Some(prior) = field(case, "state").as_str() {
        let value = prior.replace("<tmp>", &tmp.to_string_lossy());
        anthology_core::state::write(&env, vec![("rtk_path".into(), Value::Str(value))])
            .expect("prior state");
    }
    let lock = match rtk_install::validate_lock(field(case, "lock")) {
        Ok(l) => l,
        Err(e) => {
            return Observed::text(2, "", &format!("rtk_install: {}\n", e.py_str()));
        }
    };
    let uname = field(case, "uname").as_array().expect("uname");
    let (system, machine) = (
        uname[0].as_str().unwrap_or("").to_string(),
        uname[1].as_str().unwrap_or("").to_string(),
    );
    let url = rtk_install::target(&system, &machine)
        .map(|t| rtk_install::release_url(&lock, t))
        .unwrap_or_default();
    let error = field(case, "error").as_array().map(|e| {
        (
            e[0].as_str().unwrap_or("").to_string(),
            e[1].as_str().unwrap_or("").to_string(),
        )
    });
    let fetch = CaseFetch {
        serve: field(case, "serve").as_str().unwrap_or("lock").to_string(),
        blob: unhex(field(case, "blob").as_str().unwrap_or("")),
        url,
        error,
        calls: Default::default(),
    };
    let mut limits = RtkLimits::default();
    if let Some(n) = int(field(case, "max_binary_bytes")) {
        limits.max_binary_bytes = n as u64;
    }
    if let Some(n) = int(field(case, "max_tar_members")) {
        limits.max_tar_members = n as usize;
    }
    UNAME.with(|u| *u.borrow_mut() = (system, machine));
    let mut deps = RtkDeps::new(&fetch);
    deps.uname = case_uname;
    deps.limits = limits;
    let res = rtk_install::reconcile(&env, &data, &lock, 5.0, &deps);

    let mut out = String::new();
    let none = || "None".to_string();
    out.push_str(&format!(
        "rtk_path: {}\ninstalled: {}\nwarning: {}\ncalls: {}\n",
        res.rtk_path
            .as_ref()
            .map_or_else(none, |p| shown(&p.to_string_lossy())),
        if res.installed { "True" } else { "False" },
        res.warning.as_deref().map_or_else(none, shown),
        fetch.calls.borrow().len()
    ));
    for (u, t) in fetch.calls.borrow().iter() {
        out.push_str(&format!("call: {u} {}\n", g(*t)));
    }
    out.push_str("tree:\n");
    for line in listing(&data) {
        out.push_str(&shown(&line));
        out.push('\n');
    }
    let st = anthology_core::state::read(&env);
    out.push_str(&format!(
        "state: {}\n",
        st.get("rtk_path")
            .and_then(Value::as_str)
            .map_or_else(none, shown)
    ));
    Observed::text(0, &out, "")
}

thread_local! {
    static UNAME: std::cell::RefCell<(String, String)> = Default::default();
}

fn case_uname() -> (String, String) {
    UNAME.with(|u| u.borrow().clone())
}

// --- cases ---------------------------------------------------------------------------------

const TRIPLES: [&str; 4] = [
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
];

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    let mut m = Map::new();
    for (k, v) in pairs {
        m.insert(k.to_string(), v);
    }
    Value::Object(m)
}

fn s(v: &str) -> Value {
    Value::Str(v.to_string())
}

fn n(v: i64) -> Value {
    Value::Int(Int::Small(v))
}

/// A lock whose `host` target hashes `blob` (or `wrong`), the others their own names.
fn lock_value(version: &str, repo: &str, host: Option<&str>, blob: &[u8], wrong: bool) -> Value {
    let mut digests = Map::new();
    for t in TRIPLES {
        let d = if Some(t) == host {
            if wrong {
                "0".repeat(64)
            } else {
                sha256_hex(blob)
            }
        } else {
            sha256_hex(t.as_bytes())
        };
        digests.insert(t.to_string(), Value::Str(d));
    }
    obj(vec![
        ("version", s(version)),
        ("tag", s(&format!("rtk--v{version}"))),
        ("repo", s(repo)),
        ("sha256", Value::Object(digests)),
    ])
}

fn host_of(uname: (&str, &str)) -> Option<&'static str> {
    rtk_install::target(uname.0, uname.1)
        .ok()
        .map(Target::triple)
}

fn good_blob() -> Vec<u8> {
    raw_tar_gz(&[
        ("rtk", RawKind::File(b"#!/bin/sh\necho fake-rtk\n".to_vec())),
        ("LICENSE", RawKind::File(b"license text\n".to_vec())),
    ])
}

#[allow(clippy::too_many_arguments)]
fn case(
    lock: Value,
    uname: (&str, &str),
    blob: &[u8],
    serve: &str,
    error: Option<(&str, &str)>,
    pre: Vec<Value>,
    state: Option<&str>,
    limits: (Option<i64>, Option<i64>),
) -> Value {
    obj(vec![
        ("lock", lock),
        ("uname", Value::Array(vec![s(uname.0), s(uname.1)])),
        ("blob", s(&hex(blob))),
        ("serve", s(serve)),
        (
            "error",
            error.map_or(Value::Null, |(t, m)| Value::Array(vec![s(t), s(m)])),
        ),
        ("pre", Value::Array(pre)),
        ("state", state.map_or(Value::Null, s)),
        ("max_binary_bytes", limits.0.map_or(Value::Null, n)),
        ("max_tar_members", limits.1.map_or(Value::Null, n)),
    ])
}

fn pre_file(path: &str, data: &[u8], mode: i64, age: Option<f64>) -> Value {
    obj(vec![
        ("path", s(path)),
        ("kind", s("file")),
        ("data", s(&hex(data))),
        ("mode", n(mode)),
        ("age", age.map_or(Value::Null, Value::Float)),
    ])
}

fn pre_dir(path: &str, age: Option<f64>) -> Value {
    obj(vec![
        ("path", s(path)),
        ("kind", s("dir")),
        ("mode", n(0o755)),
        ("age", age.map_or(Value::Null, Value::Float)),
    ])
}

fn pre_link(path: &str, target: &str) -> Value {
    obj(vec![
        ("path", s(path)),
        ("kind", s("symlink")),
        ("target", s(target)),
    ])
}

const LINUX: (&str, &str) = ("Linux", "x86_64");
const V: &str = "1.2.1-anthology.1";
const REPO: &str = "rbonweb/anthology";

/// A current install of `blob` at version `v`.
fn installed(v: &str, blob: &[u8]) -> Vec<Value> {
    vec![
        pre_file(
            &format!("rtk/{v}/rtk"),
            b"#!/bin/sh\necho fake-rtk\n",
            0o755,
            None,
        ),
        pre_file(
            &format!("rtk/{v}/.sha256"),
            format!("{}\n", sha256_hex(blob)).as_bytes(),
            0o644,
            None,
        ),
    ]
}

fn seed_cases() -> Vec<Value> {
    let blob = good_blob();
    let host = host_of(LINUX);
    let lock = || lock_value(V, REPO, host, &blob, false);
    let none = (None, None);
    let mut out = vec![
        // A fresh install, then the same with the state already right.
        case(lock(), LINUX, &blob, "lock", None, vec![], None, none),
        case(
            lock(),
            LINUX,
            &blob,
            "lock",
            None,
            installed(V, &blob),
            Some(&format!("<tmp>/data/rtk/{V}/rtk")),
            none,
        ),
        // Already installed, stale state.
        case(
            lock(),
            LINUX,
            &blob,
            "lock",
            None,
            installed(V, &blob),
            Some("/previous/rtk"),
            none,
        ),
        // A wrong hash.
        case(
            lock_value(V, REPO, host, &blob, true),
            LINUX,
            &blob,
            "lock",
            None,
            vec![],
            Some("/previous/rtk"),
            none,
        ),
        // Fetch errors.
        case(
            lock(),
            LINUX,
            &blob,
            "error",
            Some(("OSError", "offline")),
            vec![],
            None,
            none,
        ),
        case(
            lock(),
            LINUX,
            &blob,
            "error",
            Some(("URLError", "<urlopen error dns>")),
            vec![],
            None,
            none,
        ),
        case(lock(), LINUX, &blob, "elsewhere", None, vec![], None, none),
        // Platforms.
        case(
            lock(),
            ("Windows", "AMD64"),
            &blob,
            "lock",
            None,
            vec![],
            None,
            none,
        ),
        case(
            lock(),
            ("FreeBSD", "amd64"),
            &blob,
            "lock",
            None,
            vec![],
            None,
            none,
        ),
        case(
            lock_value(V, REPO, host_of(("Darwin", "arm64")), &blob, false),
            ("Darwin", "arm64"),
            &blob,
            "lock",
            None,
            vec![],
            None,
            none,
        ),
        // An invalid lock.
        case(obj(vec![]), LINUX, &blob, "lock", None, vec![], None, none),
        // A symlinked version folder.
        case(
            lock(),
            LINUX,
            &blob,
            "lock",
            None,
            vec![
                pre_dir("elsewhere-target", None),
                pre_link(&format!("rtk/{V}"), "<tmp>/data/elsewhere-target"),
            ],
            None,
            none,
        ),
        // Prune: old, recent, markerless, stray, temp folders.
        case(
            lock(),
            LINUX,
            &blob,
            "lock",
            None,
            vec![
                pre_file("rtk/0.9.0/rtk", b"x", 0o755, None),
                pre_file("rtk/0.9.0/.last-used", b"", 0o644, Some(45.0)),
                pre_file("rtk/1.0.0/rtk", b"x", 0o755, None),
                pre_file("rtk/1.0.0/.last-used", b"", 0o644, Some(5.0)),
                pre_file("rtk/0.8.0/rtk", b"x", 0o755, Some(90.0)),
                pre_dir("rtk/0.8.0", Some(90.0)),
                pre_dir("rtk/notes", Some(400.0)),
                pre_dir("rtk/.tmp-stale", Some(2.0)),
                pre_dir("rtk/.tmp-fresh", Some(0.01)),
            ],
            None,
            none,
        ),
        // OSError paths: `.last-used` is a folder (touch), `rtk` is a non-empty folder (the
        // rename), the version folder is a file (mkdir).
        case(
            lock(),
            LINUX,
            &blob,
            "lock",
            None,
            {
                let mut p = installed(V, &blob);
                p.push(pre_dir(&format!("rtk/{V}/.last-used"), None));
                p
            },
            None,
            none,
        ),
        case(
            lock(),
            LINUX,
            &blob,
            "lock",
            None,
            vec![pre_file(&format!("rtk/{V}/rtk/keep"), b"x", 0o644, None)],
            None,
            none,
        ),
        case(
            lock(),
            LINUX,
            &blob,
            "lock",
            None,
            vec![pre_file(&format!("rtk/{V}"), b"a file", 0o644, None)],
            None,
            none,
        ),
        // Limits.
        case(
            lock(),
            LINUX,
            &blob,
            "lock",
            None,
            vec![],
            None,
            (Some(4), None),
        ),
        case(
            lock(),
            LINUX,
            &blob,
            "lock",
            None,
            vec![],
            None,
            (None, Some(1)),
        ),
    ];
    for (name, members) in rejected_members() {
        let _ = name;
        let b = raw_tar_gz(&members);
        out.push(case(
            lock_value(V, REPO, host, &b, false),
            LINUX,
            &b,
            "lock",
            None,
            vec![],
            None,
            none,
        ));
    }
    for b in [
        b"".to_vec(),
        b"this is not a tarball".to_vec(),
        gzip(&[0u8; 100]),
        gzip(&[0u8; 1024]),
    ] {
        out.push(case(
            lock_value(V, REPO, host, &b, false),
            LINUX,
            &b,
            "lock",
            None,
            vec![],
            None,
            none,
        ));
    }
    out
}

fn rejected_members() -> Vec<(&'static str, Vec<(&'static str, RawKind)>)> {
    let body = || RawKind::File(b"#!/bin/sh\necho fake-rtk\n".to_vec());
    let x = || RawKind::File(b"x".to_vec());
    vec![
        (
            "symlink rtk",
            vec![("rtk", RawKind::Symlink("/bin/sh".into()))],
        ),
        (
            "hardlink rtk",
            vec![
                ("rtk", RawKind::Hardlink("LICENSE".into())),
                ("LICENSE", x()),
            ],
        ),
        ("dotdot member", vec![("../rtk", body()), ("LICENSE", x())]),
        ("nested member", vec![("bin/rtk", body())]),
        ("absolute member", vec![("/rtk", body())]),
        ("no rtk member", vec![("LICENSE", x())]),
        (
            "duplicate rtk",
            vec![("rtk", body()), ("rtk", RawKind::File(b"other".to_vec()))],
        ),
        ("dir rtk", vec![("rtk", RawKind::Dir)]),
        ("dot rtk", vec![("./rtk", body())]),
        // tarfile reads an old-style `rtk/` as the folder `rtk`.
        (
            "old-style folder",
            vec![("rtk/", RawKind::OldFile(Vec::new())), ("rtk", body())],
        ),
        (
            "old-style regular rtk",
            vec![("rtk", RawKind::OldFile(b"old".to_vec()))],
        ),
    ]
}

fn gzip(b: &[u8]) -> Vec<u8> {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(b).expect("gzip");
    gz.finish().expect("gzip")
}

const MEMBER_NAMES: [&str; 10] = [
    "rtk", "rtk", "rtk", "LICENSE", "./rtk", "bin/rtk", "../rtk", "/rtk", "README", "rtk/",
];

fn rand_bytes(rng: &mut Rng, max: usize) -> Vec<u8> {
    (0..rng.index(max + 1))
        .map(|_| rng.below(256) as u8)
        .collect()
}

fn rand_blob(rng: &mut Rng) -> Vec<u8> {
    let k = rng.f64();
    if k < 0.04 {
        return Vec::new();
    }
    if k < 0.08 {
        return rand_bytes(rng, 64);
    }
    if k < 0.12 {
        let junk = rand_bytes(rng, 700);
        return gzip(&junk);
    }
    if k < 0.5 {
        // A well-formed release: rtk (any bytes) and LICENSE.
        return raw_tar_gz(&[
            ("rtk", RawKind::File(rand_bytes(rng, 64))),
            ("LICENSE", RawKind::File(b"license text\n".to_vec())),
        ]);
    }
    let mut members: Vec<(String, RawKind)> = Vec::new();
    let count = if rng.chance(0.05) {
        60 + rng.index(10)
    } else {
        1 + rng.index(4)
    };
    for i in 0..count {
        let name = if count > 10 && i > 0 {
            format!("f{i}")
        } else {
            rng.pick(&MEMBER_NAMES).to_string()
        };
        let kind = match rng.below(10) {
            0 => RawKind::Symlink("/bin/sh".into()),
            1 => RawKind::Hardlink("LICENSE".into()),
            2 => RawKind::Dir,
            3 => RawKind::OldFile(rand_bytes(rng, 48)),
            _ => RawKind::File(rand_bytes(rng, 48)),
        };
        members.push((name, kind));
    }
    let borrowed: Vec<(&str, RawKind)> = members
        .iter()
        .map(|(n, k)| (n.as_str(), k.clone()))
        .collect();
    let mut blob = raw_tar_gz(&borrowed);
    if rng.chance(0.05) {
        // A valid archive, then a header block tarfile cannot read: it ends the archive.
        let mut gz = flate2::read::MultiGzDecoder::new(&blob[..]);
        let mut tar = Vec::new();
        std::io::Read::read_to_end(&mut gz, &mut tar).expect("gunzip");
        // Drop the end-of-archive blocks, add a garbage block.
        while tar.len() >= 512 && tar[tar.len() - 512..].iter().all(|&b| b == 0) {
            tar.truncate(tar.len() - 512);
        }
        tar.extend((0..512).map(|i| (i * 7 + 3) as u8));
        blob = gzip(&tar);
    }
    blob
}

const UNAMES: [(&str, &str); 9] = [
    ("Linux", "x86_64"),
    ("Linux", "x86_64"),
    ("Linux", "x86_64"),
    ("Linux", "aarch64"),
    ("Darwin", "arm64"),
    ("Darwin", "x86_64"),
    ("Windows", "AMD64"),
    ("FreeBSD", "amd64"),
    (" linux ", "AMD64"),
];

const VERSIONS: [&str; 4] = ["1.2.1-anthology.1", "1.2.1-anthology.3", "2.0.0", "0.9.0"];

fn rand_pre(rng: &mut Rng, version: &str, blob: &[u8]) -> Vec<Value> {
    let mut pre = Vec::new();
    match rng.below(10) {
        0 | 1 => pre.extend(installed(version, blob)),
        2 => {
            pre.extend(installed(version, blob));
            pre[1] = pre_file(&format!("rtk/{version}/.sha256"), b"ffff\n", 0o644, None);
        }
        3 => {
            pre.extend(installed(version, blob));
            pre[0] = pre_file(&format!("rtk/{version}/rtk"), b"x", 0o644, None);
        }
        4 => pre.push(pre_file(&format!("rtk/{version}"), b"a file", 0o644, None)),
        5 => {
            pre.push(pre_dir("target-dir", None));
            pre.push(pre_link(&format!("rtk/{version}"), "<tmp>/data/target-dir"));
        }
        6 => {
            pre.extend(installed(version, blob));
            pre.push(pre_dir(&format!("rtk/{version}/.last-used"), None));
        }
        _ => {}
    }
    for _ in 0..rng.index(4) {
        let other = rng.pick(&[
            "0.8.0",
            "1.0.0",
            "3.1.4-x.1",
            "notes",
            ".tmp-old1",
            ".tmp-new1",
        ]);
        if other == &version {
            continue;
        }
        let age = *rng.pick(&[0.01, 1.0, 10.0, 45.0, 90.0]);
        let base = format!("rtk/{other}");
        if pre.iter().any(|p| field(p, "path").as_str() == Some(&base)) {
            continue;
        }
        if rng.chance(0.5) {
            pre.push(pre_file(&format!("{base}/rtk"), b"x", 0o755, None));
            pre.push(pre_file(
                &format!("{base}/.last-used"),
                b"",
                0o644,
                Some(age),
            ));
        }
        pre.push(pre_dir(&base, Some(age)));
    }
    pre
}

/// One random case.
pub fn gen_case(rng: &mut Rng) -> Value {
    let uname = *rng.pick(&UNAMES);
    let version = *rng.pick(&VERSIONS);
    let blob = rand_blob(rng);
    let host = host_of(uname);
    let lock = if rng.chance(0.04) {
        let mut l = lock_value(version, REPO, host, &blob, false);
        if let Some(m) = l.as_object_mut() {
            m.insert("tag".into(), s("v0"));
        }
        l
    } else {
        let repo = *rng.pick(&[REPO, "o/r", "a.b/c-d_e"]);
        lock_value(version, repo, host, &blob, rng.chance(0.08))
    };
    let (serve, error) = match rng.below(20) {
        0 => ("error", Some(("OSError", "offline"))),
        1 => ("error", Some(("URLError", "<urlopen error timed out>"))),
        2 => ("error", Some(("RuntimeError", "weird"))),
        3 => ("elsewhere", None),
        _ => ("lock", None),
    };
    let pre = rand_pre(rng, version, &blob);
    let state = match rng.below(4) {
        0 => Some("/previous/rtk".to_string()),
        1 => Some(format!("<tmp>/data/rtk/{version}/rtk")),
        _ => None,
    };
    let limits = match rng.below(12) {
        0 => (Some(rng.range(0, 40)), None),
        1 => (None, Some(rng.range(1, 4))),
        _ => (None, None),
    };
    case(
        lock,
        uname,
        &blob,
        serve,
        error,
        pre,
        state.as_deref(),
        limits,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_run_and_cover_the_outcomes() {
        let seeds = Reconcile.seeds();
        let outs: Vec<String> = seeds
            .iter()
            .map(|c| String::from_utf8_lossy(&run(&c.data).stdout).into_owned())
            .collect();
        assert!(outs[0].contains("installed: True"), "{}", outs[0]);
        assert!(outs[1].contains("calls: 0"), "{}", outs[1]);
        assert!(outs.iter().any(|o| o.contains("does not match rtk.lock")));
        assert!(outs.iter().any(|o| o.contains("tarball has no rtk member")));
        assert!(outs.iter().any(|o| o.contains("is a symlink")));
    }

    #[test]
    fn generated_cases_run() {
        let mut rng = Rng::new(7);
        let mut ok = 0;
        for c in Reconcile.generate(&mut rng, 200) {
            let o = run(&c.data);
            assert!(o.exit == 0 || o.exit == 2, "{o:?}");
            if String::from_utf8_lossy(&o.stdout).contains("installed: True") {
                ok += 1;
            }
        }
        assert!(ok > 20, "{ok} successful installs");
    }
}
