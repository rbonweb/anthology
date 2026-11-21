//! The `pycompat.pyerr` difftest surface (P1's oracle `port/difftest/oracle/pycompat.py`).
//!
//! Compares `type(e).__name__ + ": " + str(e)` for `OSError(n, os.strerror(n)[, filename])`
//! with `PyExc::OSError`'s `type_name()` and `py_str()`, as full text (I-10): every errno from 0
//! to a few past the C library's last one (133 on glibc Linux, the macOS runner's on macOS),
//! with and without filenames that need Python's `repr`, and `str(KeyError(k))`. The table the
//! Rust side reads follows the OS the difftest runs on, as the oracle's `os.strerror` does.

use anthology_pycompat::json::Value;
use anthology_pycompat::pyerr::PyExc;

use super::pycompat_str::{field, field_i64, field_opt, gen_cases, int, obj, ok, python, s};
use crate::case::{Case, Rng, Surface};
use crate::observe::Observed;
use crate::oracle::Oracle;

/// The surface name (plan §1.4).
pub const NAME: &str = "pycompat.pyerr";

/// Whether `difftest --all` runs this surface on macOS too (`ci/jobs/difftest-macos.sh`).
pub const ON_MACOS: bool = true;

/// The registered implementation.
pub fn surface() -> Option<Box<dyn Surface>> {
    Some(Box::new(PycompatPyerr))
}

struct PycompatPyerr;

/// The last errno the C library names: glibc's EHWPOISON, macOS's ENOTCAPABLE. The plan's floor
/// is 133 and 106 (I-10); the seeds run a few past it, into the unknown form.
const LAST: i64 = if cfg!(target_os = "macos") { 107 } else { 133 };

const NAMES: &[&str] = &[
    "/tmp/x",
    "it's",
    "a\"b",
    "both'\"",
    "caf\u{e9}\u{200b}\n",
    "\u{1f600}\u{e0001}\u{7f}",
    "\\path\twith\rcontrols",
    "",
];

fn oserror(errno: i64, filename: Option<&str>) -> Value {
    obj(vec![
        ("op", s("pyerr.oserror")),
        ("errno", int(errno)),
        ("filename", filename.map_or(Value::Null, s)),
    ])
}

impl Surface for PycompatPyerr {
    fn name(&self) -> &'static str {
        NAME
    }

    fn seeds(&self) -> Vec<Case> {
        let mut out = Vec::new();
        for n in (0..=LAST + 3).chain([-1, 255, 4096, 10000, i64::from(i32::MAX)]) {
            out.push(Case::new(format!("errno:{n}"), oserror(n, None)));
            out.push(Case::new(
                format!("errno-file:{n}"),
                oserror(n, Some(NAMES[(n.unsigned_abs() as usize) % NAMES.len()])),
            ));
        }
        for (i, name) in NAMES.iter().enumerate() {
            out.push(Case::new(format!("name:{}", i + 1), oserror(2, Some(name))));
            out.push(Case::new(
                format!("key:{}", i + 1),
                obj(vec![("op", s("pyerr.keyerror")), ("key", s(name))]),
            ));
        }
        out
    }

    fn generate(&self, rng: &mut Rng, n: usize) -> Vec<Case> {
        gen_cases(n, || {
            let name = if rng.chance(0.2) {
                None
            } else {
                Some(rng.string(10))
            };
            if rng.chance(0.15) {
                obj(vec![
                    ("op", s("pyerr.keyerror")),
                    ("key", s(&name.unwrap_or_default())),
                ])
            } else {
                oserror(rng.range(0, LAST + 5), name.as_deref())
            }
        })
    }

    fn run_python(&self, c: &Case, oracle: &mut Oracle) -> Observed {
        python(c, oracle)
    }

    fn run_rust(&self, c: &Case) -> Observed {
        let e = match field(&c.data, "op") {
            "pyerr.oserror" => PyExc::OSError {
                errno: field_i64(&c.data, "errno") as i32,
                filename: field_opt(&c.data, "filename")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            },
            "pyerr.keyerror" => {
                return ok(&s(&PyExc::KeyError(field(&c.data, "key").into()).py_str()));
            }
            op => panic!("pycompat.pyerr: unknown op {op:?}"),
        };
        ok(&s(&format!("{}: {}", e.type_name(), e.py_str())))
    }
}
