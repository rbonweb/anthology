//! The `pycompat.difflib` difftest surface (P1's oracle `port/difftest/oracle/pycompat.py`).
//!
//! `list(difflib.unified_diff(a, b, fromfile, tofile, n=n, lineterm=...))` against
//! `pycompat::difflib`. Seeds and generated cases include sequences of more than 200 lines with
//! repeated ("popular") lines, so `autojunk` matters, and the settings and setup forms
//! (`n=0`, `lineterm=""`; `n=3`, `lineterm="\n"`).

use anthology_pycompat::difflib;
use anthology_pycompat::json::Value;

use super::pycompat_str::{field, field_i64, gen_cases, int, obj, ok, python, s, strs};
use crate::case::{Case, Rng, Surface};
use crate::observe::Observed;
use crate::oracle::Oracle;

/// The surface name (plan §1.4).
pub const NAME: &str = "pycompat.difflib";

/// Whether `difftest --all` runs this surface on macOS too (`ci/jobs/difftest-macos.sh`).
pub const ON_MACOS: bool = true;

/// The registered implementation.
pub fn surface() -> Option<Box<dyn Surface>> {
    Some(Box::new(PycompatDifflib))
}

struct PycompatDifflib;

fn case(a: &[String], b: &[String], n: i64, lineterm: &str) -> Value {
    obj(vec![
        ("op", s("difflib.unified")),
        ("a", strs(a)),
        ("b", strs(b)),
        ("fromfile", s("settings.json")),
        ("tofile", s("b/.claude/memory/x.md")),
        ("n", int(n)),
        ("lineterm", s(lineterm)),
    ])
}

fn lines(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| (*x).to_string()).collect()
}

impl Surface for PycompatDifflib {
    fn name(&self) -> &'static str {
        NAME
    }

    fn seeds(&self) -> Vec<Case> {
        let settings_a = lines(&["{", "  \"a\": 1,", "  \"b\": [", "    2", "  ]", "}"]);
        let settings_b = lines(&[
            "{",
            "  \"a\": 1,",
            "  \"b\": [",
            "    2,",
            "    3",
            "  ]",
            "}",
        ]);
        // 300 lines where "}" and "" are popular (more than 300 // 100 + 1 = 4 times).
        let big_a: Vec<String> = (0..300)
            .map(|i| match i % 10 {
                0 => "}".to_string(),
                5 => String::new(),
                _ => format!("line {i}"),
            })
            .collect();
        let mut big_b = big_a.clone();
        big_b[17] = "changed".into();
        big_b.insert(150, "inserted".into());
        big_b.remove(250);
        let mut out = Vec::new();
        let pairs: Vec<(Vec<String>, Vec<String>)> = vec![
            (settings_a.clone(), settings_b.clone()),
            (vec![], settings_b.clone()),
            (settings_a.clone(), vec![]),
            (vec![], vec![]),
            (settings_a.clone(), settings_a.clone()),
            (big_a.clone(), big_b.clone()),
            (big_b, big_a),
            (lines(&["a", "b", "c"]), lines(&["x", "b", "y"])),
            (lines(&["a\n", "b\n"]), lines(&["a\n", "c\n"])),
        ];
        let mut k = 0;
        for (a, b) in &pairs {
            for (n, lt) in [(0, ""), (3, ""), (3, "\n"), (1, "")] {
                k += 1;
                out.push(Case::new(format!("seed:{k}"), case(a, b, n, lt)));
            }
        }
        out
    }

    fn generate(&self, rng: &mut Rng, n: usize) -> Vec<Case> {
        gen_cases(n, || {
            let alphabet = 2 + rng.index(30);
            let len = |rng: &mut Rng| {
                if rng.chance(0.25) {
                    150 + rng.index(250)
                } else {
                    rng.index(25)
                }
            };
            let la = len(rng);
            let a: Vec<String> = (0..la)
                .map(|_| format!("l{}", rng.index(alphabet)))
                .collect();
            let mut b = a.clone();
            for _ in 0..rng.index(8) {
                let k = rng.index(4);
                let at = if b.is_empty() { 0 } else { rng.index(b.len()) };
                match k {
                    0 if !b.is_empty() => {
                        b.remove(at);
                    }
                    1 => b.insert(at, format!("l{}", rng.index(alphabet + 3))),
                    _ if !b.is_empty() => b[at] = format!("l{}", rng.index(alphabet + 3)),
                    _ => b.push("new".into()),
                }
            }
            if rng.chance(0.1) {
                let lb = len(rng);
                b = (0..lb)
                    .map(|_| format!("l{}", rng.index(alphabet)))
                    .collect();
            }
            let n = *rng.pick(&[0, 0, 1, 3, 5]);
            let lt = *rng.pick(&["", "", "\n"]);
            case(&a, &b, n, lt)
        })
    }

    fn run_python(&self, c: &Case, oracle: &mut Oracle) -> Observed {
        python(c, oracle)
    }

    fn run_rust(&self, c: &Case) -> Observed {
        let list = |k: &str| -> Vec<String> {
            c.data
                .as_object()
                .and_then(|o| o.get(k))
                .and_then(Value::as_array)
                .map(|v| {
                    v.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default()
        };
        let (a, b) = (list("a"), list("b"));
        let a: Vec<&str> = a.iter().map(String::as_str).collect();
        let b: Vec<&str> = b.iter().map(String::as_str).collect();
        let out = difflib::unified_diff_lineterm(
            &a,
            &b,
            field(&c.data, "fromfile"),
            field(&c.data, "tofile"),
            field_i64(&c.data, "n") as usize,
            field(&c.data, "lineterm"),
        );
        ok(&strs(&out))
    }
}
