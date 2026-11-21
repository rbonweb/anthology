//! The `runtime.guardrails_loc` difftest surface (RT1a): `guardrails_loc` and `compat` against
//! `plugins/anthology/scripts/{guardrails_loc,compat}.py` on generated layouts and settings.
//!
//! Oracle: `port/difftest/oracle/guardrails_loc.py`. Its module docs give the case format; the
//! layout is built by `anthology_testkit::rt_a::materialize` here and by its twin there, each
//! under its own temporary root, and every path in a result is written relative to `@ROOT@`.
//!
//! Seeds are the layouts of `tests/test_guardrails_loc.py` and the compat rows of
//! `tests/test_doctor.py`. Generated cases draw every piece from a vocabulary aimed at the
//! fail-closed rules:
//! - registry entries: near-miss keys, scopes, `projectPath` inside, outside and through links,
//!   every JSON type of `lastUpdated`, literal repeated JSON keys (last value, first position);
//! - marketplace sources: every type and location form;
//! - `marketplace.json` sources: escaping, absolute, empty, non-str, NUL, unlisted, invalid,
//!   oversized, and symlink chains (short ones resolve; 1,100 links fail in both: Python's
//!   recursion, Rust's `REALPATH_MAX_DEPTH`);
//! - settings: every `enabledPlugins` value type in all three files; every encoding; invalid
//!   UTF-8 and UTF-16 bytes and odd lengths; empty and Python-whitespace files (`\x1c`,
//!   U+3000); unparseable texts; non-regular files (folder, FIFO, `/dev/null`, `/dev/zero`,
//!   dangling link, a link to itself); repeated keys; sizes at the 2 MiB limit, one byte over
//!   and well under (about 1% of cases); deep nesting in every file;
//! - the wrappers, agent-memory, subagent-model, `repo_top`, bounded and unbounded
//!   `read_json`, and `decode_text`.
//!
//! The generator never builds a layout the two sides cannot agree on by design:
//! - JSON nesting is at most 4 levels, or 20,000 to 30,000 (where both refuse: Python by
//!   `RecursionError`, Rust by `REPO_JSON_DEPTH` or `PY_JSON_DEPTH`). The seeds pin the user
//!   files' exact edge (9,997 nested containers read, 9,998 not, as in CPython 3.12). In a
//!   repo-controlled file, 513 to 9,997 levels differ on purpose (Rust refuses from 513: the
//!   repo settings read as disabled, a project file is skipped; the safe direction).
//! - Symlink chains are at most 8 links or exactly 1,100. From 41 to about 990 Rust refuses
//!   (`ELOOP`, as the kernel does past 40) where Python still resolves: the safe direction.
//! - No lone-surrogate escapes (D-6). This hides one known fail-open direction of D-6: a path
//!   string (`installPath`, `installLocation`, a manifest `source`) holding a lone-surrogate
//!   escape is unencodable in Python (no such path, so `live_missing`), while Rust looks up the
//!   U+FFFD path. Only the marketplace's author can write it, and that author controls the
//!   guard anyway.
//! - The non-ASCII characters inside a container `lastUpdated` are ones whose
//!   `str.isprintable` Rust knows without P1's tables.
//! - No unbounded read of a FIFO or a device (Python blocks or never ends there too).

use std::path::Path;

use anthology_core::Env;
use anthology_pycompat::json::{self, Int, Map, Value};
use anthology_runtime::{compat, guardrails_loc};
use anthology_testkit::rt_a::{self, RealTemp};

use crate::case::{Case, Rng, Surface};
use crate::observe::{Observed, from_oracle};
use crate::oracle::Oracle;

/// The surface name.
pub const NAME: &str = "runtime.guardrails_loc";

/// Whether `difftest --all` runs this surface on macOS too (`ci/jobs/difftest-macos.sh`): yes,
/// the layouts (FIFOs, `/dev/zero`, unreadable files, `/private` temp roots) exist there too.
pub const ON_MACOS: bool = true;

/// The registered implementation.
pub fn surface() -> Option<Box<dyn Surface>> {
    Some(Box::new(GuardrailsLoc))
}

struct GuardrailsLoc;

impl Surface for GuardrailsLoc {
    fn name(&self) -> &'static str {
        NAME
    }

    fn seeds(&self) -> Vec<Case> {
        seeds()
            .into_iter()
            .enumerate()
            .map(|(i, data)| Case::new(format!("seed:{}", i + 1), data))
            .collect()
    }

    fn generate(&self, rng: &mut Rng, n: usize) -> Vec<Case> {
        (0..n)
            .map(|i| Case::new(format!("gen:{}", i + 1), gen_case(rng)))
            .collect()
    }

    fn run_python(&self, c: &Case, oracle: &mut Oracle) -> Observed {
        from_oracle(&oracle.call("guardrails_loc", &c.data))
    }

    fn run_rust(&self, c: &Case) -> Observed {
        run(&c.data)
    }
}

// ---------------------------------------------------------------------------------------------
// The Rust side

fn field<'a>(case: &'a Value, k: &str) -> Option<&'a Value> {
    case.as_object().and_then(|o| o.get(k))
}

fn arg(case: &Value, k: &str, root: &Path) -> String {
    let s = field(case, k)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("runtime.guardrails_loc: no {k:?}"));
    rt_a::expand(s, root, None)
}

fn norm(p: Option<&Path>, root: &Path) -> Value {
    match p {
        None => Value::Null,
        Some(p) => Value::Str(
            p.to_string_lossy()
                .replace(&*root.to_string_lossy(), "@ROOT@"),
        ),
    }
}

fn strs(v: &[String]) -> Value {
    Value::Array(v.iter().map(|s| Value::Str(s.clone())).collect())
}

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    let mut m = Map::new();
    for (k, v) in pairs {
        m.insert(k.to_string(), v);
    }
    Value::Object(m)
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).expect("hex"))
        .collect()
}

fn run(case: &Value) -> Observed {
    let tmp = RealTemp::new();
    let root = tmp.path.clone();
    let tree = field(case, "tree")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    rt_a::materialize(&root, &tree);
    let mut env = Env::default();
    if let Some(vars) = field(case, "env").and_then(Value::as_object) {
        for (k, v) in vars.iter() {
            env = env.with(
                k,
                &rt_a::expand(v.as_str().expect("env value"), &root, None),
            );
        }
    }
    let op = field(case, "op").and_then(Value::as_str).expect("op");
    let result = match op {
        "inspect" => {
            let loc = guardrails_loc::inspect(
                Path::new(&arg(case, "plugin_root", &root)),
                &env,
                Path::new(&arg(case, "cwd", &root)),
            );
            obj(vec![
                ("root", norm(loc.root.as_deref(), &root)),
                (
                    "problem",
                    loc.problem
                        .map_or(Value::Null, |p| Value::Str(p.as_str().into())),
                ),
            ])
        }
        "disabled" => obj(vec![(
            "disabled",
            Value::Bool(guardrails_loc::disabled(
                &env,
                Path::new(&arg(case, "cwd", &root)),
            )),
        )]),
        "wrappers" => obj(vec![(
            "bad",
            strs(&compat::wrapper_problems(Path::new(&arg(
                case,
                "plugin_root",
                &root,
            )))),
        )]),
        "agent_memory" => {
            let (bad, good) = compat::agent_memory_problems(Path::new(&arg(case, "cwd", &root)));
            obj(vec![("bad", strs(&bad)), ("good", strs(&good))])
        }
        "subagent_model" => obj(vec![(
            "source",
            compat::subagent_model_source(&env, Path::new(&arg(case, "cwd", &root)))
                .map_or(Value::Null, |s| Value::Str(s.into())),
        )]),
        "repo_top" => obj(vec![(
            "top",
            norm(
                compat::repo_top(Path::new(&arg(case, "cwd", &root))).as_deref(),
                &root,
            ),
        )]),
        "read_json" => {
            let path = arg(case, "path", &root);
            let value = match field(case, "limit").and_then(Value::as_i64) {
                Some(limit) => compat::read_json_limited(Path::new(&path), limit as u64),
                None => compat::read_json(Path::new(&path)),
            };
            if field(case, "found_only").and_then(Value::as_bool) == Some(true) {
                obj(vec![("found", Value::Bool(value.is_some()))])
            } else {
                obj(vec![("value", value.unwrap_or(Value::Null))])
            }
        }
        "decode" => {
            let raw = unhex(field(case, "hex").and_then(Value::as_str).expect("hex"));
            obj(vec![("text", Value::Str(compat::decode_text(&raw)))])
        }
        other => panic!("runtime.guardrails_loc: unknown op {other:?}"),
    };
    Observed {
        exit: 0,
        stdout: json::dumps(&result, &json::PY_DEFAULT_SORTED).into_bytes(),
        stderr: Vec::new(),
        files: None,
    }
}

// ---------------------------------------------------------------------------------------------
// Building cases

const CONFIG: &str = "hm/.claude";
const CACHE: &str = "hm/.claude/plugins/cache/anthology/anthology-guardrails/0.1.0";
const CACHED_ANTHOLOGY: &str = "hm/.claude/plugins/cache/anthology/anthology/0.1.0";
const MARKET: &str = "market";
const LIVE: &str = "market/plugins/anthology-guardrails";
const MANIFEST: &str = "market/.claude-plugin/marketplace.json";
const INSTALLED: &str = "hm/.claude/plugins/installed_plugins.json";
const KNOWN: &str = "hm/.claude/plugins/known_marketplaces.json";
const USER_SETTINGS: &str = "hm/.claude/settings.json";
const LIMIT: i64 = 2 * 1024 * 1024;
const FLAG_FALSE: &str = r#"{"enabledPlugins": {"anthology-guardrails@anthology": false}}"#;

fn s(x: &str) -> Value {
    Value::Str(x.to_string())
}

fn int(i: i64) -> Value {
    Value::Int(Int::Small(i))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn text(v: &Value) -> String {
    json::dumps(v, &json::PY_DEFAULT)
}

/// A change to a case under construction (a seed's settings variant).
type Edit = Box<dyn Fn(&mut B)>;

/// A case under construction.
#[derive(Default)]
struct B {
    tree: Vec<Value>,
    env: Vec<(String, String)>,
}

impl B {
    fn new() -> B {
        B {
            tree: Vec::new(),
            env: vec![("CLAUDE_CONFIG_DIR".into(), format!("@ROOT@/{CONFIG}"))],
        }
    }
    fn push(&mut self, pairs: Vec<(&str, Value)>) -> &mut B {
        self.tree.push(obj(pairs));
        self
    }
    fn dir(&mut self, p: &str) -> &mut B {
        self.push(vec![("t", s("dir")), ("p", s(p))])
    }
    fn file(&mut self, p: &str, t: &str) -> &mut B {
        self.push(vec![("t", s("file")), ("p", s(p)), ("text", s(t))])
    }
    fn file_enc(&mut self, p: &str, t: &str, enc: &str) -> &mut B {
        self.push(vec![
            ("t", s("file")),
            ("p", s(p)),
            ("text", s(t)),
            ("enc", s(enc)),
        ])
    }
    fn file_hex(&mut self, p: &str, b: &[u8]) -> &mut B {
        self.push(vec![("t", s("file")), ("p", s(p)), ("hex", s(&hex(b)))])
    }
    fn file_pad(&mut self, p: &str, t: &str, key: &str, n: i64) -> &mut B {
        self.push(vec![
            ("t", s("file")),
            ("p", s(p)),
            ("text", s(t)),
            (key, int(n)),
        ])
    }
    fn guard(&mut self, p: &str, kind: &str) -> &mut B {
        self.push(vec![("t", s("guard")), ("p", s(p)), ("kind", s(kind))])
    }
    fn link(&mut self, p: &str, to: &str) -> &mut B {
        self.push(vec![("t", s("link")), ("p", s(p)), ("to", s(to))])
    }
    fn fifo(&mut self, p: &str) -> &mut B {
        self.push(vec![("t", s("fifo")), ("p", s(p))])
    }
    fn chmod(&mut self, p: &str, mode: i64) -> &mut B {
        self.push(vec![("t", s("chmod")), ("p", s(p)), ("mode", int(mode))])
    }
    fn env(&mut self, k: &str, v: &str) -> &mut B {
        self.env.retain(|(x, _)| x != k);
        self.env.push((k.into(), v.into()));
        self
    }
    fn json(&mut self, p: &str, v: &Value) -> &mut B {
        self.file(p, &text(v))
    }
    /// The finished case: `op` plus its arguments.
    fn case(&self, op: &str, args: Vec<(&str, Value)>) -> Value {
        let mut env = Map::new();
        for (k, v) in &self.env {
            env.insert(k.clone(), s(v));
        }
        let mut pairs = vec![
            ("op", s(op)),
            ("tree", Value::Array(self.tree.clone())),
            ("env", Value::Object(env)),
        ];
        pairs.extend(args);
        obj(pairs)
    }
    fn inspect(&self, plugin_root: &str) -> Value {
        self.case(
            "inspect",
            vec![("plugin_root", s(plugin_root)), ("cwd", s("@ROOT@"))],
        )
    }

    // --- the LiveRootCase helpers
    fn installed_entry(&mut self, install: &str) -> &mut B {
        let v = json::loads(&format!(
            r#"{{"version": 2, "plugins": {{"anthology-guardrails@anthology": [{{"scope": "user", "installPath": "{install}", "version": "0.1.0", "lastUpdated": "2026-01-01T00:00:00Z"}}]}}}}"#
        ))
        .unwrap();
        self.json(INSTALLED, &v)
    }
    fn installed(&mut self) -> &mut B {
        self.installed_entry(&format!("@ROOT@/{CACHE}"))
    }
    fn known(&mut self, source: &str, location: &str) -> &mut B {
        self.file(
            KNOWN,
            &format!(
                r#"{{"anthology": {{"source": {source}, "installLocation": "{location}", "lastUpdated": "x"}}}}"#
            ),
        )
    }
    fn directory_known(&mut self) -> &mut B {
        self.known(
            r#"{"source": "directory", "path": "@ROOT@/market"}"#,
            "@ROOT@/market",
        )
    }
    fn manifest(&mut self, source: &str) -> &mut B {
        self.file(
            MANIFEST,
            &format!(
                r#"{{"name": "anthology", "plugins": [{{"name": "anthology-guardrails", "source": {source}}}]}}"#
            ),
        )
    }
    fn diverging(&mut self) -> &mut B {
        self.guard(CACHE, "file")
            .guard(LIVE, "file")
            .manifest(r#""./plugins/anthology-guardrails""#)
            .directory_known()
            .installed()
    }
}

/// The Python tests' layouts (`tests/test_guardrails_loc.py`, the compat rows of
/// `tests/test_doctor.py`), each as one case.
fn seeds() -> Vec<Value> {
    let plugin = "@ROOT@/repo/plugins/anthology";
    let lone = "@ROOT@/lone/anthology";
    let mut out = Vec::new();
    // ResolutionTest
    out.push(B::new().diverging().inspect(plugin));
    for source in [
        r#"{"source": "github", "repo": "owner/anthology", "ref": "build"}"#,
        r#"{"source": "git", "url": "https://example.invalid/x.git"}"#,
        r#"{"source": "url", "url": "https://example.invalid/m.json"}"#,
    ] {
        out.push(
            B::new()
                .diverging()
                .known(source, "@ROOT@/market")
                .inspect(plugin),
        );
    }
    out.push(
        B::new()
            .diverging()
            .link("link", "@ROOT@/market")
            .known(
                r#"{"source": "directory", "path": "@ROOT@/link"}"#,
                "@ROOT@/link",
            )
            .inspect(plugin),
    );
    // FallbackTest
    let sibling_case = || {
        let mut b = B::new();
        b.dir("plugins-dir/anthology")
            .guard("plugins-dir/anthology-guardrails", "file")
            .guard(CACHE, "file")
            .installed();
        b
    };
    let pd = "@ROOT@/plugins-dir/anthology";
    out.push(sibling_case().inspect(pd));
    for t in [
        "{not json",
        "[]",
        r#"{"anthology": 3}"#,
        r#"{"anthology": {"source": "directory"}}"#,
        "",
    ] {
        out.push(sibling_case().file(KNOWN, t).inspect(pd));
    }
    for manifest in [
        None,
        Some("{broken"),
        Some(r#"{"plugins": "x"}"#),
        Some(r#"{"plugins": []}"#),
    ] {
        let mut b = sibling_case();
        b.directory_known();
        if let Some(m) = manifest {
            b.file(MANIFEST, m);
        }
        out.push(b.inspect(pd));
    }
    out.push(
        B::new()
            .guard(CACHE, "file")
            .dir(CACHED_ANTHOLOGY)
            .guard(
                "hm/.claude/plugins/cache/anthology/anthology-guardrails",
                "file",
            )
            .installed()
            .inspect(&format!("@ROOT@/{CACHED_ANTHOLOGY}")),
    );
    out.push(
        B::new()
            .guard(CACHE, "file")
            .installed()
            .dir("lone/anthology")
            .inspect(lone),
    );
    out.push(
        B::new()
            .dir("dev/anthology")
            .guard("dev/anthology-guardrails", "file")
            .inspect("@ROOT@/dev/anthology"),
    );
    out.push(B::new().inspect("@ROOT@/nowhere/anthology"));
    out.push(
        B::new()
            .file(
                INSTALLED,
                r#"{"version": 2, "plugins": {"anthology@anthology": [{"scope": "user", "installPath": "@ROOT@/repo/plugins/anthology"}]}}"#,
            )
            .inspect(plugin),
    );
    out.push(
        B::new()
            .guard(CACHE, "file")
            .installed()
            .file(USER_SETTINGS, FLAG_FALSE)
            .inspect(lone),
    );
    // EscapingSourceTest
    for source in [
        r#""../outside""#,
        r#""./plugins/../../outside""#,
        r#""../../etc""#,
        r#""@ROOT@/abs-outside""#,
        r#"{"source": "github", "repo": "a/b"}"#,
        "5",
        r#""""#,
        "null",
    ] {
        out.push(
            B::new()
                .guard("outside", "file")
                .guard("abs-outside", "file")
                .guard(CACHE, "file")
                .directory_known()
                .manifest(source)
                .installed()
                .inspect(lone),
        );
    }
    out.push(
        B::new()
            .guard("sym-outside", "file")
            .dir("market/plugins")
            .link(LIVE, "@ROOT@/sym-outside")
            .guard(CACHE, "file")
            .directory_known()
            .manifest(r#""./plugins/anthology-guardrails""#)
            .installed()
            .inspect(lone),
    );
    for known in [true, false] {
        let mut b = B::new();
        b.guard("sym-outside", "file")
            .dir("market/plugins/anthology")
            .link(LIVE, "@ROOT@/sym-outside")
            .guard(CACHE, "file")
            .manifest(r#""./plugins/anthology-guardrails""#)
            .installed();
        if known {
            b.directory_known();
        }
        out.push(b.inspect("@ROOT@/market/plugins/anthology"));
    }
    out.push(
        B::new()
            .dir("dev/anthology")
            .guard("dev", "file")
            .link("dev/anthology-guardrails", "@ROOT@/dev")
            .inspect("@ROOT@/dev/anthology"),
    );
    out.push(
        B::new()
            .guard("sym-outside", "file")
            .dir("dev/anthology")
            .link("dev/anthology-guardrails", "@ROOT@/sym-outside")
            .inspect("@ROOT@/dev/anthology"),
    );
    // LiveMissingTest
    for (live_hooks, settings) in [(true, false), (false, false), (true, true)] {
        let mut b = B::new();
        b.guard(CACHE, "file");
        if live_hooks {
            b.dir(&format!("{LIVE}/hooks"))
                .dir(&format!("{LIVE}/scripts"));
        }
        b.manifest(r#""./plugins/anthology-guardrails""#)
            .directory_known()
            .installed();
        if settings {
            b.file(USER_SETTINGS, FLAG_FALSE);
        }
        out.push(b.inspect(plugin));
        out.push(b.inspect("@ROOT@/market/plugins/anthology"));
    }
    out.push(
        B::new()
            .guard(CACHE, "file")
            .directory_known()
            .installed()
            .file(
                MANIFEST,
                r#"{"name": "anthology", "plugins": [{"name": "other", "source": "./plugins/other"}]}"#,
            )
            .inspect(plugin),
    );
    // CacheSnapshotGoneTest
    let with_anthology = |b: &mut B, extra: &str| {
        b.file(
            INSTALLED,
            &format!(
                r#"{{"version": 2, "plugins": {{"anthology-guardrails@anthology": [{{"scope": "user", "installPath": "@ROOT@/{CACHE}", "version": "0.1.0", "lastUpdated": "2026-01-01T00:00:00Z"}}], "anthology@anthology": [{{"scope": "user", "installPath": "@ROOT@/anthology", "version": "0.1.0"}}]{extra}}}}}"#
            ),
        );
    };
    for (live, cache_dir, kind) in [
        (true, false, "directory"),
        (true, true, "directory"),
        (false, false, "directory"),
        (true, false, "github"),
    ] {
        let mut b = B::new();
        if live {
            b.guard(LIVE, "file");
        } else {
            b.dir(&format!("{LIVE}/hooks"));
        }
        if cache_dir {
            b.dir(&format!("{CACHE}/hooks"));
        }
        b.manifest(r#""./plugins/anthology-guardrails""#);
        if kind == "github" {
            b.known(
                r#"{"source": "github", "repo": "owner/anthology"}"#,
                "@ROOT@/market",
            );
        } else {
            b.directory_known();
        }
        with_anthology(&mut b, "");
        out.push(b.inspect(plugin));
        b.file(USER_SETTINGS, FLAG_FALSE);
        out.push(b.inspect(plugin));
    }
    {
        let mut b = B::new();
        b.guard("outside/anthology-guardrails", "file")
            .manifest(r#""../outside/anthology-guardrails""#)
            .directory_known();
        with_anthology(&mut b, "");
        out.push(b.inspect(plugin));
    }
    out.push(
        B::new()
            .guard(LIVE, "file")
            .manifest(r#""./plugins/anthology-guardrails""#)
            .directory_known()
            .file(
                INSTALLED,
                &format!(
                    r#"{{"version": 2, "plugins": {{"anthology@anthology": [{{"scope": "user", "installPath": "@ROOT@/anthology"}}], "anthology-guardrails@anthology": [{{"scope": "project", "projectPath": "@ROOT@/elsewhere", "installPath": "@ROOT@/{CACHE}", "lastUpdated": "2026-01-01T00:00:00Z"}}]}}}}"#
                ),
            )
            .inspect(plugin),
    );
    // BoundedReadTest, DecodeLikeClaudeCodeTest, EnabledValueTest, UsersOwnFilesAreUncapped
    let flag_pad =
        r#"{"enabledPlugins": {"anthology-guardrails@anthology": false}, "pad": "@PAD@"}"#;
    let pad = r#"{"pad": "@PAD@"}"#;
    let settings_cases: Vec<Edit> = vec![
        Box::new(|b| {
            b.file_pad(".claude/settings.json", flag_pad, "pad", LIMIT + 10);
        }),
        Box::new(|b| {
            b.file_pad(".claude/settings.local.json", pad, "pad", 3 << 20);
        }),
        Box::new(|b| {
            b.file_pad(".claude/settings.json", flag_pad, "pad", 3 << 19);
        }),
        Box::new(|b| {
            b.file_pad(".claude/settings.json", pad, "pad", 3 << 19);
        }),
        Box::new(|b| {
            b.file_pad(".claude/settings.json", pad, "pad", LIMIT - 11);
        }),
        Box::new(|b| {
            b.file_pad(".claude/settings.json", pad, "pad", LIMIT - 10);
        }),
        Box::new(|b| {
            b.file(".claude/settings.json", FLAG_FALSE);
        }),
        Box::new(|b| {
            b.file(
                ".claude/settings.json",
                r#"{"enabledPlugins": {"other@x": false}}"#,
            );
        }),
        Box::new(|_| {}),
        Box::new(|b| {
            b.file(".claude/settings.json", "");
        }),
        Box::new(|b| {
            b.file(".claude/settings.json", " \n");
        }),
        Box::new(|b| {
            b.file(USER_SETTINGS, "{not json");
        }),
        Box::new(|b| {
            b.link(".claude/settings.json", "/dev/null");
        }),
        Box::new(|b| {
            b.fifo(".claude/settings.json");
        }),
        Box::new(|b| {
            b.file(".claude/settings.json", "{}")
                .chmod(".claude/settings.json", 0);
        }),
        Box::new(|b| {
            b.link(".claude/settings.json", "/dev/zero");
        }),
        Box::new(|b| {
            b.file_pad(USER_SETTINGS, flag_pad, "pad", 3 << 20);
        }),
        Box::new(|b| {
            b.file_pad(USER_SETTINGS, pad, "pad", 3 << 20);
        }),
        Box::new(|b| {
            b.link(USER_SETTINGS, "/dev/null");
        }),
        Box::new(|b| {
            b.file_enc(
                ".claude/settings.json",
                &format!("{{{}}}", &FLAG_FALSE[1..FLAG_FALSE.len() - 1]),
                "utf16le-bom",
            );
        }),
        Box::new(|b| {
            b.file_enc(USER_SETTINGS, FLAG_FALSE, "utf16le-bom");
        }),
    ];
    for f in &settings_cases {
        let mut b = B::new();
        b.diverging();
        f(&mut b);
        out.push(b.inspect(plugin));
    }
    for name in ["settings.json", "settings.local.json"] {
        for t in ["{not json", "[1, 2", r#"{"a": }"#, "\u{0}"] {
            out.push(
                B::new()
                    .diverging()
                    .file(&format!(".claude/{name}"), t)
                    .inspect(plugin),
            );
        }
    }
    let flag = r#""enabledPlugins":{"anthology-guardrails@anthology":false}"#;
    let disabled = |b: &B| b.case("disabled", vec![("cwd", s("@ROOT@"))]);
    for (raw, name) in [
        (
            [b"\xff\xfe".as_slice(), &utf16le(&format!("{{{flag}}}"))].concat(),
            "settings.json",
        ),
        (
            [b"\xff\xfe".as_slice(), &utf16le(r#"{"other": 1}"#)].concat(),
            "settings.json",
        ),
        (
            [b"\xfe\xff".as_slice(), &utf16be(&format!("{{{flag}}}"))].concat(),
            "settings.json",
        ),
        (
            [b"\xfe\xff".as_slice(), &utf16be(r#"{"other": 1}"#)].concat(),
            "settings.json",
        ),
        (
            [b"\xef\xbb\xbf".as_slice(), format!("{{{flag}}}").as_bytes()].concat(),
            "settings.json",
        ),
        (
            [
                format!(r#"{{{flag},"env":{{"X":"a"#).as_bytes(),
                b"\xff\"}}".as_slice(),
            ]
            .concat(),
            "settings.json",
        ),
        (b"{\"env\":{\"X\":\"a\xff\"}}".to_vec(), "settings.json"),
        (
            [b"\xef\xbb\xbf".as_slice(), br#"{"other": 1}"#.as_slice()].concat(),
            "settings.local.json",
        ),
    ] {
        out.push(disabled(
            B::new().file_hex(&format!(".claude/{name}"), &raw),
        ));
    }
    out.push(disabled(B::new().file_pad(
        ".claude/settings.json",
        &format!("{{{flag},\"zz\":@DEEP@}}"),
        "deep",
        200_000,
    )));
    out.push(disabled(B::new().file_pad(
        ".claude/settings.local.json",
        r#"{"zz":@DEEP@}"#,
        "deep",
        200_000,
    )));
    out.push(disabled(B::new().file_hex(
        USER_SETTINGS,
        &[b"\xef\xbb\xbf".as_slice(), format!("{{{flag}}}").as_bytes()].concat(),
    )));
    for value in [
        "[]",
        r#"["0.1.0"]"#,
        r#""yes""#,
        r#""true""#,
        "null",
        "0",
        "1",
        "{}",
        r#"{"a": 1}"#,
        "false",
        "true",
        "1.0",
    ] {
        for place in [
            USER_SETTINGS,
            ".claude/settings.json",
            ".claude/settings.local.json",
        ] {
            let mut b = B::new();
            b.diverging().file(
                USER_SETTINGS,
                r#"{"enabledPlugins": {"anthology-guardrails@anthology": true}}"#,
            );
            b.file(
                place,
                &format!(r#"{{"enabledPlugins": {{"anthology-guardrails@anthology": {value}}}}}"#),
            );
            out.push(b.inspect(plugin));
        }
    }
    for key in [
        "anthology-guardrails-extra@x",
        "my-anthology-guardrails@x",
        "anthology-guardrails",
        "ANTHOLOGY-GUARDRAILS@anthology",
        "anthology-guardrails@other",
    ] {
        out.push(disabled(B::new().file(
            ".claude/settings.json",
            &format!(r#"{{"enabledPlugins": {{"{key}": false}}}}"#),
        )));
    }
    {
        let mut b = B::new();
        b.diverging().file_pad(
            INSTALLED,
            &format!(
                r#"{{"version": 2, "plugins": {{"anthology-guardrails@anthology": [{{"scope": "user", "installPath": "@ROOT@/{CACHE}", "lastUpdated": "2026-01-01T00:00:00Z"}}]}}, "pad": "@PAD@"}}"#
            ),
            "pad",
            3 << 20,
        );
        out.push(b.inspect(plugin));
        let mut b = B::new();
        b.diverging().file_pad(
            KNOWN,
            r#"{"anthology": {"source": {"source": "directory", "path": "@ROOT@/market"}, "installLocation": "@ROOT@/market"}, "pad": "@PAD@"}"#,
            "pad",
            3 << 20,
        );
        out.push(b.inspect(plugin));
        let mut b = B::new();
        b.diverging().file_pad(
            MANIFEST,
            r#"{"name": "anthology", "pad": "@PAD@", "plugins": [{"name": "anthology-guardrails", "source": "./plugins/anthology-guardrails"}]}"#,
            "pad",
            LIMIT + 10,
        );
        out.push(b.inspect(plugin));
    }
    // test_doctor compat rows
    out.extend(compat_seeds());
    out.extend(review_seeds());
    out
}

/// `market/plugins/s0 -> s1 -> … -> s<n-1> -> real-chain` (which has the guard).
fn chain(b: &mut B, n: usize) {
    b.guard("market/plugins/real-chain", "file");
    for i in 0..n {
        let to = if i + 1 == n {
            "real-chain".to_string()
        } else {
            format!("s{}", i + 1)
        };
        b.link(&format!("market/plugins/s{i}"), &to);
    }
}

/// Review round 1 (C1, C2, I1): chains, the user files' exact depth edge, loops, repeats.
fn review_seeds() -> Vec<Value> {
    let plugin = "@ROOT@/repo/plugins/anthology";
    let lone = "@ROOT@/lone/anthology";
    let mut out = Vec::new();
    for n in [5, 1100] {
        let mut b = B::new();
        chain(&mut b, n);
        b.guard(CACHE, "file")
            .manifest(r#""./plugins/s0""#)
            .directory_known()
            .installed();
        out.push(b.inspect(plugin));
        let mut b = B::new();
        chain(&mut b, n);
        b.manifest(r#""./plugins/s0""#)
            .directory_known()
            .installed_entry("@ROOT@/nowhere");
        out.push(b.inspect(plugin));
    }
    let nest = |n: usize| format!("{}{}", "[".repeat(n), "]".repeat(n));
    for depth in [9_996, 9_997] {
        // installed_plugins.json: the top object + `depth` = 9,997 (read) or 9,998 (invalid)
        let mut b = B::new();
        b.guard(CACHE, "file").file(
            INSTALLED,
            &format!(
                r#"{{"version": 2, "plugins": {{"anthology-guardrails@anthology": [{{"scope": "user", "installPath": "@ROOT@/{CACHE}"}}]}}, "pad": {}}}"#,
                nest(depth)
            ),
        );
        out.push(b.inspect(lone));
        // known_marketplaces.json
        let mut b = B::new();
        b.dir("plugins-dir/anthology")
            .guard("plugins-dir/anthology-guardrails", "file")
            .guard(CACHE, "file")
            .installed()
            .file(
                KNOWN,
                &format!(
                    r#"{{"anthology": {{"source": {{"source": "directory"}}, "installLocation": "@ROOT@/market"}}, "pad": {}}}"#,
                    nest(depth)
                ),
            );
        out.push(b.inspect("@ROOT@/plugins-dir/anthology"));
        // the user's settings.json
        let mut b = B::new();
        b.diverging().file(
            USER_SETTINGS,
            &format!(
                r#"{{"enabledPlugins": {{"anthology-guardrails@anthology": false}}, "pad": {}}}"#,
                nest(depth)
            ),
        );
        out.push(b.inspect(plugin));
        // compat.read_json: unbounded at both depths; bounded only past Python's limit (below
        // it, 513 and more is the deliberate band where Rust skips: module docs)
        let mut b = B::new();
        b.file("f.json", &format!(r#"{{"a": {}}}"#, nest(depth)));
        let limits = if depth == 9_997 {
            vec![Value::Null, int(LIMIT)]
        } else {
            vec![Value::Null]
        };
        for limit in limits {
            out.push(b.case(
                "read_json",
                vec![
                    ("path", s("@ROOT@/f.json")),
                    ("limit", limit),
                    ("found_only", Value::Bool(true)),
                ],
            ));
        }
    }
    for place in [
        USER_SETTINGS,
        ".claude/settings.json",
        ".claude/settings.local.json",
    ] {
        let mut b = B::new();
        b.diverging().link(place, &format!("@ROOT@/{place}"));
        out.push(b.inspect(plugin));
    }
    let k = r#""anthology-guardrails@anthology""#;
    for text in [
        format!(r#"{{"enabledPlugins": {{{k}: false, {k}: true}}}}"#),
        format!(r#"{{"enabledPlugins": {{{k}: true, {k}: false}}}}"#),
        format!(r#"{{"enabledPlugins": {{{k}: false}}, "enabledPlugins": {{}}}}"#),
        format!(r#"{{"enabledPlugins": {{}}, "enabledPlugins": {{{k}: []}}}}"#),
    ] {
        for place in [
            USER_SETTINGS,
            ".claude/settings.json",
            ".claude/settings.local.json",
        ] {
            out.push(
                B::new()
                    .file(place, &text)
                    .case("disabled", vec![("cwd", s("@ROOT@"))]),
            );
        }
    }
    out.push(
        B::new()
            .guard(CACHE, "file")
            .guard(LIVE, "file")
            .manifest(r#""./plugins/anthology-guardrails""#)
            .directory_known()
            .file(
                INSTALLED,
                &format!(
                    r#"{{"plugins": {{"anthology-guardrails@anthology": [{{"installPath": "@ROOT@/nowhere"}}]}}, "plugins": {{"anthology-guardrails@other": [{{"installPath": "@ROOT@/{CACHE}"}}]}}}}"#
                ),
            )
            .inspect(plugin),
    );
    out
}

fn utf16le(t: &str) -> Vec<u8> {
    t.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

fn utf16be(t: &str) -> Vec<u8> {
    t.encode_utf16().flat_map(u16::to_be_bytes).collect()
}

fn compat_seeds() -> Vec<Value> {
    let mut out = Vec::new();
    let doctor = || {
        let mut b = B::new();
        b.env("HOME", "@ROOT@")
            .env("CLAUDE_CONFIG_DIR", "@ROOT@/config")
            .dir("config")
            .dir("proj");
        b
    };
    let cwd = || vec![("cwd", s("@ROOT@/proj"))];
    // PathWrappersTest
    let mut b = doctor();
    for name in ["anthology", "anthology-mem", "rtk", "rtk-hook"] {
        b.file(&format!("plug/bin/{name}"), "#!/bin/sh\n")
            .chmod(&format!("plug/bin/{name}"), 0o755);
    }
    out.push(b.case("wrappers", vec![("plugin_root", s("@ROOT@/plug"))]));
    let mut b = doctor();
    for name in ["anthology", "anthology-mem", "rtk-hook"] {
        b.file(&format!("plug/bin/{name}"), "#!/bin/sh\n")
            .chmod(&format!("plug/bin/{name}"), 0o755);
    }
    b.chmod("plug/bin/rtk-hook", 0o644);
    out.push(b.case("wrappers", vec![("plugin_root", s("@ROOT@/plug"))]));
    out.push(doctor().case(
        "wrappers",
        vec![("plugin_root", s("@ROOT@/repo/plugins/anthology"))],
    ));
    // AgentMemoryPathTest
    let mem = "proj/.claude/agent-memory";
    for dirs in [
        vec![],
        vec![""],
        vec!["anthology-implementer"],
        vec!["implementer"],
        vec!["reviewer", "anthology-implementer"],
        vec!["anthology:implementer"],
        vec!["anthology_debugger"],
        vec!["notes", "anthology-implementer"],
    ] {
        let mut b = doctor();
        b.dir("proj/.git");
        for d in dirs {
            b.dir(&format!("{mem}/{d}"));
        }
        out.push(b.case("agent_memory", cwd()));
        b.dir("proj/pkg/x");
        out.push(b.case("agent_memory", vec![("cwd", s("@ROOT@/proj/pkg/x"))]));
    }
    // SubagentModelTest
    let model = r#"{"env": {"CLAUDE_CODE_SUBAGENT_MODEL": "m"}}"#;
    out.push(doctor().case("subagent_model", cwd()));
    for v in ["SECRETMODEL", "", " "] {
        out.push(
            doctor()
                .env("CLAUDE_CODE_SUBAGENT_MODEL", v)
                .case("subagent_model", cwd()),
        );
    }
    for path in [
        "config/settings.json",
        "proj/.claude/settings.json",
        "proj/.claude/settings.local.json",
    ] {
        out.push(doctor().file(path, model).case("subagent_model", cwd()));
    }
    out.push(
        doctor()
            .file("config/settings.json", r#"{"env": {"OTHER": "1"}}"#)
            .file("proj/.claude/settings.json", "{nope")
            .file("proj/.claude/settings.local.json", r#"{"env": "bad"}"#)
            .case("subagent_model", cwd()),
    );
    let model_pad = r#"{"env": {"CLAUDE_CODE_SUBAGENT_MODEL": "m"}, "pad": "@PAD@"}"#;
    let base = model_pad.len() as i64 - "@PAD@".len() as i64;
    for (path, n) in [
        ("proj/.claude/settings.json", LIMIT + 10),
        ("proj/.claude/settings.local.json", LIMIT + 10),
        ("proj/.claude/settings.json", LIMIT - base),
        ("proj/.claude/settings.json", LIMIT - base + 1),
        ("config/settings.json", LIMIT + 10),
    ] {
        out.push(
            doctor()
                .file_pad(path, model_pad, "pad", n)
                .case("subagent_model", cwd()),
        );
    }
    for raw in [
        [b"\xff\xfe".as_slice(), &utf16le(model)].concat(),
        [b"\xfe\xff".as_slice(), &utf16be(model)].concat(),
        [b"\xef\xbb\xbf".as_slice(), model.as_bytes()].concat(),
        [
            &model.as_bytes()[..model.len() - 2],
            b", \"x\": \"a\xff\"}}".as_slice(),
        ]
        .concat(),
    ] {
        out.push(
            doctor()
                .file_hex("proj/.claude/settings.json", &raw)
                .case("subagent_model", cwd()),
        );
    }
    out.push(
        doctor()
            .file("real.json", model)
            .link("proj/.claude/settings.json", "@ROOT@/real.json")
            .case("subagent_model", cwd()),
    );
    out.push(
        doctor()
            .link("proj/.claude/settings.json", "/dev/zero")
            .case("subagent_model", cwd()),
    );
    out.push(
        doctor()
            .fifo("proj/.claude/settings.json")
            .case("subagent_model", cwd()),
    );
    let mut b = doctor();
    b.fifo("settings.json");
    out.push(b.case(
        "read_json",
        vec![("path", s("@ROOT@/settings.json")), ("limit", int(LIMIT))],
    ));
    out
}

// ---------------------------------------------------------------------------------------------
// The generator

/// A JSON value of any type, nesting at most `depth` more levels (no lone surrogates; strings
/// inside containers use characters whose `isprintable` is known without P1's tables).
fn rand_value(rng: &mut Rng, depth: usize) -> Value {
    let k = rng.below(12);
    match k {
        0 => Value::Null,
        1 => Value::Bool(true),
        2 => Value::Bool(false),
        3 => int(*rng.pick(&[0, 1, -1, 5, 20260101, i64::MAX, i64::MIN])),
        4 => Value::Int(Int::Big(
            (*rng.pick(&["123456789012345678901234567890", "-99999999999999999999"])).into(),
        )),
        5 => Value::Float(*rng.pick(&[
            0.0,
            -0.0,
            0.5,
            1e16,
            1e-7,
            2.5,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NAN,
            123.456,
        ])),
        6 | 7 => s(&rand_text(rng)),
        8 | 9 if depth > 0 => Value::Array(
            (0..rng.below(4))
                .map(|_| rand_value(rng, depth - 1))
                .collect(),
        ),
        10 | 11 if depth > 0 => {
            let mut m = Map::new();
            for _ in 0..rng.below(4) {
                let key = rand_text(rng);
                let v = rand_value(rng, depth - 1);
                m.insert(key, v);
            }
            Value::Object(m)
        }
        _ => s("2026-01-01T00:00:00Z"),
    }
}

/// A short string: dates, quotes, backslashes, controls and a few known non-ASCII characters.
fn rand_text(rng: &mut Rng) -> String {
    const PIECES: &[&str] = &[
        "2026-01-01T00:00:00Z",
        "2025-12-31",
        "2026",
        "",
        "a",
        "Z",
        "'",
        "\"",
        "\\",
        "\n",
        "\t",
        "\r",
        "\u{1}",
        "\u{7f}",
        "\u{85}",
        "\u{a0}",
        "\u{ad}",
        "é",
        "€",
        "\u{2028}",
        "\u{feff}",
        "😀",
        "日本",
        " ",
        "[",
        "{",
    ];
    (0..rng.below(4)).map(|_| *rng.pick(PIECES)).collect()
}

const KEYS: &[&str] = &[
    "anthology-guardrails@anthology",
    "anthology-guardrails@anthology",
    "anthology-guardrails@other",
    "anthology-guardrails@",
    "anthology-guardrails@a@b",
    "anthology-guardrails-extra@x",
    "my-anthology-guardrails@x",
    "anthology-guardrails",
    "ANTHOLOGY-GUARDRAILS@anthology",
    " anthology-guardrails@anthology",
    "anthology@anthology",
    "superpowers@x",
];

fn rand_enabled_value(rng: &mut Rng) -> Value {
    match rng.below(14) {
        0..=4 => Value::Bool(true),
        5 | 6 => Value::Bool(false),
        7 => Value::Null,
        8 => int(*rng.pick(&[0, 1])),
        9 => Value::Float(1.0),
        10 => s(rng.pick(&["true", "yes", "", "false"])),
        11 => Value::Array(vec![]),
        12 => Value::Array(vec![s("0.1.0")]),
        _ => rand_value(rng, 2),
    }
}

/// A settings object (`enabledPlugins` and friends).
fn rand_settings_obj(rng: &mut Rng) -> Value {
    let mut m = Map::new();
    if rng.chance(0.85) {
        let enabled = if rng.chance(0.9) {
            let mut e = Map::new();
            for _ in 0..rng.below(4) {
                e.insert((*rng.pick(KEYS)).to_string(), rand_enabled_value(rng));
            }
            Value::Object(e)
        } else {
            rand_value(rng, 1)
        };
        m.insert("enabledPlugins".into(), enabled);
    }
    if rng.chance(0.3) {
        m.insert("env".into(), rand_env_block(rng));
    }
    if rng.chance(0.2) {
        m.insert("permissions".into(), rand_value(rng, 2));
    }
    Value::Object(m)
}

fn rand_env_block(rng: &mut Rng) -> Value {
    if rng.chance(0.15) {
        return rand_value(rng, 1);
    }
    let mut e = Map::new();
    if rng.chance(0.8) {
        let v = match rng.below(8) {
            0..=2 => s("m"),
            3 => s(""),
            4 => s(" \t"),
            5 => s("\u{3000}x"),
            6 => s("\u{1c}"),
            _ => rand_value(rng, 1),
        };
        e.insert("CLAUDE_CODE_SUBAGENT_MODEL".into(), v);
    }
    if rng.chance(0.3) {
        e.insert("OTHER".into(), s("1"));
    }
    Value::Object(e)
}

/// The bytes of `text` in one of the encodings, perhaps with an invalid byte inside a string.
fn rand_encoded(rng: &mut Rng, text: &str) -> Vec<u8> {
    let enc = rng.below(10);
    let mut t = text.to_string();
    let bad = rng.chance(0.15);
    if bad && let Some(i) = t.rfind('"') {
        t.insert_str(i, "@BAD@");
    }
    let body = |t: &str, f: &dyn Fn(&str) -> Vec<u8>| -> Vec<u8> {
        let mut out = Vec::new();
        for (i, part) in t.split("@BAD@").enumerate() {
            if i > 0 {
                out.extend_from_slice(match enc {
                    7 | 8 => b"\x00\xd8", // a lone high surrogate unit (UTF-16)
                    _ => b"\xff",
                });
            }
            out.extend(f(part));
        }
        out
    };
    match enc {
        0..=5 => body(&t, &|p| p.as_bytes().to_vec()),
        6 => [
            b"\xef\xbb\xbf".as_slice(),
            &body(&t, &|p| p.as_bytes().to_vec()),
        ]
        .concat(),
        7 => [b"\xff\xfe".as_slice(), &body(&t, &|p| utf16le(p))].concat(),
        8 => [b"\xfe\xff".as_slice(), &body(&t, &|p| utf16be(p))].concat(),
        _ => {
            // UTF-16LE with an odd trailing byte or garbage after
            let mut out = [b"\xff\xfe".as_slice(), &utf16le(text)].concat();
            out.push(b'}');
            out
        }
    }
}

/// `v` as JSON text, an object's members perhaps repeated: a member of the top object or of one
/// object below it is written again with another value (Python keeps the last value at the
/// first position, as `json::Map` does).
fn with_repeats(rng: &mut Rng, v: &Value) -> String {
    fn members(rng: &mut Rng, o: &Map, nested: bool) -> String {
        let mut parts: Vec<String> = Vec::new();
        for (k, item) in o.iter() {
            let body = match item.as_object() {
                Some(inner) if nested => members(rng, inner, false),
                _ => text(item),
            };
            parts.push(format!("{}: {body}", text(&s(k))));
        }
        let keys: Vec<&str> = o.iter().map(|(k, _)| k).collect();
        if !keys.is_empty() && rng.chance(0.6) {
            let k = *rng.pick(&keys);
            let again = format!("{}: {}", text(&s(k)), text(&rand_enabled_value(rng)));
            if rng.chance(0.5) {
                parts.push(again);
            } else {
                parts.insert(0, again);
            }
        }
        format!("{{{}}}", parts.join(", "))
    }
    match v.as_object() {
        Some(o) => members(rng, o, true),
        None => text(v),
    }
}

/// One settings file at `path` (or nothing), as a repo-controlled file when `repo`.
fn rand_settings(rng: &mut Rng, b: &mut B, path: &str, repo: bool, model: bool) {
    let k = rng.f64();
    if k < 0.45 {
        return;
    }
    let obj = if model && rng.chance(0.7) {
        let mut m = Map::new();
        m.insert("env".into(), rand_env_block(rng));
        Value::Object(m)
    } else {
        rand_settings_obj(rng)
    };
    let body = if rng.chance(0.2) {
        with_repeats(rng, &obj)
    } else {
        text(&obj)
    };
    if k < 0.80 {
        let raw = rand_encoded(rng, &body);
        b.file_hex(path, &raw);
        return;
    }
    if k < 0.86 {
        let t = *rng.pick(&[
            "",
            " \n",
            "\u{1c}",
            "\u{3000}",
            "\u{feff}",
            "\u{feff} ",
            "\t\r\n",
            "\u{85}",
            "\u{200b}",
        ]);
        let enc = *rng.pick(&["utf8", "utf8", "utf16le-bom", "utf8-bom"]);
        b.file_enc(path, t, enc);
        return;
    }
    if k < 0.91 {
        let t = *rng.pick(&[
            "{not json",
            "[1, 2",
            r#"{"a": }"#,
            "\u{0}",
            "{} x",
            "nul",
            r#"{"enabledPlugins": {"anthology-guardrails@a": false}"#,
            "[]",
            "5",
            "\"s\"",
            "null",
            "true",
            r#"{"enabledPlugins": []}"#,
            r#"{"enabledPlugins": "x"}"#,
            r#"{"a": NaN, "enabledPlugins": {"anthology-guardrails@x": Infinity}}"#,
        ]);
        b.file(path, t);
        return;
    }
    if k < 0.95 {
        match rng.below(if model && !repo { 3 } else { 6 }) {
            0 => {
                b.dir(path);
            }
            1 => {
                b.file("elsewhere-real.json", &body)
                    .link(path, "@ROOT@/elsewhere-real.json");
            }
            5 => {
                b.link(path, &format!("@ROOT@/{path}")); // a link to itself: ELOOP
            }
            2 => {
                b.link(path, "/dev/null");
            }
            3 => {
                b.fifo(path);
            }
            _ => {
                b.link(path, "/dev/zero");
            }
        }
        return;
    }
    if k < 0.97 {
        let raw: Vec<u8> = (0..rng.below(24)).map(|_| rng.below(256) as u8).collect();
        let prefix: &[u8] = rng.pick(&[b"".as_slice(), b"\xff\xfe", b"\xfe\xff", b"\xef\xbb\xbf"]);
        b.file_hex(path, &[prefix, &raw].concat());
        return;
    }
    if k < 0.98 {
        b.file(path, &body).chmod(path, 0);
        return;
    }
    if k < 0.99 {
        // sizes around the limit (rare: 2 MiB per side)
        let flagged = format!(
            r#"{{"enabledPlugins": {{"anthology-guardrails@anthology": {}}}, "env": {{"CLAUDE_CODE_SUBAGENT_MODEL": "m"}}, "pad": "@PAD@"}}"#,
            if rng.chance(0.5) { "false" } else { "true" }
        );
        let base = flagged.len() as i64 - 5;
        let n = *rng.pick(&[LIMIT - base, LIMIT - base + 1, LIMIT + 10, 3 << 19, 1000]);
        b.file_pad(path, &flagged, "pad", n);
        return;
    }
    // deep nesting, past every limit (module docs)
    let t = if rng.chance(0.5) {
        r#"{"enabledPlugins": {"anthology-guardrails@anthology": true}, "zz": @DEEP@}"#
    } else {
        r#"{"zz": @DEEP@}"#
    };
    b.file_pad(path, t, "deep", 20_000 + rng.below(10_000) as i64);
}

fn guard_kind(rng: &mut Rng) -> &'static str {
    match rng.below(20) {
        0 => "dir",
        1 => "link",
        2 => "dangling",
        _ => "file",
    }
}

const PLUGIN_ROOTS: &[&str] = &[
    "@ROOT@/plugins-dir/anthology",
    "@ROOT@/plugins-dir/anthology",
    "@ROOT@/market/plugins/anthology",
    "@ROOT@/market/plugins/anthology",
    "@ROOT@/lone/anthology",
    "@ROOT@/hm/.claude/plugins/cache/anthology/anthology/0.1.0",
    "@ROOT@/dev/anthology",
    "@ROOT@/plugins-dir/anthology/",
    "@ROOT@/plugins-dir//anthology",
    "@ROOT@/plugins-dir/./anthology",
    "@ROOT@/link-pd/anthology",
    "@ROOT@/plugins-dir/x/../anthology",
];

const INSTALL_PATHS: &[&str] = &[
    "@ROOT@/hm/.claude/plugins/cache/anthology/anthology-guardrails/0.1.0",
    "@ROOT@/hm/.claude/plugins/cache/anthology/anthology-guardrails/0.1.0",
    "@ROOT@/hm/.claude/plugins/cache/anthology/anthology-guardrails/0.1.0/",
    "@ROOT@/hm/.claude/plugins/cache/anthology/anthology-guardrails/0.2.0",
    "@ROOT@/market/plugins/anthology-guardrails",
    "@ROOT@//market/plugins/./anthology-guardrails",
    "@ROOT@/plugins-dir/anthology-guardrails",
    "@ROOT@/outside/anthology-guardrails",
    "@ROOT@/nowhere",
    "",
];

fn rand_entry(rng: &mut Rng) -> Value {
    let mut m = Map::new();
    if rng.chance(0.92) {
        let v = if rng.chance(0.95) {
            s(rng.pick(INSTALL_PATHS))
        } else {
            rand_value(rng, 0)
        };
        m.insert("installPath".into(), v);
    }
    match rng.below(10) {
        0..=4 => m.insert("scope".into(), s("user")),
        5 => m.insert("scope".into(), s("project")),
        6 => m.insert("scope".into(), s("local")),
        7 => m.insert("scope".into(), rand_value(rng, 1)),
        _ => {}
    }
    if rng.chance(0.4) {
        let v = match rng.below(9) {
            0 | 1 => s("@ROOT@/proj"),
            2 => s("@ROOT@"),
            3 => s("@ROOT@/proj/"),
            4 => s("@ROOT@/elsewhere"),
            5 => s("@ROOT@/link-proj"),
            6 => s("/"),
            7 => s("@ROOT@/pro"),
            _ => rand_value(rng, 0),
        };
        m.insert("projectPath".into(), v);
    }
    if rng.chance(0.8) {
        let v = if rng.chance(0.6) {
            s(rng.pick(&[
                "2026-01-01T00:00:00Z",
                "2026-02-01T00:00:00Z",
                "2025-12-31T23:59:59Z",
                "2026-01-01T00:00:00Z",
            ]))
        } else {
            rand_value(rng, 2)
        };
        m.insert("lastUpdated".into(), v);
    }
    if rng.chance(0.2) {
        m.insert("version".into(), s("0.1.0"));
    }
    Value::Object(m)
}

fn rand_installed(rng: &mut Rng, b: &mut B) {
    let k = rng.f64();
    if k < 0.06 {
        return;
    }
    if k < 0.12 {
        b.file(
            INSTALLED,
            rng.pick(&[
                "{not json",
                "[]",
                r#"{"plugins": []}"#,
                "",
                r#"{"plugins": {"anthology-guardrails@anthology": "x"}}"#,
            ]),
        );
        return;
    }
    let mut plugins = Map::new();
    for i in 0..rng.below(4) + 1 {
        let key = if i == 0 && rng.chance(0.5) {
            "anthology-guardrails@anthology".to_string()
        } else {
            (*rng.pick(KEYS)).to_string()
        };
        let v = if rng.chance(0.93) {
            Value::Array((0..rng.below(3) + 1).map(|_| rand_entry(rng)).collect())
        } else {
            rand_value(rng, 1)
        };
        plugins.insert(key, v);
    }
    let mut top = Map::new();
    top.insert("version".into(), int(2));
    top.insert("plugins".into(), Value::Object(plugins));
    let top = Value::Object(top);
    if rng.chance(0.1) {
        let raw = with_repeats(rng, &top);
        b.file(INSTALLED, &raw);
    } else {
        b.json(INSTALLED, &top);
    }
}

fn rand_known(rng: &mut Rng, b: &mut B) {
    let k = rng.f64();
    if k < 0.15 {
        return;
    }
    if k < 0.22 {
        b.file(
            KNOWN,
            rng.pick(&[
                "{not json",
                "[]",
                r#"{"anthology": 3}"#,
                r#"{"anthology": {"source": "directory"}}"#,
                "",
            ]),
        );
        return;
    }
    let mut known = Map::new();
    for name in ["anthology", "other", "a@b"] {
        if name != "anthology" && rng.chance(0.7) {
            continue;
        }
        let source = match rng.below(12) {
            0..=4 => json::loads(r#"{"source": "directory", "path": "@ROOT@/market"}"#).unwrap(),
            5 | 6 => json::loads(r#"{"source": "github", "repo": "owner/anthology"}"#).unwrap(),
            7 => {
                json::loads(r#"{"source": "git", "url": "https://example.invalid/x.git"}"#).unwrap()
            }
            8 => json::loads(r#"{"source": "url"}"#).unwrap(),
            9 => s("directory"),
            10 => json::loads(r#"{"source": 5}"#).unwrap(),
            _ => json::loads(r#"{"source": "Directory"}"#).unwrap(),
        };
        let mut item = Map::new();
        item.insert("source".into(), source);
        if rng.chance(0.93) {
            let loc = match rng.below(12) {
                0..=5 => s("@ROOT@/market"),
                6 => s("@ROOT@/link-market"),
                7 => s("@ROOT@/market/"),
                8 => s("@ROOT@//market"),
                9 => s("market"),
                10 => s(""),
                _ => rand_value(rng, 0),
            };
            item.insert("installLocation".into(), loc);
        }
        item.insert("lastUpdated".into(), s("x"));
        known.insert(name.into(), Value::Object(item));
    }
    b.json(KNOWN, &Value::Object(known));
}

const SOURCES: &[&str] = &[
    "./plugins/anthology-guardrails",
    "./plugins/anthology-guardrails",
    "./plugins/anthology-guardrails",
    "plugins/anthology-guardrails",
    "./plugins/anthology-guardrails/",
    "./plugins/../plugins/anthology-guardrails",
    "../outside/anthology-guardrails",
    "./plugins/../../outside/anthology-guardrails",
    "@ROOT@/outside/anthology-guardrails",
    "@ROOT@/market/plugins/anthology-guardrails",
    "",
    ".",
    "./plugins/link-out",
    "./plugins/link-in",
    "./plugins/anthology-guardrails\u{0}x",
    "./plugins/s0",
];

fn rand_manifest(rng: &mut Rng, b: &mut B) {
    let k = rng.f64();
    if k < 0.12 {
        return;
    }
    if k < 0.2 {
        b.file(
            MANIFEST,
            rng.pick(&[
                "{broken",
                r#"{"plugins": "x"}"#,
                r#"{"plugins": []}"#,
                "",
                "[]",
            ]),
        );
        return;
    }
    if k < 0.21 {
        b.file_pad(
            MANIFEST,
            r#"{"pad": "@PAD@", "plugins": [{"name": "anthology-guardrails", "source": "./plugins/anthology-guardrails"}]}"#,
            "pad",
            *rng.pick(&[LIMIT + 10, 1000]),
        );
        return;
    }
    if k < 0.22 {
        b.file_pad(
            MANIFEST,
            r#"{"zz": @DEEP@, "plugins": [{"name": "anthology-guardrails", "source": "./plugins/anthology-guardrails"}]}"#,
            "deep",
            20_000 + rng.below(10_000) as i64,
        );
        return;
    }
    let mut specs = Vec::new();
    for _ in 0..rng.below(3) + 1 {
        let mut spec = Map::new();
        let name = match rng.below(10) {
            0..=6 => s("anthology-guardrails"),
            7 => s("other"),
            8 => s("anthology"),
            _ => rand_value(rng, 0),
        };
        spec.insert("name".into(), name);
        let source = if rng.chance(0.9) {
            s(rng.pick(SOURCES))
        } else {
            rand_value(rng, 1)
        };
        spec.insert("source".into(), source);
        specs.push(if rng.chance(0.95) {
            Value::Object(spec)
        } else {
            rand_value(rng, 0)
        });
    }
    let mut top = Map::new();
    top.insert("name".into(), s("anthology"));
    top.insert("plugins".into(), Value::Array(specs));
    let raw = rand_encoded(rng, &text(&Value::Object(top)));
    b.file_hex(MANIFEST, &raw);
}

fn rand_inspect_layout(rng: &mut Rng, b: &mut B) {
    if rng.chance(0.15) {
        b.env("CLAUDE_CONFIG_DIR", rng.pick(&["", "@ROOT@/hm/.claude/"]));
        b.env("HOME", rng.pick(&["@ROOT@/hm", "@ROOT@/hm/"]));
    }
    // guardrails folders
    if rng.chance(0.6) {
        b.guard(CACHE, guard_kind(rng));
    } else if rng.chance(0.3) {
        b.dir(&format!("{CACHE}/hooks"));
    }
    match rng.below(10) {
        0..=5 => {
            b.guard(LIVE, guard_kind(rng));
        }
        6 => {
            b.dir(&format!("{LIVE}/hooks"));
        }
        7 => {
            b.guard("outside/anthology-guardrails", "file")
                .dir("market/plugins")
                .link(LIVE, "@ROOT@/outside/anthology-guardrails");
        }
        8 => {
            b.guard("market/plugins/real-g", "file")
                .link(LIVE, "@ROOT@/market/plugins/real-g");
        }
        _ => {}
    }
    if rng.chance(0.5) {
        b.guard("outside/anthology-guardrails", "file");
    }
    if rng.chance(0.15) {
        b.guard(MARKET, "file");
    }
    b.dir("market/plugins");
    if rng.chance(0.2) {
        b.link(
            "market/plugins/link-out",
            "@ROOT@/outside/anthology-guardrails",
        );
    }
    if rng.chance(0.06) {
        let n = *rng.pick(&[1, 2, 8, 1100]);
        chain(b, n);
    }
    if rng.chance(0.2) {
        b.link(
            "market/plugins/link-in",
            "@ROOT@/market/plugins/anthology-guardrails",
        );
    }
    if rng.chance(0.3) {
        b.link("link-market", "@ROOT@/market");
    }
    match rng.below(10) {
        0..=4 => {
            b.guard("plugins-dir/anthology-guardrails", guard_kind(rng));
        }
        5 => {
            b.guard("outside/anthology-guardrails", "file")
                .dir("plugins-dir")
                .link(
                    "plugins-dir/anthology-guardrails",
                    "@ROOT@/outside/anthology-guardrails",
                );
        }
        6 => {
            b.guard("plugins-dir", "file")
                .link("plugins-dir/anthology-guardrails", "@ROOT@/plugins-dir");
        }
        7 => {
            b.guard("plugins-dir/real-g", "file").link(
                "plugins-dir/anthology-guardrails",
                "@ROOT@/plugins-dir/real-g",
            );
        }
        _ => {}
    }
    if rng.chance(0.5) {
        b.dir("plugins-dir/anthology").dir("plugins-dir/x");
    }
    if rng.chance(0.2) {
        b.link("link-pd", "@ROOT@/plugins-dir");
    }
    if rng.chance(0.3) {
        b.guard("dev/anthology-guardrails", guard_kind(rng));
    }
    if rng.chance(0.3) {
        b.guard(
            "hm/.claude/plugins/cache/anthology/anthology-guardrails",
            "file",
        )
        .dir(CACHED_ANTHOLOGY);
    }
    // cwd folders (`link-proj` is made by the caller)
    b.dir("proj/sub").dir("elsewhere");
    rand_installed(rng, b);
    rand_known(rng, b);
    rand_manifest(rng, b);
}

fn rand_cwd(rng: &mut Rng) -> &'static str {
    match rng.below(10) {
        0..=6 => "@ROOT@/proj",
        7 => "@ROOT@/proj/sub",
        8 => "@ROOT@/elsewhere",
        _ => "@ROOT@/link-proj",
    }
}

fn settings_for(rng: &mut Rng, b: &mut B, cwd: &str, model: bool) {
    let rel = cwd.trim_start_matches("@ROOT@/");
    let rel = if rel == "@ROOT@" { "" } else { rel };
    let rel = if rel == "link-proj" { "proj" } else { rel };
    rand_settings(rng, b, USER_SETTINGS, false, model);
    rand_settings(rng, b, &format!("{rel}/.claude/settings.json"), true, model);
    rand_settings(
        rng,
        b,
        &format!("{rel}/.claude/settings.local.json"),
        true,
        model,
    );
}

fn gen_case(rng: &mut Rng) -> Value {
    let mut b = B::new();
    let k = rng.below(100);
    if k < 62 {
        rand_inspect_layout(rng, &mut b);
        let cwd = rand_cwd(rng);
        b.link("link-proj", "@ROOT@/proj");
        settings_for(rng, &mut b, cwd, false);
        let root = *rng.pick(PLUGIN_ROOTS);
        return b.case("inspect", vec![("plugin_root", s(root)), ("cwd", s(cwd))]);
    }
    if k < 75 {
        b.dir("proj/sub").link("link-proj", "@ROOT@/proj");
        let cwd = rand_cwd(rng);
        settings_for(rng, &mut b, cwd, false);
        return b.case("disabled", vec![("cwd", s(cwd))]);
    }
    if k < 83 {
        return gen_subagent(rng, b);
    }
    if k < 88 {
        return gen_agent_memory(rng, b);
    }
    if k < 92 {
        return gen_wrappers(rng, b);
    }
    if k < 95 {
        return gen_repo_top(rng, b);
    }
    if k < 98 {
        return gen_read_json(rng, b);
    }
    let raw: Vec<u8> = (0..rng.below(40)).map(|_| rng.below(256) as u8).collect();
    let prefix: &[u8] = rng.pick(&[
        b"".as_slice(),
        b"\xff\xfe",
        b"\xfe\xff",
        b"\xef\xbb\xbf",
        b"\xef\xbb\xbf\xef\xbb\xbf",
    ]);
    let mut raw = [prefix, &raw].concat();
    if rng.chance(0.3) {
        // UTF-16 code units around the surrogates
        for _ in 0..rng.below(6) {
            let u: u16 = *rng.pick(&[
                0xd800, 0xdbff, 0xdc00, 0xdfff, 0xd83d, 0xde00, 0x0041, 0xfeff,
            ]);
            raw.extend(if rng.chance(0.5) {
                u.to_le_bytes()
            } else {
                u.to_be_bytes()
            });
        }
    }
    b.case("decode", vec![("hex", s(&hex(&raw)))])
}

fn gen_repo_layout(rng: &mut Rng, b: &mut B) {
    b.dir("proj/pkg/x");
    match rng.below(6) {
        0 | 1 => {
            b.dir("proj/.git");
        }
        2 => {
            b.file("proj/.git", "gitdir: elsewhere\n");
        }
        3 => {
            b.dir(".git");
        }
        4 => {
            b.link("proj/.git", "@ROOT@/nowhere");
        }
        _ => {}
    }
    if rng.chance(0.2) {
        b.link("link-proj", "@ROOT@/proj/pkg");
    }
}

fn repo_cwd(rng: &mut Rng) -> &'static str {
    rng.pick(&[
        "@ROOT@/proj",
        "@ROOT@/proj/pkg/x",
        "@ROOT@/proj/pkg/x/",
        "@ROOT@/proj/./pkg/../pkg/x",
        "@ROOT@/proj/missing/deeper",
        "@ROOT@",
    ])
}

fn gen_subagent(rng: &mut Rng, mut b: B) -> Value {
    if rng.chance(0.15) {
        b.env(
            "CLAUDE_CODE_SUBAGENT_MODEL",
            rng.pick(&["", " ", "m", "\u{3000}", "\u{1c}", "SECRETMODEL"]),
        );
    }
    gen_repo_layout(rng, &mut b);
    let cwd = repo_cwd(rng);
    let top = if rng.chance(0.75) {
        "proj"
    } else {
        "proj/pkg/x"
    };
    if rng.chance(0.4) {
        rand_settings(rng, &mut b, USER_SETTINGS, false, true);
    }
    rand_settings(
        rng,
        &mut b,
        &format!("{top}/.claude/settings.json"),
        true,
        true,
    );
    rand_settings(
        rng,
        &mut b,
        &format!("{top}/.claude/settings.local.json"),
        true,
        true,
    );
    b.case("subagent_model", vec![("cwd", s(cwd))])
}

fn gen_agent_memory(rng: &mut Rng, mut b: B) -> Value {
    gen_repo_layout(rng, &mut b);
    let mem = if rng.chance(0.7) {
        "proj/.claude/agent-memory"
    } else {
        ".claude/agent-memory"
    };
    if rng.chance(0.15) {
        b.file(mem, "x");
        return b.case("agent_memory", vec![("cwd", s(repo_cwd(rng)))]);
    }
    b.dir(mem);
    let mut used: Vec<String> = Vec::new();
    for _ in 0..rng.below(4) {
        let agent = *rng.pick(&anthology_core::roster::ROSTER);
        let name = match rng.below(6) {
            0 => agent.to_string(),
            1 => format!("anthology:{agent}"),
            2 => format!("anthology_{agent}"),
            3 => format!("anthology-{agent}"),
            4 => "notes".to_string(),
            _ => format!("Anthology-{agent}"),
        };
        if used.contains(&name) {
            continue;
        }
        used.push(name.clone());
        let path = format!("{mem}/{name}");
        match rng.below(6) {
            0 => {
                b.file(&path, "x");
            }
            1 => {
                b.link(&path, "@ROOT@/proj/pkg");
            }
            2 => {
                b.link(&path, "@ROOT@/nowhere");
            }
            _ => {
                b.dir(&path);
            }
        }
    }
    b.case("agent_memory", vec![("cwd", s(repo_cwd(rng)))])
}

fn gen_wrappers(rng: &mut Rng, mut b: B) -> Value {
    let healthy = rng.chance(0.4);
    for name in compat::WRAPPERS {
        let path = format!("plug/bin/{name}");
        if healthy && rng.chance(0.85) {
            b.file(&path, "#!/bin/sh\n").chmod(&path, 0o755);
            continue;
        }
        match rng.below(10) {
            0..=4 => {
                b.file(&path, "#!/bin/sh\n").chmod(
                    &path,
                    *rng.pick(&[0o755, 0o700, 0o644, 0o600, 0o000, 0o500]),
                );
            }
            5 => {
                b.dir(&path);
            }
            6 => {
                b.file("plug/real", "#!/bin/sh\n")
                    .chmod("plug/real", 0o755)
                    .link(&path, "@ROOT@/plug/real");
            }
            7 => {
                b.link(&path, "@ROOT@/nowhere");
            }
            _ => {}
        }
    }
    let root = *rng.pick(&["@ROOT@/plug", "@ROOT@/plug/", "@ROOT@/nope"]);
    b.case("wrappers", vec![("plugin_root", s(root))])
}

fn gen_repo_top(rng: &mut Rng, mut b: B) -> Value {
    gen_repo_layout(rng, &mut b);
    if rng.chance(0.2) {
        b.link("proj/pkg/loop", "@ROOT@/proj/pkg/loop");
    }
    let cwd = *rng.pick(&[
        "@ROOT@/proj",
        "@ROOT@/proj/pkg/x",
        "@ROOT@/proj/pkg/x/../x",
        "@ROOT@/link-proj/x",
        "@ROOT@/proj/pkg/loop/a",
        "@ROOT@/proj/missing",
        "@ROOT@//proj//pkg",
    ]);
    b.case("repo_top", vec![("cwd", s(cwd))])
}

fn gen_read_json(rng: &mut Rng, mut b: B) -> Value {
    let limit = match rng.below(4) {
        0 => Value::Null,
        1 => int(LIMIT),
        2 => int(rng.below(40) as i64),
        _ => int(1000),
    };
    let bounded = !limit.is_null();
    let obj = rand_value(rng, 3);
    match rng.below(10) {
        0..=5 => {
            let raw = rand_encoded(rng, &text(&obj));
            b.file_hex("f.json", &raw);
        }
        6 => {
            b.dir("f.json");
        }
        7 => {
            b.link("f.json", "/dev/null");
        }
        8 if bounded => {
            b.fifo("f.json");
        }
        _ => {}
    }
    b.case(
        "read_json",
        vec![("path", s("@ROOT@/f.json")), ("limit", limit)],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_case_runs_on_the_rust_side() {
        for case in seeds() {
            let o = run(&case);
            assert_eq!(o.exit, 0);
        }
        let mut rng = Rng::new(7);
        for _ in 0..200 {
            let case = gen_case(&mut rng);
            assert_eq!(run(&case).exit, 0, "{}", text(&case));
        }
    }

    /// The generator reaches every outcome of every op, so a clean run means something.
    #[test]
    fn generated_cases_reach_every_outcome() {
        let mut seen = std::collections::BTreeMap::<String, usize>::new();
        let mut rng = Rng::new(20261121);
        for _ in 0..3000 {
            let case = gen_case(&mut rng);
            let op = field(&case, "op")
                .and_then(Value::as_str)
                .unwrap()
                .to_string();
            let out = String::from_utf8(run(&case).stdout).unwrap();
            let v = json::loads(&out).unwrap();
            let o = v.as_object().unwrap();
            let key = match op.as_str() {
                "inspect" => {
                    let root = match o.get("root").and_then(Value::as_str) {
                        None => "none",
                        Some(r) if r.contains("cache") => "cache",
                        Some(r) if r.contains("market") => "live",
                        Some(r) if r.contains("plugins-dir") || r.contains("dev") => "sibling",
                        Some(_) => "other",
                    };
                    let problem = o.get("problem").and_then(Value::as_str).unwrap_or("ok");
                    format!("inspect {root} {problem}")
                }
                "disabled" => format!("disabled {}", text(o.get("disabled").unwrap())),
                "subagent_model" => format!("model {}", text(o.get("source").unwrap())),
                "agent_memory" => format!(
                    "memory bad={} good={}",
                    !o.get("bad").unwrap().as_array().unwrap().is_empty(),
                    !o.get("good").unwrap().as_array().unwrap().is_empty()
                ),
                "wrappers" => format!(
                    "wrappers bad={}",
                    !o.get("bad").unwrap().as_array().unwrap().is_empty()
                ),
                "repo_top" => format!("repo_top found={}", !o.get("top").unwrap().is_null()),
                "read_json" => format!("read_json found={}", !o.get("value").unwrap().is_null()),
                other => other.to_string(),
            };
            *seen.entry(key).or_default() += 1;
        }
        for want in [
            "inspect live ok",
            "inspect live disabled",
            "inspect cache ok",
            "inspect cache disabled",
            "inspect sibling ok",
            "inspect sibling disabled",
            "inspect none live_missing",
            "inspect none disabled",
            "inspect none not_installed",
            "inspect none not_found",
            "disabled true",
            "disabled false",
            "model null",
            "model \"the process environment\"",
            "model \"user settings.json\"",
            "model \"the project's .claude/settings.json\"",
            "model \"the project's .claude/settings.local.json\"",
            "memory bad=true good=false",
            "memory bad=false good=true",
            "memory bad=false good=false",
            "wrappers bad=true",
            "wrappers bad=false",
            "repo_top found=true",
            "repo_top found=false",
            "read_json found=true",
            "read_json found=false",
            "decode",
        ] {
            assert!(seen.get(want).is_some_and(|&n| n >= 3), "{want}: {seen:?}");
        }
    }
}
