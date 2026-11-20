//! The `pycompat.str` difftest surface (P1's oracle `port/difftest/oracle/pycompat.py`).
//!
//! Seeds: every code point, in 4,096-code-point chunks (`str.table`: each predicate, the
//! category, the decimal value, `lower()`, and three Final_Sigma probes per character), and
//! fixed strings for `lower` (multi-character Final_Sigma cases such as `ΑΣ`, `ΑΣ Α`, `Σ`),
//! `split`, `strip`, `repr`, `len`, slices, `int()` and `float()`. Generated cases mix Greek
//! capitals, sigmas, case-ignorable and cased characters, every kind of whitespace and
//! number-like text.
//!
//! This module also holds the helpers the other `pycompat.*` surfaces of P1 share.

use anthology_pycompat::json::{self, Int, Map, Value};
use anthology_pycompat::str as pystr;

use crate::case::{Case, Rng, Surface};
use crate::observe::{Observed, from_oracle};
use crate::oracle::Oracle;

/// The surface name (plan §1.4).
pub const NAME: &str = "pycompat.str";

/// Whether `difftest --all` runs this surface on macOS too (`ci/jobs/difftest-macos.sh`).
pub const ON_MACOS: bool = true;

/// The registered implementation.
pub fn surface() -> Option<Box<dyn Surface>> {
    Some(Box::new(PycompatStr))
}

struct PycompatStr;

// ---------------------------------------------------------------------------------------------
// Shared by P1's surfaces.

/// The oracle module of every `pycompat.*` surface but json.
pub const ORACLE: &str = "pycompat";

/// An object from `(key, value)` pairs.
pub fn obj(pairs: Vec<(&str, Value)>) -> Value {
    let mut m = Map::new();
    for (k, v) in pairs {
        m.insert(k.to_string(), v);
    }
    Value::Object(m)
}

/// A string value.
pub fn s(v: &str) -> Value {
    Value::Str(v.to_string())
}

/// An int value.
pub fn int(n: i64) -> Value {
    Value::Int(Int::Small(n))
}

/// A list of strings.
pub fn strs<S: AsRef<str>>(v: &[S]) -> Value {
    Value::Array(v.iter().map(|x| s(x.as_ref())).collect())
}

/// A case field as text.
pub fn field<'a>(case: &'a Value, k: &str) -> &'a str {
    case.as_object()
        .and_then(|o| o.get(k))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("pycompat: the case has no string {k:?}"))
}

/// A case field as an int.
pub fn field_i64(case: &Value, k: &str) -> i64 {
    case.as_object()
        .and_then(|o| o.get(k))
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("pycompat: the case has no int {k:?}"))
}

/// A case field, if present and not null.
pub fn field_opt<'a>(case: &'a Value, k: &str) -> Option<&'a Value> {
    case.as_object()
        .and_then(|o| o.get(k))
        .filter(|v| !v.is_null())
}

/// What the oracle answers for a result: exit 0 and `json.dumps(result, sort_keys=True)`.
pub fn ok(result: &Value) -> Observed {
    Observed::text(0, &json::dumps(result, &json::PY_DEFAULT_SORTED), "")
}

/// What the oracle answers when Python raises: exit 1 and the exception's type name.
pub fn raised(type_name: &str) -> Observed {
    Observed::text(1, "", type_name)
}

/// The Python side of a case.
pub fn python(c: &Case, oracle: &mut Oracle) -> Observed {
    from_oracle(&oracle.call(ORACLE, &c.data))
}

/// Sorted inclusive runs `[[lo, hi], ...]` of the code points in `lo..hi` (no surrogates) that
/// `pred` accepts.
pub fn runs(lo: u32, hi: u32, mut pred: impl FnMut(char) -> bool) -> Value {
    let mut out: Vec<(u32, u32)> = Vec::new();
    for cp in lo..hi {
        let Some(c) = char::from_u32(cp) else {
            continue;
        };
        if pred(c) {
            match out.last_mut() {
                Some(last) if last.1 + 1 == cp => last.1 = cp,
                _ => out.push((cp, cp)),
            }
        }
    }
    Value::Array(
        out.into_iter()
            .map(|(a, b)| Value::Array(vec![int(a.into()), int(b.into())]))
            .collect(),
    )
}

/// Case ids for a list of seed values: `<prefix>:<n>`.
pub fn seed_cases(prefix: &str, data: Vec<Value>) -> Vec<Case> {
    data.into_iter()
        .enumerate()
        .map(|(i, d)| Case::new(format!("{prefix}:{}", i + 1), d))
        .collect()
}

/// Generated case ids: `gen:<n>`.
pub fn gen_cases(n: usize, mut make: impl FnMut() -> Value) -> Vec<Case> {
    (0..n)
        .map(|i| Case::new(format!("gen:{}", i + 1), make()))
        .collect()
}

/// A random string of up to `max` characters drawn from `pool` (and now and then any
/// character).
pub fn pool_string(rng: &mut Rng, pool: &[char], max: usize) -> String {
    let n = rng.index(max + 1);
    (0..n)
        .map(|_| {
            if rng.chance(0.1) {
                rng.unicode_char()
            } else {
                *rng.pick(pool)
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------

const CHUNK: u32 = 0x1000;

const LOWER_SEEDS: &[&str] = &[
    "ΑΣ",
    "ΑΣ Α",
    "Σ",
    "ΑΣ'",
    "Α'Σ",
    "ΑΣ'Α",
    "Σ\u{308}",
    "Α\u{308}Σ",
    "ΣΑ",
    "ΑΣΣ",
    "ΑΣ.",
    "1Σ",
    "\u{2b0}Σ",
    "AΣ\u{2b0}",
    "ΑΣ\u{345}",
    "ǅΣ",
    "ΟΔΥΣΣΕΥΣ",
    "ΣΑΣ ΣΑΣ.",
    "Α.Σ.",
    "İSTANBUL",
    "Abİ",
    "\u{1e9e}",
    "ﬁ",
    "ꭰ",
    "",
    "ABC def",
];

const WS_SEEDS: &[&str] = &[
    " a  b\x1c c d  ",
    "\x1f\x1e x \u{85}",
    "",
    "   ",
    "\u{a0}a\u{2028}b\u{3000}",
    "\u{200b}a\u{200b}",
    "a\u{180e}b",
    "\t\n\x0b\x0c\r",
    "x\u{feff}y",
];

const REPR_SEEDS: &[&str] = &[
    "it's",
    "a\"b",
    "both'\"",
    "\0\x7f\u{80}\u{a0}\u{ad}",
    "é\u{301}\u{200b}😀\u{e0001}\u{d7ff}",
    "\\\t\n\r",
    "\u{2028}\u{fffe}\u{ffff}\u{10ffff}",
    "plain",
];

const NUMBER_SEEDS: &[&str] = &[
    "12",
    " -0 ",
    "+٣٤",
    "1_000",
    "1__0",
    "_1",
    "1_",
    "\x1c1",
    " 5 ",
    "00",
    "-",
    "",
    "١_٢",
    "99999999999999999999",
    "-9223372036854775808",
    "²",
    "1 2",
    "0x10",
    "\u{a0}7\u{3000}",
    "1.5",
    " 1. ",
    ".5",
    ".",
    "1e5",
    "1e",
    "1E+05",
    "inf",
    "-Infinity",
    "nAn",
    "+nan",
    "1_000.000_1",
    "1_.5",
    "1._5",
    "1e1_0",
    "١.٥",
    " 1.5\u{3000}",
    "0x1p3",
    "1e400",
    "-0",
    "5e-324",
    "in_f",
    "1.5.3",
    "e5",
    ".e5",
    "1_e5",
    "infinityx",
    "  ",
    "1.e5",
    "-.5e-3",
    "𝟏𝟐",
    "١٢٣٤٥٦٧٨٩٠",
];

impl Surface for PycompatStr {
    fn name(&self) -> &'static str {
        NAME
    }

    fn seeds(&self) -> Vec<Case> {
        let mut out = Vec::new();
        let mut lo = 0;
        while lo < 0x11_0000 {
            out.push(Case::new(
                format!("table:{lo:x}"),
                obj(vec![
                    ("op", s("str.table")),
                    ("lo", int(lo.into())),
                    ("hi", int((lo + CHUNK).into())),
                ]),
            ));
            lo += CHUNK;
        }
        let one = |op: &str, v: &str| obj(vec![("op", s(op)), ("s", s(v))]);
        out.extend(seed_cases(
            "lower",
            LOWER_SEEDS.iter().map(|v| one("str.lower", v)).collect(),
        ));
        let mut ws = Vec::new();
        for v in WS_SEEDS {
            ws.push(one("str.split", v));
            ws.push(one("str.strip", v));
            ws.push(one("str.len", v));
        }
        out.extend(seed_cases("ws", ws));
        out.extend(seed_cases(
            "repr",
            REPR_SEEDS.iter().map(|v| one("str.repr", v)).collect(),
        ));
        let mut nums = Vec::new();
        for v in NUMBER_SEEDS {
            nums.push(one("str.int", v));
            nums.push(one("str.float", v));
        }
        nums.push(one("str.int", &"9".repeat(4300)));
        nums.push(one("str.int", &"9".repeat(4301)));
        nums.push(one("str.int", &format!("-{}", "1_0".repeat(2000))));
        out.extend(seed_cases("num", nums));
        let slices = [
            ("aé😀b", 1, 3),
            ("aé😀b", 2, 99),
            ("abc", 2, 1),
            ("abc", 5, 9),
        ];
        out.extend(seed_cases(
            "slice",
            slices
                .iter()
                .map(|(v, a, b)| {
                    obj(vec![
                        ("op", s("str.slice")),
                        ("s", s(v)),
                        ("a", int(*a)),
                        ("b", int(*b)),
                    ])
                })
                .collect(),
        ));
        out
    }

    fn generate(&self, rng: &mut Rng, n: usize) -> Vec<Case> {
        gen_cases(n, || gen_case(rng))
    }

    fn run_python(&self, c: &Case, oracle: &mut Oracle) -> Observed {
        python(c, oracle)
    }

    fn run_rust(&self, c: &Case) -> Observed {
        run(&c.data)
    }
}

/// Characters that exercise case, Final_Sigma and whitespace.
const CASE_POOL: &[char] = &[
    'Σ', 'Σ', 'Σ', 'σ', 'ς', 'Α', 'Β', 'α', 'A', 'a', 'Z', '\'', '.', ':', '\u{2b0}', '\u{308}',
    '\u{345}', '\u{301}', '\u{200d}', '\u{ad}', 'ǅ', 'İ', 'ẞ', 'Ⅻ', '1', ' ', '\u{a0}', '\u{3000}',
    '\x1c', '\x1f', '\u{85}', '\u{2028}', '\t', '\n', '_', '-', 'Ω', 'Ԁ', 'Ꭰ', 'ꭰ', '𐐀', '𞤀',
];

/// Characters that make number-like text.
const NUMBER_POOL: &[char] = &[
    '0', '1', '5', '9', '_', '_', '.', 'e', 'E', '+', '-', ' ', '\u{a0}', '\u{3000}', '\x1c', '٣',
    '١', '𝟐', '²', 'i', 'n', 'f', 'a', 'x',
];

fn gen_case(rng: &mut Rng) -> Value {
    let k = rng.f64();
    if k < 0.65 {
        let op = *rng.pick(&[
            "str.lower",
            "str.lower",
            "str.lower",
            "str.split",
            "str.strip",
            "str.repr",
            "str.len",
        ]);
        let v = pool_string(rng, CASE_POOL, 12);
        obj(vec![("op", s(op)), ("s", s(&v))])
    } else if k < 0.75 {
        let v = rng.string(8);
        let a = rng.index(10) as i64;
        let b = rng.index(10) as i64;
        obj(vec![
            ("op", s("str.slice")),
            ("s", s(&v)),
            ("a", int(a)),
            ("b", int(b)),
        ])
    } else {
        let op = if rng.chance(0.5) {
            "str.int"
        } else {
            "str.float"
        };
        let mut v = pool_string(rng, NUMBER_POOL, 8);
        if rng.chance(0.3) {
            v = format!(
                "{}{}{}",
                rng.pick(&["", " ", "-", "+", "\u{2003}"]),
                rng.pick(&["1", "12_3", "0.5", ".5e-2", "inf", "nan", "٣٤"]),
                rng.pick(&["", " ", "\u{85}", "_", "e"])
            );
        }
        obj(vec![("op", s(op)), ("s", s(&v))])
    }
}

/// Python's `repr(float)`.
pub fn float_repr(v: f64) -> String {
    if v.is_nan() {
        "nan".into()
    } else if v.is_infinite() {
        if v > 0.0 { "inf".into() } else { "-inf".into() }
    } else {
        json::dumps(&Value::Float(v), &json::PY_DEFAULT)
    }
}

fn run(case: &Value) -> Observed {
    let text = || field(case, "s");
    match field(case, "op") {
        "str.table" => ok(&table(
            field_i64(case, "lo") as u32,
            field_i64(case, "hi") as u32,
        )),
        "str.lower" => ok(&s(&pystr::lower(text()))),
        "str.split" => ok(&strs(&pystr::split_ws(text()))),
        "str.strip" => ok(&s(pystr::strip_ws(text()))),
        "str.repr" => ok(&s(&pystr::repr(text()))),
        "str.len" => ok(&int(pystr::char_len(text()) as i64)),
        "str.slice" => ok(&s(pystr::char_slice(
            text(),
            field_i64(case, "a") as usize,
            field_i64(case, "b") as usize,
        ))),
        "str.int" => match pystr::py_int(text()) {
            Some(n) => ok(&Value::Int(n)),
            None => raised("ValueError"),
        },
        "str.float" => match pystr::py_float(text()) {
            Some(v) => ok(&s(&float_repr(v))),
            None => raised("ValueError"),
        },
        op => panic!("pycompat.str: unknown op {op:?}"),
    }
}

/// A `char` predicate.
type Pred = fn(char) -> bool;

fn table(lo: u32, hi: u32) -> Value {
    let preds: [(&str, Pred); 6] = [
        ("isspace", pystr::is_space),
        ("isdecimal", pystr::is_decimal),
        ("isdigit", pystr::is_digit),
        ("isalpha", pystr::is_alpha),
        ("isalnum", pystr::is_alnum),
        ("isprintable", pystr::is_printable),
    ];
    let mut m = Map::new();
    for (name, pred) in preds {
        m.insert(name.into(), runs(lo, hi, pred));
    }
    let mut cats: Vec<(u32, u32, &str)> = Vec::new();
    let mut dec = Vec::new();
    let mut low = Vec::new();
    for cp in lo..hi {
        let Some(c) = char::from_u32(cp) else {
            continue;
        };
        let cat = pystr::unicode_category(c);
        match cats.last_mut() {
            Some(last) if last.2 == cat && last.1 + 1 == cp => last.1 = cp,
            _ => cats.push((cp, cp, cat)),
        }
        if let Some(d) = pystr::decimal_value(c) {
            dec.push(Value::Array(vec![int(cp.into()), int(d.into())]));
        }
        let l = pystr::lower(&c.to_string());
        if l != c.to_string() {
            low.push(Value::Array(vec![int(cp.into()), s(&l)]));
        }
    }
    m.insert(
        "category".into(),
        Value::Array(
            cats.into_iter()
                .map(|(a, b, c)| Value::Array(vec![int(a.into()), int(b.into()), s(c)]))
                .collect(),
        ),
    );
    m.insert("decimal".into(), Value::Array(dec));
    m.insert("lower".into(), Value::Array(low));
    let last_is_final = |t: String| pystr::lower(&t).ends_with('ς');
    m.insert(
        "sigma_after".into(),
        runs(lo, hi, |c| last_is_final(format!("{c}Σ"))),
    );
    m.insert(
        "sigma_skip".into(),
        runs(lo, hi, |c| last_is_final(format!("A{c}Σ"))),
    );
    m.insert(
        "sigma_before".into(),
        runs(lo, hi, |c| {
            pystr::lower(&format!("AΣ{c}")).chars().nth(1) == Some('ς')
        }),
    );
    Value::Object(m)
}
