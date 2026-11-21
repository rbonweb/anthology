//! The `pycompat.quote` difftest surface (P1's oracle `port/difftest/oracle/pycompat.py`).
//!
//! `urllib.parse.quote(s, safe=...)` and `urllib.parse.unquote(s)` against `pycompat::quote`:
//! every ASCII character and a spread of non-ASCII ones, several `safe` sets (`/`, empty, with
//! non-ASCII characters), and `%` escapes that are valid, half valid, invalid UTF-8, split by
//! non-ASCII text, upper and lower case.

use anthology_pycompat::json::Value;
use anthology_pycompat::quote;

use super::pycompat_str::{field, gen_cases, obj, ok, pool_string, python, s};
use crate::case::{Case, Rng, Surface};
use crate::observe::Observed;
use crate::oracle::Oracle;

/// The surface name (plan §1.4).
pub const NAME: &str = "pycompat.quote";

/// Whether `difftest --all` runs this surface on macOS too (`ci/jobs/difftest-macos.sh`).
pub const ON_MACOS: bool = true;

/// The registered implementation.
pub fn surface() -> Option<Box<dyn Surface>> {
    Some(Box::new(PycompatQuote))
}

struct PycompatQuote;

fn quote_case(v: &str, safe: &str) -> Value {
    obj(vec![
        ("op", s("quote.quote")),
        ("s", s(v)),
        ("safe", s(safe)),
    ])
}

fn unquote_case(v: &str) -> Value {
    obj(vec![("op", s("quote.unquote")), ("s", s(v))])
}

const SAFES: &[&str] = &["/", "", ":/@", "~", "é/", "%", "!*'();:@&=+$,/?#[]"];

const UNQUOTES: &[&str] = &[
    "",
    "plain",
    "a%20b",
    "%2F%2f",
    "%zz%",
    "%",
    "100%",
    "%4",
    "%41%",
    "%C3%A9",
    "%c3%a9",
    "%C3",
    "%C3x",
    "%E2%82",
    "%E2%82%AC",
    "%F0%9F%98%80",
    "%FF%FE",
    "é%41",
    "%C3é%A9",
    "%%41",
    "%ED%A0%80",
    "%C0%80",
    "%41%42%43",
    "a+b",
    "%2B",
];

const POOL: &[char] = &[
    '%', '%', '%', '0', '9', 'a', 'f', 'A', 'F', 'g', 'C', '3', 'E', '2', '8', 'B', 'D', ' ', '/',
    '+', 'é', '😀', '~', '_', '.', '-', '\0',
];

impl Surface for PycompatQuote {
    fn name(&self) -> &'static str {
        NAME
    }

    fn seeds(&self) -> Vec<Case> {
        let mut out = Vec::new();
        let all_ascii: String = (0u8..0x80).map(char::from).collect();
        let mixed = format!("{all_ascii}é€😀\u{a0}\u{ffff}\u{10ffff}");
        for (i, safe) in SAFES.iter().enumerate() {
            out.push(Case::new(
                format!("quote:{}", i + 1),
                quote_case(&mixed, safe),
            ));
        }
        out.push(Case::new("quote:empty", quote_case("", "/")));
        out.push(Case::new("quote:file", quote_case("notes/a b#1.md", "/")));
        for (i, v) in UNQUOTES.iter().enumerate() {
            out.push(Case::new(format!("unquote:{}", i + 1), unquote_case(v)));
        }
        out
    }

    fn generate(&self, rng: &mut Rng, n: usize) -> Vec<Case> {
        gen_cases(n, || {
            let v = pool_string(rng, POOL, 14);
            if rng.chance(0.5) {
                unquote_case(&v)
            } else {
                let safe = if rng.chance(0.3) {
                    rng.ascii_string(4)
                } else {
                    (*rng.pick(SAFES)).to_string()
                };
                quote_case(&v, &safe)
            }
        })
    }

    fn run_python(&self, c: &Case, oracle: &mut Oracle) -> Observed {
        python(c, oracle)
    }

    fn run_rust(&self, c: &Case) -> Observed {
        let v = field(&c.data, "s");
        match field(&c.data, "op") {
            "quote.quote" => ok(&s(&quote::quote(v, field(&c.data, "safe")))),
            "quote.unquote" => ok(&s(&quote::unquote(v))),
            op => panic!("pycompat.quote: unknown op {op:?}"),
        }
    }
}
