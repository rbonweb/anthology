//! The `pycompat.path` difftest surface (P1's oracle `port/difftest/oracle/pycompat.py`).
//!
//! - `posixpath.normpath(p)` and `posixpath.join(a, b)` against `pycompat::path` on path-ish
//!   strings (slashes in runs, `.`, `..`, empty parts, NUL, non-ASCII).
//! - `os.path.realpath(<root>/p)` against `pycompat::path::realpath`, each side in its own
//!   fresh temporary folder holding the same tree of folders, files and symbolic links
//!   (absolute and relative targets, chains, loops, dangling links, `..` after a link, missing
//!   parts). The root sits six folders down a fresh temporary folder whose realpath is written
//!   `<TMP>` (on macOS that covers `/var` → `/private/var`), so a path leading out of the root
//!   reads the same everywhere.

use std::path::Path;

use anthology_pycompat::json::Value;
use anthology_pycompat::path;

use super::pycompat_str::{field, gen_cases, obj, ok, pool_string, python, s};
use crate::case::{Case, Rng, Surface};
use crate::observe::Observed;
use crate::oracle::Oracle;

/// The surface name (plan §1.4).
pub const NAME: &str = "pycompat.path";

/// Whether `difftest --all` runs this surface on macOS too (`ci/jobs/difftest-macos.sh`).
pub const ON_MACOS: bool = true;

/// The registered implementation.
pub fn surface() -> Option<Box<dyn Surface>> {
    Some(Box::new(PycompatPath))
}

struct PycompatPath;

const NORM: &[&str] = &[
    "",
    "/",
    "//",
    "///",
    "////a",
    "//a/../b",
    "a/../..",
    "/../a",
    "./a/./b/",
    "a//b",
    "../../a/..",
    "a/b/../../..",
    "/a/b/../../..",
    ".",
    "..",
    "./",
    "a/.",
    "a/..",
    "//..",
    "/./",
    "a\0b/../c",
    "é/ü/../ö",
    "~/x/../y",
    "a/./../b/./c/..",
    "//./a",
];

const JOINS: &[(&str, &str)] = &[
    ("a", "b"),
    ("a/", "b"),
    ("", "b"),
    ("a", "/b"),
    ("a", ""),
    ("", ""),
    ("/", "b"),
    ("//", "b"),
    ("a//", "b"),
    ("a", "b/"),
];

type Tree = &'static [(&'static str, &'static str, &'static str)];

/// (tree, paths to resolve).
const TREES: &[(Tree, &[&str])] = &[
    (
        &[
            ("d", "dir", ""),
            ("d/f", "file", ""),
            ("l", "link", "d"),
            ("abs", "link", "/"),
            ("rel", "link", "../x"),
        ],
        &[
            "l/f",
            "l/../d",
            "l/missing/x",
            "d/f/..",
            "rel",
            "abs/..",
            "abs",
            "nope/../l",
            ".",
        ],
    ),
    (
        &[("a", "link", "b"), ("b", "link", "a"), ("c", "link", "c")],
        &["a", "a/x/y", "b/..", "c", "c/d"],
    ),
    (
        &[
            ("d/e", "dir", ""),
            ("chain1", "link", "chain2"),
            ("chain2", "link", "d/e"),
            ("d/e/up", "link", ".."),
            ("d/e/dot", "link", "."),
            ("dang", "link", "nowhere/x"),
        ],
        &[
            "chain1",
            "chain1/up/e/up",
            "d/e/dot/dot/up",
            "dang",
            "dang/..",
            "chain1/../d",
        ],
    ),
    (
        &[
            ("deep/a/b/c", "dir", ""),
            ("deep/l", "link", "a/b/c/../../.."),
            ("x", "link", "deep//l/"),
        ],
        &["deep/l", "x/a", "x/../deep/a/b/c/../c", "deep/./a/../a/b"],
    ),
];

fn tree_value(tree: Tree) -> Value {
    Value::Array(
        tree.iter()
            .map(|(p, k, t)| Value::Array(vec![s(p), s(k), s(t)]))
            .collect(),
    )
}

fn realpath_case(tree: Tree, p: &str) -> Value {
    obj(vec![
        ("op", s("path.realpath")),
        ("tree", tree_value(tree)),
        ("p", s(p)),
    ])
}

const POOL: &[char] = &['/', '/', '/', '.', '.', '.', 'a', 'b', 'é', '~', '\0', ' '];

impl Surface for PycompatPath {
    fn name(&self) -> &'static str {
        NAME
    }

    fn seeds(&self) -> Vec<Case> {
        let mut out = Vec::new();
        for (i, p) in NORM.iter().enumerate() {
            out.push(Case::new(
                format!("normpath:{}", i + 1),
                obj(vec![("op", s("path.normpath")), ("p", s(p))]),
            ));
        }
        for (i, (a, b)) in JOINS.iter().enumerate() {
            out.push(Case::new(
                format!("join:{}", i + 1),
                obj(vec![("op", s("path.join")), ("a", s(a)), ("b", s(b))]),
            ));
        }
        let mut k = 0;
        for (tree, paths) in TREES {
            for p in *paths {
                k += 1;
                out.push(Case::new(format!("realpath:{k}"), realpath_case(tree, p)));
            }
        }
        out
    }

    fn generate(&self, rng: &mut Rng, n: usize) -> Vec<Case> {
        gen_cases(n, || {
            let k = rng.f64();
            if k < 0.6 {
                obj(vec![
                    ("op", s("path.normpath")),
                    ("p", s(&pool_string(rng, POOL, 14))),
                ])
            } else if k < 0.8 {
                obj(vec![
                    ("op", s("path.join")),
                    ("a", s(&pool_string(rng, POOL, 6))),
                    ("b", s(&pool_string(rng, POOL, 6))),
                ])
            } else {
                let (tree, paths) = rng.pick(TREES);
                let parts: Vec<&str> = (0..1 + rng.index(4))
                    .map(|_| {
                        if rng.chance(0.5) {
                            *rng.pick(paths)
                        } else {
                            *rng.pick(&["..", ".", "", "a", "d", "l", "missing", "chain1"])
                        }
                    })
                    .collect();
                realpath_case(tree, &parts.join("/"))
            }
        })
    }

    fn run_python(&self, c: &Case, oracle: &mut Oracle) -> Observed {
        python(c, oracle)
    }

    fn run_rust(&self, c: &Case) -> Observed {
        match field(&c.data, "op") {
            "path.normpath" => ok(&s(&path::normpath(field(&c.data, "p")))),
            "path.join" => ok(&s(&path::join(field(&c.data, "a"), field(&c.data, "b")))),
            "path.realpath" => ok(&s(&run_realpath(&c.data))),
            op => panic!("pycompat.path: unknown op {op:?}"),
        }
    }
}

fn run_realpath(case: &Value) -> String {
    let dir = tempfile::tempdir().expect("a temporary folder");
    let tmp = std::fs::canonicalize(dir.path()).expect("the folder's realpath");
    let root = tmp.join("n1/n2/n3/n4/n5/n6/root");
    std::fs::create_dir_all(&root).expect("mkdir");
    let tree = case
        .as_object()
        .and_then(|o| o.get("tree"))
        .and_then(Value::as_array)
        .expect("tree");
    for item in tree {
        let item = item.as_array().expect("a tree entry");
        let get = |i: usize| item[i].as_str().expect("a string");
        let p = root.join(get(0));
        match get(1) {
            "dir" => std::fs::create_dir_all(&p).expect("mkdir"),
            "file" => {
                std::fs::create_dir_all(p.parent().unwrap_or(Path::new("/"))).expect("mkdir");
                std::fs::write(&p, b"").expect("write");
            }
            _ => {
                std::fs::create_dir_all(p.parent().unwrap_or(Path::new("/"))).expect("mkdir");
                std::os::unix::fs::symlink(get(2), &p).expect("symlink");
            }
        }
    }
    let joined = path::join(&root.to_string_lossy(), field(case, "p"));
    let out = path::realpath(Path::new(&joined))
        .to_string_lossy()
        .into_owned();
    let t = tmp.to_string_lossy().into_owned();
    if out == t || out.starts_with(&format!("{t}/")) {
        format!("<TMP>{}", &out[t.len()..])
    } else {
        out
    }
}
