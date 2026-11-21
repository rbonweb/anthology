//! The `pycompat.fnmatch` difftest surface (P1's oracle `port/difftest/oracle/pycompat.py`).
//!
//! `fnmatch.fnmatchcase(name, pat)` against `pycompat::fnmatch::fnmatchcase`. The guard
//! matches protected paths against patterns taken from the command (`candidate`), so seeds hold
//! its shapes (`~/.ssh/*`, browser package globs, bracket edge cases such as `[z-a]`, `[]]`,
//! `[!]`, `[a-]`, unterminated `[`), and generated patterns mix `*`, `?`, every bracket form,
//! `\`, set-operation characters and non-ASCII text, with names built from the pattern.

use anthology_pycompat::fnmatch;
use anthology_pycompat::json::Value;

use super::pycompat_str::{field, gen_cases, obj, ok, python, s};
use crate::case::{Case, Rng, Surface};
use crate::observe::Observed;
use crate::oracle::Oracle;

/// The surface name (plan §1.4).
pub const NAME: &str = "pycompat.fnmatch";

/// Whether `difftest --all` runs this surface on macOS too (`ci/jobs/difftest-macos.sh`).
pub const ON_MACOS: bool = true;

/// The registered implementation.
pub fn surface() -> Option<Box<dyn Surface>> {
    Some(Box::new(PycompatFnmatch))
}

struct PycompatFnmatch;

fn case(name: &str, pat: &str) -> Value {
    obj(vec![
        ("op", s("fnmatch.case")),
        ("name", s(name)),
        ("pat", s(pat)),
    ])
}

const SEEDS: &[(&str, &str)] = &[
    ("/home/user/.ssh/id_rsa", "/home/user/.ssh/*"),
    ("/home/user/.ssh", "/home/user/.ss?"),
    ("/home/user/.ssh-beta", "/home/user/.ssh*"),
    ("/home/user/.aws/credentials", "/home/user/.[a-z]ws/*"),
    ("google-chrome-stable", "google-chrome*"),
    ("chromium-browser", "chromium*"),
    ("playwright", "playwright*"),
    ("ungoogled-chromium", "ungoogled-chromium"),
    ("x", "[z-a]"),
    ("a", "[z-a]"),
    ("]", "[]]"),
    ("a", "[]a]"),
    ("!", "[!]"),
    ("a", "[!]"),
    ("-", "[a-]"),
    ("-", "[-a]"),
    ("[", "["),
    ("[a", "[a"),
    ("b", "[!a-c]"),
    ("d", "[!a-c]"),
    ("\\", "[\\]"),
    ("\\", "\\"),
    ("a", "\\a"),
    ("&", "[&&]"),
    ("~", "[a~~]"),
    ("|", "[||]"),
    ("^", "[^]"),
    ("b", "[^a]"),
    ("-", "[a-c-e]"),
    ("d", "[a-c-e]"),
    ("b", "[c-a-b]"),
    ("e", "[a-b-c-a-e]"),
    ("a\nb", "a?b"),
    ("a\nb", "a*"),
    ("", "*"),
    ("", ""),
    ("abc", "a*c*"),
    ("aXbXc", "*X*X*"),
    ("é.txt", "?.txt"),
    ("😀", "[😀-😂]"),
    ("A", "a"),
    ("ab", "[a][b]"),
    ("a.b", "a.b"),
    ("axb", "a.b"),
    ("a(b)", "a(b)"),
    ("a+b", "a+b"),
    ("a{1}", "a{1}"),
    ("a$", "a$"),
    ("[!", "[!"),
    ("x]", "[!]]"),
    ("a-b", "a[--]b"),
    ("a", "[a-a]"),
    ("b", "[c-a]"),
];

const PIECES: &[&str] = &[
    "*", "*", "?", "a", "b", "z", ".", "/", "-", "~", "é", "😀", "[a-c]", "[!a-c]", "[]a]", "[a-]",
    "[z-a]", "[!]", "[", "]", "\\", "[\\]]", "[&&]", "[--]", "[^a]", "[a-c-e]", "[!b-]", "{", "}",
    "(", "|", "$", "^", "+", "\n",
];

fn instantiate(rng: &mut Rng, pat: &str) -> String {
    let mut out = String::new();
    for c in pat.chars() {
        match c {
            '*' => {
                for _ in 0..rng.index(3) {
                    out.push(*rng.pick(&['a', 'b', '/', '.', 'x']));
                }
            }
            '?' => out.push(*rng.pick(&['a', '\n', '/', 'é'])),
            '[' | ']' | '!' if rng.chance(0.5) => out.push(*rng.pick(&['a', 'b', 'c', '-', ']'])),
            c => out.push(c),
        }
    }
    if rng.chance(0.2) && !out.is_empty() {
        out.pop();
    }
    if rng.chance(0.2) {
        out.push(*rng.pick(&['a', '-', ']', 'z']));
    }
    out
}

impl Surface for PycompatFnmatch {
    fn name(&self) -> &'static str {
        NAME
    }

    fn seeds(&self) -> Vec<Case> {
        SEEDS
            .iter()
            .enumerate()
            .map(|(i, (n, p))| Case::new(format!("seed:{}", i + 1), case(n, p)))
            .collect()
    }

    fn generate(&self, rng: &mut Rng, n: usize) -> Vec<Case> {
        gen_cases(n, || {
            let k = 1 + rng.index(6);
            let pat: String = (0..k).map(|_| *rng.pick(PIECES)).collect();
            let name = if rng.chance(0.15) {
                rng.string(6)
            } else {
                instantiate(rng, &pat)
            };
            case(&name, &pat)
        })
    }

    fn run_python(&self, c: &Case, oracle: &mut Oracle) -> Observed {
        python(c, oracle)
    }

    fn run_rust(&self, c: &Case) -> Observed {
        let m = fnmatch::fnmatchcase(field(&c.data, "name"), field(&c.data, "pat"));
        ok(&Value::Bool(m))
    }
}
