//! The `pycompat.json` difftest surface (F2's oracle `port/difftest/oracle/json.py`).
//!
//! Cases (the oracle's docstring has the exact shapes):
//! `{"op": "loads_dumps", "text": TEXT, "preset": NAME}`,
//! `{"op": "loads_bytes_lossy" | "loads_utf8_sig", "hex": HEX, "preset": NAME}`.
//! Seeds are the cases of the committed golden corpus. Generated cases cover random documents
//! with every escape kind (unicode and astral, escaped pairs, lone surrogate escapes), NaN and
//! Infinity, big ints (past the 4300-digit limit too), `1e16`, `-0.0`, `5e-324`, random 64-bit
//! doubles, nesting up to depth 10,000, every GC-4 preset, invalid documents (mutations, odd
//! whitespace, errors after astral text) and invalid UTF-8 bytes.
//!
//! The oracle marks D-6 (lone surrogates) and D-1 (depth that makes CPython raise
//! `RecursionError`); both are declared in `port/difftest/divergences/pycompat.json.toml`.

use anthology_pycompat::json::{
    self, DumpOpts, JsonError, Map, PRESETS, Value, loads, loads_bytes_lossy, loads_utf8_sig,
};

use crate::case::{Case, Rng, Surface, golden_seeds};
use crate::observe::{Observed, from_oracle};
use crate::oracle::Oracle;

/// The surface name (plan §1.4).
pub const NAME: &str = "pycompat.json";

/// Whether `difftest --all` runs this surface on macOS too (`ci/jobs/difftest-macos.sh`); set
/// `false` only for a surface that cannot run there, and say why.
pub const ON_MACOS: bool = true;

/// The registered implementation.
pub fn surface() -> Option<Box<dyn Surface>> {
    Some(Box::new(PycompatJson))
}

struct PycompatJson;

const GOLDEN: &str = "crates/anthology-pycompat/tests/golden/json.jsonl";

impl Surface for PycompatJson {
    fn name(&self) -> &'static str {
        NAME
    }

    fn seeds(&self) -> Vec<Case> {
        golden_seeds(GOLDEN)
    }

    fn generate(&self, rng: &mut Rng, n: usize) -> Vec<Case> {
        (0..n)
            .map(|i| Case::new(format!("gen:{}", i + 1), gen_case(rng)))
            .collect()
    }

    fn run_python(&self, c: &Case, oracle: &mut Oracle) -> Observed {
        from_oracle(&oracle.call("json", &c.data))
    }

    fn run_rust(&self, c: &Case) -> Observed {
        run(&c.data)
    }
}

fn text<'a>(case: &'a Value, k: &str) -> &'a str {
    case.as_object()
        .and_then(|o| o.get(k))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("pycompat.json: the case has no {k:?}"))
}

fn preset(name: &str) -> DumpOpts {
    PRESETS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, o)| *o)
        .unwrap_or_else(|| panic!("pycompat.json: unknown preset {name:?}"))
}

/// What Rust prints for a case, in the oracle's form.
fn run(case: &Value) -> Observed {
    let (doc, result): (String, Result<Value, JsonError>) = match text(case, "op") {
        "loads_dumps" => {
            let t = text(case, "text");
            (t.to_string(), loads(t))
        }
        "loads_bytes_lossy" => {
            let b = unhex(text(case, "hex"));
            (
                String::from_utf8_lossy(&b).into_owned(),
                loads_bytes_lossy(&b),
            )
        }
        "loads_utf8_sig" => {
            let b = unhex(text(case, "hex"));
            let doc = std::str::from_utf8(&b)
                .map(|t| t.strip_prefix('\u{feff}').unwrap_or(t).to_string())
                .unwrap_or_default();
            (doc, loads_utf8_sig(&b))
        }
        op => panic!("pycompat.json: unknown op {op:?}"),
    };
    match result {
        Ok(v) => Observed::text(0, &json::dumps(&v, &preset(text(case, "preset"))), ""),
        Err(e) => Observed::text(1, "", &format!("{}: {}", e.type_name(), e.py_str(&doc))),
    }
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

fn case_of(op: &str, key: &str, value: String, preset: &str) -> Value {
    let mut m = Map::new();
    m.insert("op".into(), Value::Str(op.into()));
    m.insert(key.into(), Value::Str(value));
    m.insert("preset".into(), Value::Str(preset.into()));
    Value::Object(m)
}

fn any_preset(rng: &mut Rng) -> &'static str {
    rng.pick(&PRESETS).0
}

const INDENTED: [&str; 3] = ["INDENT2", "INDENT2_SORTED", "STATE"];

/// One random case.
pub fn gen_case(rng: &mut Rng) -> Value {
    let k = rng.f64();
    let loads_dumps = |text: String, preset: &str| case_of("loads_dumps", "text", text, preset);
    if k < 0.40 {
        let mut t = rand_text(rng, 0);
        if rng.chance(0.2) {
            t = format!("{}{t}{}", rng.pick(&WS), rng.pick(&WS));
        }
        let p = any_preset(rng);
        loads_dumps(t, p)
    } else if k < 0.55 {
        let base = rand_text(rng, 2);
        let t = mutate(rng, &base);
        let p = any_preset(rng);
        loads_dumps(t, p)
    } else if k < 0.60 {
        let t = rand_text(rng, 2);
        let spots: Vec<usize> = t
            .char_indices()
            .filter(|(_, c)| "[{,:".contains(*c))
            .map(|(i, c)| i + c.len_utf8())
            .collect();
        let at = if spots.is_empty() {
            0
        } else {
            *rng.pick(&spots)
        };
        let ws = *rng.pick(&ODD_WS);
        loads_dumps(format!("{}{ws}{}", &t[..at], &t[at..]), "PY_DEFAULT")
    } else if k < 0.66 {
        loads_dumps(position_text(rng), "PY_DEFAULT")
    } else if k < 0.76 {
        let raw = rand_bytes(rng);
        let op = if rng.chance(0.5) {
            "loads_bytes_lossy"
        } else {
            "loads_utf8_sig"
        };
        let p = any_preset(rng);
        case_of(op, "hex", hex(&raw), p)
    } else if k < 0.86 {
        let t = special_number(rng);
        let t = if rng.chance(0.5) { format!("[{t}]") } else { t };
        let p = any_preset(rng);
        loads_dumps(t, p)
    } else if k < 0.91 {
        let n = 1 + rng.index(40);
        let items: Vec<String> = (0..n).map(|_| float_text(rng)).collect();
        let p = any_preset(rng);
        loads_dumps(format!("[{}]", items.join(",")), p)
    } else if k < 0.995 {
        let n = rng.index(6);
        let items: Vec<String> = (0..n)
            .map(|_| {
                let key = rng.string(4);
                let key = write_str(rng, &key);
                format!("{key}: {}", rand_text(rng, 1))
            })
            .collect();
        let p = any_preset(rng);
        loads_dumps(format!("{{{}}}", items.join(", ")), p)
    } else {
        deep_case(rng)
    }
}

const WS: [&str; 4] = [" ", "\t", "\n", "\r"];
const ODD_WS: [&str; 11] = [
    "\x0b", "\x0c", "\x1c", "\x1d", "\x1e", "\x1f", "\u{85}", "\u{a0}", "\u{2028}", "\u{3000}",
    "\u{feff}",
];

fn ws(rng: &mut Rng) -> String {
    let n = *rng.pick(&[0, 0, 0, 1, 2]);
    (0..n).map(|_| *rng.pick(&WS)).collect()
}

/// A JSON string literal for `s`, written with random escapes, plus (sometimes) lone surrogate
/// escapes that only exist in the text (D-6).
fn write_str(rng: &mut Rng, s: &str) -> String {
    let mut out = String::from("\"");
    let push_lone = |rng: &mut Rng, out: &mut String| {
        let c = 0xd800 + rng.below(0x800) as u32;
        if rng.chance(0.5) {
            out.push_str(&format!("\\u{c:04x}"));
        } else {
            out.push_str(&format!("\\u{c:04X}"));
        }
    };
    for ch in s.chars() {
        if rng.chance(0.01) {
            push_lone(rng, &mut out);
        }
        let c = ch as u32;
        let short = match ch {
            '"' => Some("\\\""),
            '\\' => Some("\\\\"),
            '\u{8}' => Some("\\b"),
            '\u{c}' => Some("\\f"),
            '\n' => Some("\\n"),
            '\r' => Some("\\r"),
            '\t' => Some("\\t"),
            '/' => Some("\\/"),
            _ => None,
        };
        if c > 0xffff && rng.chance(0.3) {
            let v = c - 0x10000;
            out.push_str(&format!(
                "\\u{:04x}\\u{:04x}",
                0xd800 + (v >> 10),
                0xdc00 + (v & 0x3ff)
            ));
        } else if let Some(sh) =
            short.filter(|_| c < 0x20 || "\"\\".contains(ch) || rng.chance(0.5))
        {
            if rng.chance(0.7) {
                out.push_str(sh);
            } else {
                out.push_str(&format!("\\u{c:04x}"));
            }
        } else if c < 0x20 || (c <= 0xffff && rng.chance(0.08)) {
            if rng.chance(0.5) {
                out.push_str(&format!("\\u{c:04x}"));
            } else {
                out.push_str(&format!("\\u{c:04X}"));
            }
        } else {
            out.push(ch);
        }
    }
    if rng.chance(0.03) {
        push_lone(rng, &mut out);
    }
    out.push('"');
    out
}

fn int_text(rng: &mut Rng) -> String {
    let k = rng.f64();
    let digits = |rng: &mut Rng, n: usize| -> String {
        let mut s = String::new();
        s.push(char::from(b'1' + rng.below(9) as u8));
        for _ in 1..n {
            s.push(char::from(b'0' + rng.below(10) as u8));
        }
        s
    };
    let n = if k < 0.4 {
        rng.range(-1000, 1000).to_string()
    } else if k < 0.6 {
        (rng.next_u64() as i64).to_string()
    } else if k < 0.75 {
        let base = *rng.pick(&[
            "9007199254740992",
            "9223372036854775808",
            "18446744073709551616",
            "1000000000000000000000",
        ]);
        let last = base.len() - 1;
        let d = (base.as_bytes()[last] - b'0') as i64 + rng.range(-3, 4);
        if (0..10).contains(&d) {
            format!("{}{d}", &base[..last])
        } else {
            base.to_string()
        }
    } else if k < 0.97 {
        let len = 19 + rng.index(60);
        digits(rng, len)
    } else {
        // Around the 4300-digit limit: a ValueError past it.
        let len = 4290 + rng.index(20);
        digits(rng, len)
    };
    if !n.starts_with('-') && n != "0" && rng.chance(0.3) {
        format!("-{n}")
    } else {
        n
    }
}

fn float_text(rng: &mut Rng) -> String {
    let k = rng.f64();
    if k < 0.45 {
        json::dumps(&Value::Float(rng.float_bits()), &json::PY_DEFAULT)
    } else if k < 0.65 {
        let (a, b) = (1 + rng.below(7) as u32, 1 + rng.below(7) as u32);
        let mut m = format!("{}.{}", rng.below(10u64.pow(a)), rng.below(10u64.pow(b)));
        if rng.chance(0.5) {
            m.push_str(&format!(
                "{}{}{}",
                rng.pick(&["e", "E"]),
                rng.pick(&["", "+", "-"]),
                rng.below(30)
            ));
        }
        m
    } else if k < 0.8 {
        format!(
            "{}{}e{}",
            rng.pick(&["", "-"]),
            1 + rng.below(9),
            rng.range(-330, 310)
        )
    } else if k < 0.9 {
        json::dumps(&Value::Float((rng.f64() - 0.5) * 2e6), &json::PY_DEFAULT)
    } else {
        special_number(rng)
    }
}

fn special_number(rng: &mut Rng) -> String {
    let k = rng.f64();
    if k < 0.5 {
        (*rng.pick(&[
            "NaN",
            "Infinity",
            "-Infinity",
            "-0.0",
            "0.0",
            "-0",
            "1e16",
            "1E16",
            "1e+16",
            "1e15",
            "5e-324",
            "4.9e-324",
            "2e-324",
            "1e400",
            "-1e400",
            "1e-400",
            "1e21",
            "1e22",
            "2.2250738585072014e-308",
            "1.7976931348623157e308",
            "1.7976931348623159e308",
            "0.1",
            "0.30000000000000004",
            "1000000000000000.25",
            "123456789012345678.0",
        ]))
        .to_string()
    } else if k < 0.8 {
        int_text(rng)
    } else {
        float_text(rng)
    }
}

fn rand_key(rng: &mut Rng) -> String {
    let k = rng.f64();
    if k < 0.15 {
        rng.pick(&[
            "0",
            "1",
            "2",
            "10",
            "99",
            "4294967294",
            "4294967295",
            "9007199254740993",
        ])
        .to_string()
    } else if k < 0.2 {
        rng.pick(&["01", "-1", "1.0", "", " 1"]).to_string()
    } else {
        rng.string(6)
    }
}

/// A random valid JSON text (the oracle's `_rand_text`).
fn rand_text(rng: &mut Rng, depth: usize) -> String {
    let k = rng.f64();
    if depth < 4 && k < 0.3 {
        let n = rng.index(6);
        if n == 0 {
            return format!("[{}]", ws(rng));
        }
        let items: Vec<String> = (0..n)
            .map(|_| {
                let w0 = ws(rng);
                let v = rand_text(rng, depth + 1);
                format!("{w0}{v}{}", ws(rng))
            })
            .collect();
        return format!("[{}]", items.join(","));
    }
    if depth < 4 && k < 0.55 {
        let n = rng.index(6);
        if n == 0 {
            return format!("{{{}}}", ws(rng));
        }
        let mut keys: Vec<String> = (0..n).map(|_| rand_key(rng)).collect();
        if n > 1 && rng.chance(0.3) {
            let i = rng.index(n);
            keys[i] = keys[0].clone();
        }
        let items: Vec<String> = keys
            .iter()
            .map(|key| {
                let (a, b, c) = (ws(rng), ws(rng), ws(rng));
                let ks = write_str(rng, key);
                let v = rand_text(rng, depth + 1);
                format!("{a}{ks}{b}:{c}{v}{}", ws(rng))
            })
            .collect();
        return format!("{{{}}}", items.join(","));
    }
    if k < 0.7 {
        let s = rng.string(8);
        return write_str(rng, &s);
    }
    if k < 0.82 {
        return int_text(rng);
    }
    if k < 0.95 {
        return float_text(rng);
    }
    rng.pick(&["null", "true", "false"]).to_string()
}

const MUTATION_CHARS: &[char] = &[
    '[', ']', '{', '}', ':', ',', '"', '\\', ' ', '0', '1', '5', '9', 'e', 'E', '.', '+', '-', 'a',
    'u', 'n', 'l', 't', 'r', 'f', 's', 'I', 'N', 'x', '\0', '\x1f', '\t', '\n', 'é', '😀',
];

fn mutate(rng: &mut Rng, text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return text.to_string();
    }
    let i = rng.index(chars.len());
    let c = *rng.pick(MUTATION_CHARS);
    let k = rng.f64();
    let mut out: Vec<char> = chars[..i].to_vec();
    if k < 0.35 {
        out.extend_from_slice(&chars[i + 1..]);
    } else if k < 0.7 {
        out.push(c);
        out.extend_from_slice(&chars[i..]);
    } else if k < 0.9 {
        // truncated
    } else {
        out.push(c);
        out.extend_from_slice(&chars[i + 1..]);
    }
    out.into_iter().collect()
}

fn position_text(rng: &mut Rng) -> String {
    let n = 1 + rng.index(5);
    let items: Vec<String> = (0..n)
        .map(|_| {
            let m = rng.index(6);
            let body: String = (0..m)
                .map(|_| {
                    *rng.pick(&[
                        "\u{1f600}",
                        "\u{10000}",
                        "\u{10ffff}",
                        "é",
                        "中",
                        "a",
                        "\\n",
                        "\\r\\n",
                        "\\ud83d\\ude00",
                    ])
                })
                .collect();
            format!("\"{body}\"")
        })
        .collect();
    let mut body = items[0].clone();
    for item in &items[1..] {
        body.push_str(rng.pick(&[", ", ",\r\n", ",\n", ",\r", ",\n\n "]));
        body.push_str(item);
    }
    let lead = *rng.pick(&["", "\r\n", "\n "]);
    let end = *rng.pick(&[
        ", x]",
        ", ]",
        " 1]",
        ", \"\\x\"]",
        ", \"a\x01\"]",
        ", \"\\ud800\\u12\"]",
        "",
        ", {\"k\" 1}]",
        ", 01]",
        ", \"abc",
        ", [1 2]]",
        ", -]",
        ", tru]",
        "]]",
        ", \"\\u12\"]",
    ]);
    format!("[{lead}{body}{end}")
}

const BAD_BYTES: &[&[u8]] = &[
    b"\x80",
    b"\xbf",
    b"\x80\x80",
    b"\xc0",
    b"\xc1",
    b"\xc0\x80",
    b"\xc0\xaf",
    b"\xc2",
    b"\xdf",
    b"\xe0",
    b"\xe0\x80",
    b"\xe0\x80\x80",
    b"\xe0\x9f\xbf",
    b"\xe0\xa0",
    b"\xe2\x82",
    b"\xed\xa0\x80",
    b"\xed\xbf\xbf",
    b"\xed\xa0",
    b"\xed\xa0\xbd\xed\xb8\x80",
    b"\xef\xbf",
    b"\xf0",
    b"\xf0\x80\x80\x80",
    b"\xf0\x90",
    b"\xf0\x90\x80",
    b"\xf4\x90\x80\x80",
    b"\xf5\x80\x80\x80",
    b"\xf8\x88\x80\x80\x80",
    b"\xfe",
    b"\xff",
    b"\xff\xfe",
];
const GOOD_BYTES: &[&[u8]] = &[
    b"a",
    b" ",
    b"\\n",
    b"\\u00e9",
    b"\\ud800",
    b"\xc2\x80",
    b"\xc3\xa9",
    b"\xdf\xbf",
    b"\xe0\xa0\x80",
    b"\xe2\x82\xac",
    b"\xed\x9f\xbf",
    b"\xee\x80\x80",
    b"\xef\xbf\xbf",
    b"\xef\xbb\xbf",
    b"\xf0\x90\x80\x80",
    b"\xf0\x9f\x98\x80",
    b"\xf4\x8f\xbf\xbf",
];
const BOM: &[u8] = b"\xef\xbb\xbf";

fn rand_bytes(rng: &mut Rng) -> Vec<u8> {
    let mut raw = b"\"".to_vec();
    for _ in 0..1 + rng.index(7) {
        let part = if rng.chance(0.5) {
            *rng.pick(BAD_BYTES)
        } else {
            *rng.pick(GOOD_BYTES)
        };
        raw.extend_from_slice(part);
    }
    if rng.chance(0.9) {
        raw.push(b'"');
    }
    if rng.chance(0.15) {
        raw.splice(0..0, BOM.iter().copied());
    }
    if rng.chance(0.2) {
        let mut wrapped = b"{\"k\": ".to_vec();
        wrapped.extend_from_slice(&raw);
        wrapped.push(b'}');
        raw = wrapped;
    }
    if rng.chance(0.1) {
        // A valid document as bytes, sometimes with a BOM.
        raw = rand_text(rng, 1).into_bytes();
        if rng.chance(0.3) {
            raw.splice(0..0, BOM.iter().copied());
        }
    }
    raw
}

/// Nesting up to 10,000, mostly near the top (1,000 under the indented presets, whose output grows with the
/// square of the depth).
fn deep_case(rng: &mut Rng) -> Value {
    let preset = any_preset(rng);
    let max = if INDENTED.contains(&preset) {
        1000
    } else {
        10_000
    };
    // CPython's C scanner raises RecursionError between 9,000 and 10,000 (measured) and the
    // indent encoder near 1,000, so two thirds of the depths sit at the top of the range (D-1).
    let d = match rng.below(3) {
        0 => 1 + rng.index(max),
        1 => max - rng.index(max / 10),
        _ => max,
    };
    let text = match rng.below(5) {
        0 => format!("{}{}", "[".repeat(d), "]".repeat(d)),
        1 => format!("{}1{}", "{\"a\":".repeat(d), "}".repeat(d)),
        2 => format!(
            "{}\"\u{1f600}\"{}",
            "[{\"k\": ".repeat(d / 2 + 1),
            "}]".repeat(d / 2 + 1)
        ),
        3 => format!("{}1,]{}", "[".repeat(d), "]".repeat(d.saturating_sub(1))),
        _ => format!("{}{}", "{\"a\": ".repeat(d), "}".repeat(d)),
    };
    if rng.chance(0.2) {
        let mut m = Map::new();
        m.insert("op".into(), Value::Str("loads_bytes_lossy".into()));
        m.insert("hex".into(), Value::Str(hex(text.as_bytes())));
        m.insert("preset".into(), Value::Str(preset.into()));
        return Value::Object(m);
    }
    case_of("loads_dumps", "text", text, preset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_cases_cover_every_op_preset_and_edge() {
        let mut rng = Rng::new(20261121);
        let cases = PycompatJson.generate(&mut rng, 4000);
        assert_eq!(cases.len(), 4000);
        assert_eq!(cases[0].id, "gen:1");
        let mut ops = std::collections::BTreeSet::new();
        let mut presets = std::collections::BTreeSet::new();
        let (mut lone, mut deep, mut invalid, mut big) = (0, 0, 0, 0);
        for c in &cases {
            ops.insert(text(&c.data, "op").to_string());
            presets.insert(text(&c.data, "preset").to_string());
            if let Some(t) = c
                .data
                .as_object()
                .and_then(|o| o.get("text"))
                .and_then(Value::as_str)
            {
                if t.contains("\\ud") || t.contains("\\uD") {
                    lone += 1;
                }
                if t.len() > 2000 && (t.starts_with("[[[[") || t.starts_with("{\"a\"")) {
                    deep += 1;
                }
                if loads(t).is_err() {
                    invalid += 1;
                }
                if t.len() > 4000
                    && t.trim_start_matches(['[', '-'])
                        .starts_with(|c: char| c.is_ascii_digit())
                {
                    big += 1;
                }
            }
            let o = run(&c.data);
            assert!(o.exit == 0 || o.exit == 1);
        }
        assert_eq!(ops.len(), 3);
        assert_eq!(presets.len(), 7);
        assert!(
            lone > 50 && deep > 3 && invalid > 300 && big > 2,
            "{lone} {deep} {invalid} {big}"
        );
    }

    #[test]
    fn rust_runs_a_golden_case_like_the_golden_says() {
        let seeds = PycompatJson.seeds();
        assert!(seeds.len() > 4000);
        let o = run(&seeds[0].data);
        assert_eq!(o, Observed::text(0, "null", ""));
    }
}
