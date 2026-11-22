//! The `pycompat.shlex` difftest surface (P1's oracle `port/difftest/oracle/pycompat.py`).
//!
//! `shlex.split(s)` (the list, or the `ValueError` text) and `shlex.quote(s)` against
//! `pycompat::shlex`. Seeds are the smoke inputs (the SessionStart hook commands of every
//! plugin's `hooks.json` and the test's), and shell-ish strings with every quoting form;
//! generated strings mix quotes, backslashes, whitespace kinds and words.

use anthology_pycompat::json::Value;
use anthology_pycompat::shlex;

use super::pycompat_str::{field, gen_cases, obj, ok, pool_string, python, s, strs};
use crate::case::{Case, Rng, Surface};
use crate::observe::Observed;
use crate::oracle::Oracle;

/// The surface name (plan §1.4).
pub const NAME: &str = "pycompat.shlex";

/// Whether `difftest --all` runs this surface on macOS too (`ci/jobs/difftest-macos.sh`).
pub const ON_MACOS: bool = true;

/// The registered implementation.
pub fn surface() -> Option<Box<dyn Surface>> {
    Some(Box::new(PycompatShlex))
}

struct PycompatShlex;

const SEEDS: &[&str] = &[
    // The smoke inputs: hooks.json commands (plugins/*/hooks/hooks.json, tests/test_smoke.py).
    "\"${CLAUDE_PLUGIN_ROOT}/hooks/run-hook.cmd\" session-start",
    "python3",
    "sh",
    "${CLAUDE_PLUGIN_ROOT}/scripts/other.sh --quick",
    "sh ${CLAUDE_PLUGIN_ROOT}/scripts/session-start.sh",
    // Quoting forms.
    "a 'b c' \"d\\\"e\" f\\ g",
    "''",
    "\"\"",
    "a\"\"b",
    "\"a\\b\"",
    "\"a\\\\b\"",
    "'a\\b'",
    "a\\",
    "'unterminated",
    "\"unterminated",
    "\"a\\",
    "x # not a comment",
    "tab\tsep\nnew\rline",
    "vt\x0bff\x0c",
    "é 'ü' \"😀\"",
    "a\\\nb",
    "\"$HOME\" '$HOME' $HOME",
    "",
    "   ",
    "a'b'c\"d\"e",
];

const QUOTES: &[&str] = &[
    "",
    "a",
    "a-b/c.d",
    "it's",
    "é",
    "a b",
    "$x",
    "@%+=:,./-",
    "a\nb",
    "''",
    "\"",
    "-",
];

const POOL: &[char] = &[
    'a', 'b', ' ', ' ', '\t', '\n', '\r', '\'', '\'', '"', '"', '\\', '\\', '$', '#', 'é', '\x0b',
    '-', '/', '=',
];

impl Surface for PycompatShlex {
    fn name(&self) -> &'static str {
        NAME
    }

    fn seeds(&self) -> Vec<Case> {
        let mut out = Vec::new();
        for (i, v) in SEEDS.iter().enumerate() {
            out.push(Case::new(
                format!("split:{}", i + 1),
                obj(vec![("op", s("shlex.split")), ("s", s(v))]),
            ));
        }
        for (i, v) in QUOTES.iter().enumerate() {
            out.push(Case::new(
                format!("quote:{}", i + 1),
                obj(vec![("op", s("shlex.quote")), ("s", s(v))]),
            ));
        }
        out
    }

    fn generate(&self, rng: &mut Rng, n: usize) -> Vec<Case> {
        gen_cases(n, || {
            let v = pool_string(rng, POOL, 12);
            let op = if rng.chance(0.75) {
                "shlex.split"
            } else {
                "shlex.quote"
            };
            obj(vec![("op", s(op)), ("s", s(&v))])
        })
    }

    fn run_python(&self, c: &Case, oracle: &mut Oracle) -> Observed {
        python(c, oracle)
    }

    fn run_rust(&self, c: &Case) -> Observed {
        let v = field(&c.data, "s");
        match field(&c.data, "op") {
            "shlex.split" => ok(&match shlex::split(v) {
                Ok(words) => Value::Array(vec![s("ok"), strs(&words)]),
                Err(e) => Value::Array(vec![s("error"), s(&e)]),
            }),
            "shlex.quote" => ok(&s(&shlex::quote(v))),
            op => panic!("pycompat.shlex: unknown op {op:?}"),
        }
    }
}
