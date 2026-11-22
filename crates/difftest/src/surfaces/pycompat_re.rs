//! The `pycompat.re` difftest surface (P1's oracle `port/difftest/oracle/pycompat.py`).
//!
//! - `re.classes` (seeds): for every code point, in chunks, whether each of a list of patterns
//!   fully matches it: `\w \W \s \S \d \D`, the same inside classes, `.`, under no flags, `(?a)`
//!   and IGNORECASE (literals and classes whose case-insensitive sets differ between CPython
//!   and Rust's `(?i)`, such as `i`, `s`, `k`, `σ`, `[a-z]`).
//! - `re.finditer`: every group span of every match, for the patterns the Python sources use
//!   (privacy, guard, settings, smoke, upstream-watch), lookarounds and back-references, edge
//!   syntax (`{` literals, `{,n}`, octal and `\x` escapes, `$` before a final newline, verbose
//!   mode, scoped flags), and random patterns from a small grammar on random text. A pattern
//!   Python refuses must be refused (both sides answer `error`), but for a look-behind whose
//!   width is not fixed (see `pycompat::re`).
//!
//! Spans are compared in code points; Rust's byte offsets are converted.
//!
//! Generated text mixes characters whose word-ness Rust's own `\b` would get wrong (marks,
//! `No` digits, `Pc`, `So` letters such as `Ⓐ`, ZWJ, a Unicode 15.1 letter), and patterns
//! include lazy repeats that can match empty and empty alternatives (sre's must-advance retry).
//! What `pycompat::re` refuses on purpose ("not supported") is left out, and so is a search
//! that hits fancy-regex's backtrack limit (D-7; the `d7:*` seed shows one).

use anthology_pycompat::json::Value;
use anthology_pycompat::re::{self as pyre, Flags};

use super::pycompat_str::{
    field, field_i64, gen_cases, int, obj, ok, pool_string, python, raised, runs, s, strs,
};
use crate::case::{Case, Rng, Surface};
use crate::observe::Observed;
use crate::oracle::Oracle;

/// The surface name (plan §1.4).
pub const NAME: &str = "pycompat.re";

/// Whether `difftest --all` runs this surface on macOS too (`ci/jobs/difftest-macos.sh`).
pub const ON_MACOS: bool = true;

/// The registered implementation.
pub fn surface() -> Option<Box<dyn Surface>> {
    Some(Box::new(PycompatRe))
}

struct PycompatRe;

const CHUNK: u32 = 0x8000;

const CLASS_PATTERNS: &[&str] = &[
    r"\w", r"\W", r"\s", r"\S", r"\d", r"\D", r"[\w]", r"[^\w]", r"[\W\d]", r"[\s_]", r"[^\S\n]",
    r".", r"[\D]",
];

const IGNORECASE_PATTERNS: &[&str] = &[
    "i",
    "I",
    "s",
    "k",
    "K",
    "σ",
    "Σ",
    "ς",
    "µ",
    "ǅ",
    "ß",
    "ﬀ",
    "İ",
    "ı",
    "ſ",
    "θ",
    "ϑ",
    "Ω",
    "[a-z]",
    "[^a-z]",
    "[A-Z0-9_]",
    "[ΣA]",
    "[\u{100}-\u{17f}]",
    "[\u{3b0}-\u{3ff}]",
    r"[\w-]",
    "[!-/]",
    "[\u{2160}-\u{2188}]",
    "[\u{10400}]",
    "\u{10400}",
    "[ǅ-ǌ]",
];

/// (pattern, flags, texts): the Python sources' patterns and edge syntax.
const FIND_SEEDS: &[(&str, &str, &[&str])] = &[
    // memory_privacy.py
    (
        r"(?<![\w.%+-])([A-Za-z0-9._%+-]+)@([A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,})(?![\w-])",
        "",
        &[
            "mail me: a.b+c@ex-ample.co.uk, x@y.z",
            "é@mail.com a@b.cd_",
            "a@b.c.de-",
        ],
    ),
    (
        r"(?<![\w.])(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})(?!\d|\.\d)",
        "",
        &[
            "10.0.0.1 and 1.2.3.4.5, ١٠.٠.٠.١ v1.2.3.4",
            "300.1.1.1x 8.8.8.8\n",
        ],
    ),
    (
        r"(?<![\w:.])[0-9A-Fa-f]{1,4}(?::[0-9A-Fa-f]{0,4}){2,7}(?:%[\w.-]+)?(?![\w:])",
        "",
        &[
            "fe80::1%eth0 and ::1, 2001:db8::ff00:42:8329",
            "a:b:c fe80::1%é",
        ],
    ),
    (
        r"(?<![\w.-])(?:[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?\.)+(?:local|lan|internal|intranet|corp|home|localdomain|home\.arpa|ts\.net)(?![\w(-]|\.[\w-])",
        "i",
        &[
            "ssh build.LOCAL; db.corp.internal, x.home.arpa.",
            "printer.lan(1) a.ts.net",
        ],
    ),
    (
        r"(?<![\w-])(?:ssh|scp|sftp|rsync|mosh)(?![\w-])",
        "",
        &["ssh host; my-ssh x; rsync -a; SSH", "scp_x mosh"],
    ),
    (
        r"(?<![A-Za-z0-9_-])eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}",
        "",
        &["t=eyJhbGciOiJI.eyJzdWIiOiIx.SflKxwRJSMeK x-eyJabcdefgh.abcdefgh.abcdefgh"],
    ),
    (
        "password(?:\\s*[:=](?![ \\t]*(?:\\\\?[\\\"'](?:null|none|true|false|<[^<>\\r\\n]*>|\\$\\{[^{}\\r\\n]*\\}|[*.xX]+)\\\\?[\\\"']|(?:null|none|true|false|<[^<>\\r\\n]*>|\\$\\{[^{}\\r\\n]*\\}|[*.xX]+))[ \\t]*(?:[,}\\]\\r\\n]|$))|\\\\?[\\\"']\\s*[:=]\\s*(?!(?:\\\\?[\\\"'](?:null|none|true|false|<[^<>\\r\\n]*>|\\$\\{[^{}\\r\\n]*\\}|[*.xX]+)\\\\?[\\\"']|(?:null|none|true|false|<[^<>\\r\\n]*>|\\$\\{[^{}\\r\\n]*\\}|[*.xX]+))[ \\t]*(?:[,}\\]\\r\\n]|$))(?:\\\\?[\\\"'])?(?![\\\\\\\"'{\\[\\]},\\s])\\S)",
        "i",
        &[
            "PASSWORD=hunter2",
            "password: null\n",
            "\"password\": \"x\"",
            "passwOrd = ${PW}",
            "PASSWORD=*** ",
            "password=",
        ],
    ),
    (
        r"(?:secret|access)[_-]?(?:access[_-]?)?key[\x22']?\s*[:=]\s*\S{8,}",
        "i",
        &[
            "aws_secret_access_key = abcdefgh12",
            "\"SecretAccessKey\": \"0123456789\"",
        ],
    ),
    (
        r"://[^\s/:@]*:[^\s/@]+@",
        "",
        &["postgres://u:p@h/db https://a@b"],
    ),
    (
        r"\bgh[pousr]_[A-Za-z0-9]+",
        "",
        &["token ghp_abc123 xghp_no _ghp_x"],
    ),
    (
        r"Authorization:\s*Basic\s+[A-Za-z0-9+/=]{8,}",
        "i",
        &[
            "authorization: basic dXNlcjpwYXNz",
            "AUTHORİZATION: BASIC abcdefgh",
        ],
    ),
    (
        r"/(?:Users|home)/([A-Za-z0-9._-]+)",
        "",
        &["/Users/someone/a and /home/tester/b"],
    ),
    // guard_rules.py, settings_patch.py, memory_index.py, smoke.py, upstream_watch.py
    (
        r#"<<(?!<)-?[ \t]*(['"]?)([A-Za-z_][A-Za-z0-9_.-]*)\1"#,
        "",
        &[
            "cat <<EOF\nx\nEOF",
            "cat <<-'END' x",
            "cat <<< x",
            "cat << \"A\"B",
        ],
    ),
    (
        r"file:.*%[0-9a-f]{2}",
        "is",
        &["FILE:/x%2E\n%2f", "file:%zz"],
    ),
    (
        r#""(?:[^"\\\x00-\x1f]|\\.)*""#,
        "s",
        &[r#"{"a": "b\"c", "d\n": 1}"#],
    ),
    (r"(?m)^([ \t]+)\S", "", &["{\n  \"a\": 1\n\t\"b\"\n}"]),
    (r"^---[ \t]*$", "m", &["---\nname: x\n--- \nbody\n---"]),
    (
        r"(?i)(TOKEN|SECRET|KEY|PASSWORD|PASSWD|CREDENTIAL)",
        "",
        &["GH_TOKEN api_key Secret ķey"],
    ),
    (
        r"(?ms)^### Startup budget\n.*?(?=^### |\Z)",
        "",
        &["a\n### Startup budget\nx\ny\n### B\nz"],
    ),
    (
        r"^## Startup\n(.*?)(?=^## |\Z)",
        "ms",
        &["## Startup\n- a\n## Next\n", "## Startup\nend"],
    ),
    (r"^# (.+)$", "m", &["# Title\nbody\n# Two  \n"]),
    (
        r"(?<![\w-])--json(?![\w-])",
        "",
        &["x --json y --json-out --json"],
    ),
    // Edge syntax.
    (r"a$", "", &["a\n", "a\n\n", "a", "ba\nca"]),
    (r"$", "", &["", "x\n", "x\ny"]),
    (r"a\Z", "", &["a\n", "a"]),
    (r"x*", "", &["abxd", ""]),
    (r"\b", "", &["ab cd", "a_b-c", "é ü"]),
    (r"a{,2}b{2,}c{1}d{0}", "", &["aabbbcd abbc"]),
    (r"a{b{1,x}c{}d{,}", "", &["a{b{1,x}c{}d{,}", "a{b{1,x}c{}d"]),
    (r"\x41\101\0é\U0001F600\t\.", "", &["AA\0é😀\t."]),
    (r"[]a-]+[^]x]", "", &["]-a]b"]),
    (r"[\b\x08]+", "", &["\x08\x08"]),
    (r"(?P<q>['\x22])(.*?)(?P=q)", "", &["say 'hi' and \"yo\""]),
    (r"(a)(b)?(?:c|(d))", "", &["ac abd ad"]),
    (r"(?x) a \  b # comment\n | [ ]c", "", &["a b c", "ab"]),
    (r"a b # not a comment", "x", &["ab", "a b"]),
    (r"(?i:A)b", "", &["ab Ab aB"]),
    (r"(?-i:A)b", "i", &["Ab AB ab"]),
    (r"(?s:.)\n.", "", &["\n\nx"]),
    (r"(?a)\w+\s\d", "", &["é1 a\u{a0}1 a 1 a ١"]),
    (r"\w+", "a", &["naïve Ⅻ x_1"]),
    (r"(?<=\d)\s(?=\D)", "", &["1 a 2 3 ٣ b"]),
    (r"(?#comment)x", "", &["x"]),
    (r"a++", "", &["aa"]),
    (r"(?(1)a|b)", "", &["a"]),
    (r"(?<n>x)", "", &["x"]),
    (r"a**", "", &["aa"]),
    (r"a*+b|x?+|(?:ab)++", "", &["aab xx abab"]),
    (
        r"(?=a)*a(?<=a){2}(?!b)?|(?:(?=c))+c|(?:$)*d",
        "",
        &["a ab c d"],
    ),
    (r"\B", "", &["", "ab", " "]),
    (r"\B", "a", &["", "ab", " "]),
    (r"(\-*){2}", "", &["--"]),
    (r"(?i)a(?L)", "", &["a"]),
    (r"x(?i)", "", &["x"]),
    (r"[z-a]", "", &["a"]),
    (r"\q", "", &["q"]),
    (r"(a)\2", "", &["a"]),
    (r"(a\1)", "", &["aa"]),
    (r"(?P<a>x(?P=a))", "", &["xx"]),
    (r"(?P<a>x)(?P=b)", "", &["xx"]),
    (r"(?P<a>x)(?P<a>y)", "", &["xy"]),
    (r"(?P<a>x)(?P=a)", "", &["xx"]),
    (r"(?i-i:a)", "", &["a"]),
    (r"(?au)a", "", &["a"]),
    (r"(?aa)a", "", &["a"]),
    (r"(?-:a)", "", &["a"]),
    (r"(?i-:a)", "", &["a"]),
    (r"(?s-i:.)", "", &["\n"]),
];

impl Surface for PycompatRe {
    fn name(&self) -> &'static str {
        NAME
    }

    fn seeds(&self) -> Vec<Case> {
        let mut out = Vec::new();
        let mut lo = 0;
        while lo < 0x11_0000 {
            for (flags, pats) in [
                ("", CLASS_PATTERNS),
                ("a", CLASS_PATTERNS),
                ("i", IGNORECASE_PATTERNS),
                ("ai", IGNORECASE_PATTERNS),
            ] {
                out.push(Case::new(
                    format!("classes-{flags}:{lo:x}"),
                    obj(vec![
                        ("op", s("re.classes")),
                        ("patterns", strs(pats)),
                        ("flags", s(flags)),
                        ("lo", int(lo.into())),
                        ("hi", int((lo + CHUNK).into())),
                    ]),
                ));
            }
            lo += CHUNK;
        }
        let mut n = 0;
        for (pattern, flags, texts) in FIND_SEEDS {
            for text in *texts {
                n += 1;
                out.push(Case::new(
                    format!("find:{n}"),
                    finditer(pattern, flags, text),
                ));
            }
        }
        // `\b` and `\B` next to what Rust's Unicode word set gets wrong for sre: marks, `No`,
        // `Pc`, `So` letters, ZWJ/ZWNJ, a letter unassigned in Unicode 15.0; and the
        // must-advance retry after an empty match.
        let boundary = [
            "a\u{301}b",
            "x\u{300} y",
            "²a",
            "a‿b",
            "Ⓐ",
            "a\u{200d}b",
            "a\u{200c}b",
            "\u{2ebf0}a",
            "Ⓐghp_x",
            "",
        ];
        for (i, text) in boundary.iter().enumerate() {
            for (j, pattern) in [r"\b", r"\B", r"\bghp_"].iter().enumerate() {
                out.push(Case::new(
                    format!("boundary:{}:{}", i + 1, j + 1),
                    finditer(pattern, "", text),
                ));
            }
        }
        // D-7: a pattern whose search hits fancy-regex's backtrack limit (Python answers).
        out.push(Case::new(
            "d7:1",
            finditer(
                r"(?s)(?:(?a:(?<=[a-z]){,2}?.|.)+\bé{,}(?![^\s,]*?İ{1,3}Σ{){2})*i{1,2}?1",
                "",
                "𮯰c𐐀$aI\u{200c}醥ks \u{1c}ςiba",
            ),
        ));
        for (i, (pattern, text)) in [
            ("a??", "a"),
            ("|a", "ab"),
            (r"\s*?", " x"),
            ("(a)??", "aa"),
            ("x*?|x", "xx"),
            ("(?:|a)+?b", "ab"),
            ("(?=a)|a", "aa"),
            // Blank groups repeated (fancy-regex refuses to repeat them).
            ("(?x: )*-|(?!a)x", " -x"),
            ("(?:){3}|(?<=a)b", "ab"),
            ("(?:(?:)|)+?b", "ab"),
        ]
        .iter()
        .enumerate()
        {
            out.push(Case::new(
                format!("advance:{}", i + 1),
                finditer(pattern, "", text),
            ));
        }
        out
    }

    fn generate(&self, rng: &mut Rng, n: usize) -> Vec<Case> {
        gen_cases(n, || gen_case(rng))
    }

    fn run_python(&self, c: &Case, oracle: &mut Oracle) -> Observed {
        python(c, oracle)
    }

    fn run_rust(&self, c: &Case) -> Observed {
        let flags = flags_of(field(&c.data, "flags"));
        match field(&c.data, "op") {
            "re.finditer" => run_finditer(field(&c.data, "pattern"), flags, field(&c.data, "text")),
            "re.classes" => run_classes(&c.data, flags),
            op => panic!("pycompat.re: unknown op {op:?}"),
        }
    }
}

fn finditer(pattern: &str, flags: &str, text: &str) -> Value {
    obj(vec![
        ("op", s("re.finditer")),
        ("pattern", s(pattern)),
        ("flags", s(flags)),
        ("text", s(text)),
    ])
}

fn flags_of(f: &str) -> Flags {
    Flags {
        ignorecase: f.contains('i'),
        multiline: f.contains('m'),
        dotall: f.contains('s'),
        verbose: f.contains('x'),
        ascii: f.contains('a'),
    }
}

fn run_finditer(pattern: &str, flags: Flags, text: &str) -> Observed {
    let re = match pyre::compile_with(pattern, flags) {
        Ok(r) => r,
        Err(_) => return raised("error"),
    };
    // Byte offset -> code-point offset.
    let mut cp_at = vec![0usize; text.len() + 1];
    let mut n = 0;
    for (b, ch) in text.char_indices() {
        for slot in &mut cp_at[b..b + ch.len_utf8()] {
            *slot = n;
        }
        n += 1;
    }
    cp_at[text.len()] = n;
    let mut out = Vec::new();
    for m in re.captures_iter(text) {
        let Ok(groups) = m else {
            return raised("LimitError");
        };
        out.push(Value::Array(
            groups
                .into_iter()
                .map(|g| match g {
                    Some((a, b)) => Value::Array(vec![int(cp_at[a] as i64), int(cp_at[b] as i64)]),
                    None => Value::Null,
                })
                .collect(),
        ));
    }
    ok(&Value::Array(out))
}

fn run_classes(case: &Value, flags: Flags) -> Observed {
    let lo = field_i64(case, "lo") as u32;
    let hi = field_i64(case, "hi") as u32;
    let pats = case
        .as_object()
        .and_then(|o| o.get("patterns"))
        .and_then(Value::as_array)
        .expect("patterns");
    let mut out = Vec::new();
    for p in pats {
        let p = p.as_str().expect("a pattern");
        let re = match pyre::compile_with(&format!(r"\A(?:{p})\Z"), flags) {
            Ok(r) => r,
            Err(_) => return raised("error"),
        };
        let mut buf = [0u8; 4];
        out.push(runs(lo, hi, |c| {
            re.is_match(c.encode_utf8(&mut buf)).unwrap_or(false)
        }));
    }
    ok(&Value::Array(out))
}

// ---------------------------------------------------------------------------------------------
// Random patterns.

/// Text characters.
const TEXT: &[char] = &[
    'a',
    'b',
    'c',
    'x',
    'A',
    'B',
    'Z',
    '0',
    '1',
    '9',
    '_',
    ' ',
    ' ',
    '\n',
    '\n',
    '-',
    '.',
    ',',
    'é',
    'ß',
    'Σ',
    'σ',
    'ς',
    'İ',
    'ı',
    'i',
    'I',
    's',
    'S',
    'ſ',
    'k',
    'K',
    '\u{212a}',
    'ж',
    '中',
    '٣',
    '²',
    '\u{301}',
    '\u{a0}',
    '\u{3000}',
    '\x1c',
    '\t',
    '{',
    '}',
    '[',
    ']',
    '\\',
    '$',
    '^',
    '😀',
    '𐐀',
    '𐐨',
    'Ⓐ',
    '\u{200d}',
    '\u{200c}',
    '\u{2ebf0}',
    '\u{2160}',
    '‿',
    '\u{345}',
];

const LITERALS: &[&str] = &[
    "a",
    "b",
    "c",
    "x",
    "A",
    "Z",
    "0",
    "1",
    "_",
    " ",
    "-",
    ".",
    "é",
    "Σ",
    "σ",
    "ß",
    "İ",
    "ı",
    "i",
    "s",
    "k",
    "K",
    "ж",
    "😀",
    r"\.",
    r"\-",
    r"\{",
    r"\}",
    r"\[",
    r"\]",
    r"\\",
    r"\$",
    r"\n",
    r"\t",
    r"\x41",
    r"é",
    r"\U0001f600",
    r"\0",
    r"\101",
    "{",
    "}",
    "]",
    "\u{a0}",
    "'",
    "\"",
    ",",
    "!",
    "%",
    "@",
];

const CLASSES: &[&str] = &[
    r"\w",
    r"\W",
    r"\s",
    r"\S",
    r"\d",
    r"\D",
    ".",
    "[a-z]",
    "[^a-z]",
    "[A-Z_]",
    r"[\w.-]",
    r"[^\s,]",
    "[ab]",
    "[^ab]",
    "[]a]",
    "[a-]",
    r"[\d\s]",
    "[é-ü]",
    "[Σσς]",
    "[^\n]",
    r"[\x00-\x1f]",
    "[i]",
    "[ſ]",
    "[0-9a-fA-F]",
    r"[\\-]",
    "[&&]",
    "[~~]",
    "[--]",
    "[[]",
];

const QUANTS: &[&str] = &[
    "", "", "", "", "*", "+", "?", "{2}", "{1,3}", "{,2}", "{2,}", "+?", "{1,2}?", "{", "{,}",
    "*?", "??", "{,2}?",
];

fn atom(rng: &mut Rng, depth: usize, groups: &mut usize, closed: &mut Vec<usize>) -> String {
    let k = rng.f64();
    if depth < 2 && k < 0.15 {
        let inner = seq(rng, depth + 1, groups, closed);
        let kind = rng.index(5);
        match kind {
            0 => {
                *groups += 1;
                let g = *groups;
                let t = format!("({inner})");
                closed.push(g);
                t
            }
            1 => format!("(?:{inner})"),
            2 => {
                *groups += 1;
                let g = *groups;
                closed.push(g);
                format!("(?P<g{g}>{inner})")
            }
            3 => format!("(?{}{inner})", rng.pick(&["=", "!"])),
            _ => format!("(?{}:{inner})", rng.pick(&["i", "s", "m", "-i", "x", "a"])),
        }
    } else if k < 0.20 {
        // A fixed-width look-behind.
        let body = rng.pick(&["a", "[a-z]", r"\w", r"\d", " ", "ab", r"\s"]);
        format!("(?{}{body})", rng.pick(&["<=", "<!"]))
    } else if k < 0.23 && !closed.is_empty() {
        let g = *rng.pick(closed);
        if rng.chance(0.5) {
            format!(r"(?:\{g})")
        } else {
            format!("(?:(?P=g{g}))")
        }
    } else if k < 0.30 {
        (*rng.pick(&["^", "$", r"\A", r"\Z", r"\b", r"\B"])).to_string()
    } else if k < 0.55 {
        (*rng.pick(CLASSES)).to_string()
    } else {
        (*rng.pick(LITERALS)).to_string()
    }
}

fn seq(rng: &mut Rng, depth: usize, groups: &mut usize, closed: &mut Vec<usize>) -> String {
    let mut out = String::new();
    let branches = if rng.chance(0.2) { 2 } else { 1 };
    for b in 0..branches {
        if b > 0 {
            out.push('|');
        }
        let n = if branches > 1 && rng.chance(0.15) {
            0
        } else {
            1 + rng.index(4)
        };
        for _ in 0..n {
            let a = atom(rng, depth, groups, closed);
            out.push_str(&a);
            let anchor = matches!(a.as_str(), "^" | "$" | r"\A" | r"\Z" | r"\b" | r"\B");
            if !anchor {
                out.push_str(rng.pick(QUANTS));
            }
        }
        if branches > 1 && rng.chance(0.5) {
            out.push_str(rng.pick(LITERALS));
        }
    }
    out
}

fn gen_case(rng: &mut Rng) -> Value {
    let (pattern, flags, text) = loop {
        let mut groups = 0;
        let mut closed = Vec::new();
        let mut pattern = seq(rng, 0, &mut groups, &mut closed);
        if rng.chance(0.05) {
            pattern = format!(
                "(?{}){pattern}",
                rng.pick(&["i", "m", "s", "a", "x", "ims"])
            );
        }
        let flags: String = ['i', 'm', 's', 'x', 'a']
            .iter()
            .filter(|_| rng.chance(0.12))
            .collect();
        // What pycompat::re refuses on purpose (its module docs: a back-reference under
        // IGNORECASE, a group inside a repeat that can match empty, ...) is left out; a
        // pattern Python refuses too stays in.
        let refused = pyre::translate_with(&pattern, flags_of(&flags))
            .is_err_and(|e| e.contains("not supported"));
        let text = pool_string(rng, TEXT, 16);
        // A search that hits fancy-regex's backtrack limit is D-7 (Rust fails closed; the
        // `d7:*` seed shows one): left out of the generated cases.
        let limited = pyre::compile_with(&pattern, flags_of(&flags))
            .is_ok_and(|re| re.find_iter(&text).any(|m| m.is_err()));
        if !refused && !limited {
            break (pattern, flags, text);
        }
    };
    finditer(&pattern, &flags, &text)
}
