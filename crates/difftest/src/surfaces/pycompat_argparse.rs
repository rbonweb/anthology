//! The `pycompat.argparse` difftest surface (P1's oracle `port/difftest/oracle/pycompat.py`).
//!
//! One op, `argparse.parse`: `parse_args(argv)` with `COLUMNS=columns`, compared on the exit
//! status and everything printed (usage and error lines, help text), or on the namespace (its
//! values and key order) when the parse returns.
//!
//! - **The real command lines.** `{"cli": name}` cases run, on the Python side, the parser the
//!   script itself builds (its `main([])` is stopped at `parse_args`, so nothing else runs),
//!   and on the Rust side the same parser written as a spec in `pycompat_argparse_clis.json`
//!   (the repository root written `<REPO>` on both sides). Seeds are the argument lists the
//!   CLI tests pass (`tests/test_*.py`) and their help and error paths; generated cases mutate
//!   them (abbreviations, `=`, missing and extra values, unknown options, `--`, choices, bad
//!   numbers, `-h`).
//! - **Generated parsers.** `{"spec": ...}` cases build the same parser on both sides from a
//!   random spec (see the oracle's docs on the format): positionals and options of every
//!   supported kind, mutually exclusive groups, subcommands, `allow_abbrev`, defaults of every
//!   type, `%`-keys in help, descriptions with long words, hyphens and non-ASCII spaces, and
//!   terminal widths from 20 to 200 columns; argument lists are drawn from the spec's own
//!   option strings, their prefixes and values.

use std::ffi::OsString;
use std::sync::OnceLock;

use anthology_pycompat::argparse::{self, Arg, Nargs, Spec, Subcommands, Type};
use anthology_pycompat::json::{self, Value};

use super::pycompat_str::{field_i64, field_opt, float_repr, gen_cases, int, obj, python, s, strs};
use crate::case::{Case, Rng, Surface};
use crate::observe::Observed;
use crate::oracle::Oracle;

/// The surface name (plan §1.4).
pub const NAME: &str = "pycompat.argparse";

/// Whether `difftest --all` runs this surface on macOS too (`ci/jobs/difftest-macos.sh`).
pub const ON_MACOS: bool = true;

/// The registered implementation.
pub fn surface() -> Option<Box<dyn Surface>> {
    Some(Box::new(PycompatArgparse))
}

struct PycompatArgparse;

/// The command lines' parsers, as specs (`{name: spec}`).
fn clis() -> &'static Value {
    static CLIS: OnceLock<Value> = OnceLock::new();
    CLIS.get_or_init(|| {
        json::loads(include_str!("pycompat_argparse_clis.json"))
            .expect("pycompat_argparse_clis.json is JSON")
    })
}

fn cli_spec(name: &str) -> &'static Value {
    clis()
        .as_object()
        .and_then(|o| o.get(name))
        .unwrap_or_else(|| panic!("pycompat.argparse: no command line {name:?}"))
}

fn get<'a>(v: &'a Value, k: &str) -> Option<&'a Value> {
    v.as_object()
        .and_then(|o| o.get(k))
        .filter(|v| !v.is_null())
}

fn get_str<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    get(v, k).and_then(Value::as_str)
}

fn arr<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    get(v, k)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// A spec value: JSON, a float written `{"float": repr}`.
fn value(v: &Value) -> argparse::Value {
    match v {
        Value::Null => argparse::Value::None,
        Value::Bool(b) => argparse::Value::Bool(*b),
        Value::Str(t) => argparse::Value::str(t),
        Value::Int(n) => argparse::Value::Int(n.clone()),
        Value::Float(f) => argparse::Value::Float(*f),
        Value::Array(a) => argparse::Value::List(a.iter().map(value).collect()),
        Value::Object(_) => {
            let text = get_str(v, "float").expect("a {\"float\": repr} value");
            argparse::Value::Float(text.parse().unwrap_or(match text {
                "inf" => f64::INFINITY,
                "-inf" => f64::NEG_INFINITY,
                _ => f64::NAN,
            }))
        }
    }
}

fn arg_from(a: &Value) -> Arg {
    let mut arg = match get_str(a, "name") {
        Some(n) => Arg::positional(n),
        None => {
            let flags: Vec<&str> = arr(a, "flags").iter().filter_map(Value::as_str).collect();
            Arg::option(&flags)
        }
    };
    if let Some(d) = get_str(a, "dest") {
        arg = arg.dest(d);
    }
    arg = match get_str(a, "action").unwrap_or("store") {
        "store" => arg,
        "store_true" => arg.store_true(),
        "store_false" => arg.store_false(),
        "store_const" => arg.store_const(get(a, "const").map_or(argparse::Value::None, value)),
        "append" => arg.append(),
        other => panic!("pycompat.argparse: action {other:?}"),
    };
    if let Some(n) = get(a, "nargs") {
        arg = arg.nargs(match n {
            Value::Str(t) if t == "?" => Nargs::Optional,
            Value::Str(t) if t == "*" => Nargs::ZeroOrMore,
            Value::Str(t) if t == "+" => Nargs::OneOrMore,
            n => Nargs::Exactly(n.as_i64().expect("an int nargs") as usize),
        });
    }
    if get_str(a, "action") != Some("store_const")
        && let Some(k) = get(a, "const")
    {
        arg = arg.konst(value(k));
    }
    if let Some(d) = a.as_object().and_then(|o| o.get("default")) {
        arg = arg.default(value(d));
    }
    if let Some(t) = get_str(a, "type") {
        arg = arg.ty(match t {
            "int" => Type::Int,
            "float" => Type::Float,
            "path" => Type::Path,
            other => panic!("pycompat.argparse: type {other:?}"),
        });
    }
    if let Some(c) = get(a, "choices").and_then(Value::as_array) {
        let c: Vec<&str> = c.iter().filter_map(Value::as_str).collect();
        arg = arg.choices(&c);
    }
    if get(a, "required").and_then(Value::as_bool) == Some(true) {
        arg = arg.required();
    }
    if let Some(h) = get_str(a, "help") {
        arg = arg.help(h);
    }
    if get(a, "help_suppress").and_then(Value::as_bool) == Some(true) {
        arg = arg.help_suppress();
    }
    match get(a, "metavar") {
        Some(Value::Str(m)) => arg = arg.metavar(m),
        Some(Value::Array(m)) => {
            let m: Vec<&str> = m.iter().filter_map(Value::as_str).collect();
            arg = arg.metavars(&m);
        }
        _ => {}
    }
    arg
}

fn spec_from(v: &Value) -> Spec {
    let mut spec = match get_str(v, "prog") {
        Some(p) => Spec::new(p),
        None => Spec::command(),
    };
    if let Some(d) = get_str(v, "description") {
        spec = spec.description(d);
    }
    if let Some(e) = get_str(v, "epilog") {
        spec = spec.epilog(e);
    }
    if get(v, "allow_abbrev").and_then(Value::as_bool) == Some(false) {
        spec = spec.allow_abbrev(false);
    }
    for item in arr(v, "args") {
        if let Some(m) = get(item, "mutex") {
            let required = get(m, "required").and_then(Value::as_bool) == Some(true);
            spec = spec.mutex(required, arr(m, "args").iter().map(arg_from).collect());
        } else if let Some(sub) = get(item, "sub") {
            let mut sc = Subcommands::new();
            if let Some(d) = get_str(sub, "dest") {
                sc = sc.dest(d);
            }
            if get(sub, "required").and_then(Value::as_bool) == Some(true) {
                sc = sc.required();
            }
            if let Some(h) = get_str(sub, "help") {
                sc = sc.help(h);
            }
            if let Some(m) = get_str(sub, "metavar") {
                sc = sc.metavar(m);
            }
            for p in arr(sub, "parsers") {
                let name = get_str(p, "name").expect("a parser name");
                let child = spec_from(get(p, "spec").expect("a parser spec"));
                sc = sc.parser(name, get_str(p, "help"), child);
            }
            spec = spec.subcommands(sc);
        } else {
            spec = spec.arg(arg_from(item));
        }
    }
    spec
}

fn out_value(v: &argparse::Value) -> Value {
    match v {
        argparse::Value::None => Value::Null,
        argparse::Value::Bool(b) => Value::Bool(*b),
        argparse::Value::Str(t) => s(&t.to_string_lossy()),
        argparse::Value::Int(n) => Value::Int(n.clone()),
        argparse::Value::Float(f) => obj(vec![("float", s(&float_repr(*f)))]),
        argparse::Value::List(l) => Value::Array(l.iter().map(out_value).collect()),
    }
}

fn run(case: &Value) -> Observed {
    let columns = field_i64(case, "columns");
    let spec = match get_str(case, "cli") {
        Some(name) => spec_from(cli_spec(name)),
        None => spec_from(field_opt(case, "spec").expect("a spec")),
    };
    let spec = spec.width(argparse::help_width(Some(&columns.to_string()), None));
    let argv: Vec<OsString> = arr(case, "argv")
        .iter()
        .map(|a| OsString::from(a.as_str().expect("a str argument")))
        .collect();
    match argparse::parse(&spec, &argv) {
        Ok(parsed) => {
            let ns = obj(parsed
                .values
                .iter()
                .map(|(k, v)| (k.as_str(), out_value(v)))
                .collect());
            let order: Vec<&str> = parsed.values.iter().map(|(k, _)| k.as_str()).collect();
            let result = obj(vec![("ns", ns), ("order", strs(&order))]);
            Observed::text(0, &json::dumps(&result, &json::PY_DEFAULT_SORTED), "")
        }
        Err(e) => Observed::text(e.code, &e.stdout, &e.stderr),
    }
}

fn parse_case(target: (&str, Value), argv: &[&str], columns: i64) -> Value {
    obj(vec![
        ("op", s("argparse.parse")),
        target,
        ("argv", strs(argv)),
        ("columns", int(columns)),
    ])
}

/// The argument lists of the CLI tests, with the help and error paths around them.
const CLI_SEEDS: &[(&str, &[&[&str]])] = &[
    (
        "memory_portable",
        &[
            &[],
            &["-h"],
            &["export"],
            &["export", "--out", "/tmp/work"],
            &["export", "--json"],
            &["export", "-h"],
            &["import", "a.tar.gz", "--yes"],
            &["import", "a.tar.gz", "--dry-run", "--json"],
            &["import", "a.tar.gz", "--sha256", "abc"],
            &["import", "-h"],
            &["import"],
            &["bogus"],
            &["export", "--o", "x"],
            &["export", "--js"],
            &["import", "f", "--s=abc"],
            &["export", "extra"],
            &["--json"],
            &["export", "--out"],
        ],
    ),
    (
        "rtk_install",
        &[
            &[],
            &["-h"],
            &["--data-dir", "/d", "--lock", "/l", "--dry-run"],
            &["--data-dir", "/d"],
            &["--data-dir=/d", "--timeout", "5"],
            &["--data-dir", "/d", "--timeout", "abc"],
            &["--data-dir", "/d", "--timeout", "1e3"],
            &["--da", "/d"],
            &["--d", "/d"],
            &["--data-dir", "/d", "--timeout", "-1"],
            &["--data-dir", "/d", "--timeout", "-1.5"],
            &["--data-dir", "/d", "x"],
            &["--data-dir"],
            &["--timeout", "nan", "--data-dir", "/d"],
        ],
    ),
    (
        "memory_index",
        &[
            &[],
            &["-h"],
            &["search", "a", "b"],
            &["search"],
            &["search", "--json", "x"],
            &["search", "-h"],
            &["search", "--", "-x"],
            &["related", "n", "--depth", "3"],
            &["related", "n", "--depth", "x"],
            &["related", "n", "--dep=1"],
            &["related"],
            &["orphans"],
            &["orphans", "--json"],
            &["stale", "--days", "30"],
            &["rebuild"],
            &[
                "save",
                "--agent",
                "a",
                "--type",
                "t",
                "--slug",
                "s",
                "--name",
                "n",
                "--description",
                "d",
                "--body",
                "b",
            ],
            &["save", "--agent", "a"],
            &[
                "save", "--ag", "a", "--type", "t", "--slug", "s", "--name", "n", "--body", "b",
            ],
            &["save", "-h"],
            &["nope"],
        ],
    ),
    (
        "anthology_setup",
        &[
            &[],
            &["-h"],
            &["preview", "--plugin-root", "/p", "--cwd", "/c"],
            &["remove", "--preview", "--plugin-root", "/p", "--cwd", "/c"],
            &[
                "apply",
                "--plugin-root",
                "/p",
                "--expect-sha256",
                "abc",
                "--promote",
                "a",
                "--promote",
                "b",
            ],
            &["apply", "--plugin-root", "/p"],
            &["apply", "-h"],
            &[
                "remove",
                "--unapprove",
                "--unapprove-all",
                "--plugin-root",
                "/p",
            ],
            &["remove", "--un", "--plugin-root", "/p"],
            &["preview"],
            &["preview", "--plugin-root"],
            &[
                "apply",
                "--plugin-root",
                "/p",
                "--expect-sha256",
                "x",
                "--skip",
            ],
        ],
    ),
    (
        "doctor",
        &[
            &[],
            &["--json"],
            &["-h"],
            &["--js"],
            &["--json=1"],
            &["x"],
            &["-j"],
        ],
    ),
    (
        "release_edit",
        &[
            &["plugin-version", "p.json"],
            &["check-version", "1.0.0", "1.0.1", "0"],
            &["check-version", "1", "2", "3"],
            &["check-dependents", "r", "p", "1.0"],
            &["changelog-check", "c.md", "anthology"],
            &[
                "changelog-move",
                "c.md",
                "anthology",
                "1.2.3",
                "--date",
                "2026-01-01",
            ],
            &["changelog-move", "c.md", "p", "1"],
            &["set-version", "p", "1"],
            &["set-source-version", "p", "t", "1"],
            &[],
            &["-h"],
            &["check-version", "-h"],
            &["set-version", "p"],
            &["changelog-check", "a", "b", "c"],
        ],
    ),
    (
        "rtk_trim",
        &[
            &[],
            &["-h"],
            &["--lock-guard", "a", "b"],
            &["--lock-guard", "a"],
            &["--trimmed-conflicts"],
            &["--removed-paths", "--trimmed-conflicts"],
            &["--root", "/r", "--removed-paths"],
            &["--lock-guard", "--x", "-h"],
            &["--lock", "a", "b", "--root=/r"],
            &["--r", "x"],
        ],
    ),
    (
        "vendor",
        &[
            &[],
            &["-h"],
            &["fetch"],
            &["fetch", "a", "b"],
            &["fetch", "--", "-x"],
            &["build"],
            &["build", "x", "--out", "/o"],
            &["build", "--out", "a//b/"],
            &["build", "-h"],
            &["check"],
            &["sync", "x", "--ref", "abc"],
            &["sync", "x"],
            &["diff", "x"],
            &["--root", "/r", "check"],
            &["--cache", "c", "fetch"],
            &["--help"],
            &["nope"],
            &["--root"],
        ],
    ),
    (
        "upstream_watch",
        &[
            &[],
            &["--dry-run"],
            &["--json", "out.json"],
            &["--root", "/r", "--dry-run"],
            &["-h"],
            &["--dry-run", "--json"],
            &["--j", "o"],
            &["--dry-run", "extra"],
        ],
    ),
    (
        "eval_baseline",
        &[
            &["record", "r.json", "b.json"],
            &["candidate", "r", "o"],
            &[
                "compare",
                "r",
                "b",
                "--tolerance",
                "0.5",
                "--allow-missing",
                "--summary",
                "s.md",
            ],
            &["compare", "r", "b", "--tolerance", "x"],
            &["compare", "r", "b", "--tol=1e-3"],
            &["compare", "r"],
            &[],
            &["-h"],
            &["compare", "-h"],
            &["record", "-h"],
        ],
    ),
    (
        "check_version_bump",
        &[
            &["base", "head"],
            &["0000", "h", "--repo", "/r"],
            &["base"],
            &[],
            &["-h"],
            &["a", "b", "c"],
            &["-1", "-2"],
            &["a", "b", "--repo"],
        ],
    ),
    (
        "startup_budget",
        &[
            &[],
            &["reader", "scout"],
            &["--root", "/r", "reader"],
            &["-h"],
            &["--root"],
            &["reader", "--root", "/r", "scout"],
        ],
    ),
];

/// Every option string, subcommand name and choice in a spec, for argument lists.
fn spec_words(spec: &Value, out: &mut Vec<String>) {
    for item in arr(spec, "args") {
        if let Some(m) = get(item, "mutex") {
            for a in arr(m, "args") {
                arg_words(a, out);
            }
        } else if let Some(sub) = get(item, "sub") {
            for p in arr(sub, "parsers") {
                out.extend(get_str(p, "name").map(str::to_string));
                if let Some(child) = get(p, "spec") {
                    spec_words(child, out);
                }
            }
        } else {
            arg_words(item, out);
        }
    }
}

fn arg_words(a: &Value, out: &mut Vec<String>) {
    for f in arr(a, "flags").iter().filter_map(Value::as_str) {
        out.push(f.to_string());
    }
    for c in arr(a, "choices").iter().filter_map(Value::as_str) {
        out.push(c.to_string());
    }
}

const VALUES: &[&str] = &[
    "v", "x", "1", "-1", "2.5", "-2.5", "-.5", "x y", "", "nan", "inf", "1_000", "\u{663}", " 7 ",
    "a", "b", "é", "0x10", "1e3", "--", "-", "a=b", "a//b/", "/abs/./p",
];

const UNKNOWN: &[&str] = &[
    "--zz", "-q", "--x=1", "-", "--", "-h", "--help", "--h", "-hx",
];

/// A token for an argument list over `words` (option strings, names, choices).
fn token(rng: &mut Rng, words: &[String]) -> String {
    let k = rng.f64();
    if k < 0.45 && !words.is_empty() {
        let w = rng.pick(words).clone();
        let chars: Vec<char> = w.chars().collect();
        if w.starts_with("--") && chars.len() > 3 && rng.chance(0.3) {
            // An abbreviation.
            return chars[..2 + rng.index(chars.len() - 2)].iter().collect();
        }
        if w.starts_with('-') && rng.chance(0.2) {
            let joiner = if w.starts_with("--") || rng.chance(0.5) {
                "="
            } else {
                ""
            };
            return format!("{w}{joiner}{}", rng.pick(VALUES));
        }
        if w.starts_with('-') && !w.starts_with("--") && rng.chance(0.15) {
            // Combined short flags.
            let other = rng.pick(words);
            if other.starts_with('-') && !other.starts_with("--") {
                return format!("{w}{}", &other[1..]);
            }
        }
        w
    } else if k < 0.9 {
        rng.pick(VALUES).to_string()
    } else if rng.chance(0.3) {
        rng.pick(UNKNOWN).to_string()
    } else {
        rng.pick(&["--zz", "-q", "--x=1", "-", "--"]).to_string()
    }
}

fn argv_for(rng: &mut Rng, words: &[String]) -> Vec<String> {
    (0..rng.index(8)).map(|_| token(rng, words)).collect()
}

/// A value for an argument: one of its choices, or a number or word that fits its type.
fn good_value(rng: &mut Rng, a: &Value) -> String {
    let choices: Vec<&str> = arr(a, "choices").iter().filter_map(Value::as_str).collect();
    if !choices.is_empty() && rng.chance(0.8) {
        return rng.pick(&choices).to_string();
    }
    match get_str(a, "type") {
        Some("int") => rng
            .pick(&["0", "7", "-3", "1_000", " 42 ", "\u{663}"])
            .to_string(),
        Some("float") => rng
            .pick(&["1.5", "-0.5", "1e3", "inf", "nan", "7", ".5"])
            .to_string(),
        _ => rng
            .pick(&["v", "x", "a//b/", "é", "w o", "/abs"])
            .to_string(),
    }
}

fn value_count(rng: &mut Rng, a: &Value) -> usize {
    match get(a, "nargs") {
        None => 1,
        Some(Value::Str(t)) if t == "?" => rng.index(2),
        Some(Value::Str(t)) if t == "*" => rng.index(3),
        Some(Value::Str(t)) if t == "+" => 1 + rng.index(2),
        Some(n) => n.as_i64().unwrap_or(1) as usize,
    }
}

/// An argument list `spec` accepts more often than not: every positional with values, some
/// options with theirs (as `--opt v`, `--opt=v` or `-ov`), one choice of each mutually
/// exclusive group, and a subcommand with its own arguments.
fn good_argv(rng: &mut Rng, spec: &Value) -> Vec<String> {
    let mut options: Vec<Vec<String>> = Vec::new();
    let mut positionals: Vec<String> = Vec::new();
    let mut tail: Vec<String> = Vec::new();
    let option = |rng: &mut Rng, a: &Value, options: &mut Vec<Vec<String>>| {
        let flags: Vec<&str> = arr(a, "flags").iter().filter_map(Value::as_str).collect();
        let flag = rng.pick(&flags).to_string();
        let takes = matches!(get_str(a, "action"), None | Some("store") | Some("append"));
        if !takes {
            options.push(vec![flag]);
            return;
        }
        let n = value_count(rng, a);
        let values: Vec<String> = (0..n).map(|_| good_value(rng, a)).collect();
        if n == 1 && rng.chance(0.3) {
            let joiner = if flag.starts_with("--") { "=" } else { "" };
            options.push(vec![format!("{flag}{joiner}{}", values[0])]);
        } else {
            let mut v = vec![flag];
            v.extend(values);
            options.push(v);
        }
    };
    for item in arr(spec, "args") {
        if let Some(m) = get(item, "mutex") {
            let members = arr(m, "args");
            if !members.is_empty() && rng.chance(0.8) {
                let a = rng.pick(members);
                if get(a, "flags").is_some() {
                    option(rng, a, &mut options);
                }
            }
        } else if let Some(sub) = get(item, "sub") {
            let parsers = arr(sub, "parsers");
            if !parsers.is_empty() && rng.chance(0.9) {
                let p = rng.pick(parsers);
                tail.push(get_str(p, "name").unwrap_or("x").to_string());
                if let Some(child) = get(p, "spec") {
                    tail.extend(good_argv(rng, child));
                }
            }
        } else if get(item, "flags").is_some() {
            let required = get(item, "required").and_then(Value::as_bool) == Some(true);
            if required || rng.chance(0.5) {
                option(rng, item, &mut options);
            }
        } else {
            let n = value_count(rng, item);
            for _ in 0..n {
                positionals.push(good_value(rng, item));
            }
        }
    }
    rng.shuffle(&mut options);
    let mut out: Vec<String> = Vec::new();
    let split = rng.index(options.len() + 1);
    for o in &options[..split] {
        out.extend(o.iter().cloned());
    }
    out.extend(positionals);
    for o in &options[split..] {
        out.extend(o.iter().cloned());
    }
    out.extend(tail);
    out
}

/// A CLI seed argument list, mutated.
fn mutate(rng: &mut Rng, seed: &[&str], words: &[String]) -> Vec<String> {
    let mut argv: Vec<String> = seed.iter().map(|s| s.to_string()).collect();
    for _ in 0..1 + rng.index(3) {
        let k = rng.f64();
        if k < 0.35 || argv.is_empty() {
            let at = rng.index(argv.len() + 1);
            argv.insert(at, token(rng, words));
        } else if k < 0.6 {
            let at = rng.index(argv.len());
            argv.remove(at);
        } else if k < 0.85 {
            let at = rng.index(argv.len());
            argv[at] = token(rng, words);
        } else if argv.len() > 1 {
            let at = rng.index(argv.len() - 1);
            let next = argv.remove(at + 1);
            argv[at] = format!("{}={next}", argv[at]);
        }
    }
    argv
}

// ---- random parsers ------------------------------------------------------------------------

const WORDS: &[&str] = &[
    "show",
    "the",
    "output",
    "of",
    "a",
    "file",
    "co-operate",
    "e-mail",
    "well--known",
    "--",
    "-b",
    "über-cool",
    "1-2-3",
    "naïve",
    "end.",
    "x",
    "averyveryverylongwordthatexceedsthewidthoftheline",
    "a-very-long-hyphenated-word-that-wraps-somewhere",
    "\u{a0}",
    "tab\there",
    "line\nbreak",
    "  ",
    "\u{3000}",
    "(default:",
    "none)",
];

const HELP_KEYS: &[&str] = &[
    "%(default)s",
    "%(default)r",
    "%(prog)s",
    "%(dest)s",
    "%(metavar)s",
    "%(choices)s",
    "%(type)s",
    "%(nargs)s",
    "%(required)s",
    "%(const)s",
    "%%",
];

fn text(rng: &mut Rng, keys: &[&str], max_words: usize) -> String {
    let n = rng.index(max_words + 1);
    let mut out: Vec<String> = Vec::new();
    for _ in 0..n {
        if !keys.is_empty() && rng.chance(0.12) {
            out.push(rng.pick(keys).to_string());
        } else {
            out.push(rng.pick(WORDS).to_string());
        }
    }
    let sep = if rng.chance(0.8) { " " } else { "  " };
    out.join(sep)
}

const SHORTS: &[&str] = &["-a", "-b", "-c", "-x", "-1"];
const LONGS: &[&str] = &[
    "--alpha",
    "--alpine",
    "--al",
    "--beta",
    "--b-c",
    "--x-ray",
    "--long-option-name-here",
    "--very-long-option-name-to-force-wrapping",
];
const POSITIONALS: &[&str] = &["src", "dst", "names", "n", "Mode"];
const METAVARS: &[&str] = &["FILE", "N", "x", "LONG_METAVAR_NAME", "é", "PATH"];
const CHOICES: &[&str] = &["a", "b", "c", "1", "2", "é"];

fn choices_value(rng: &mut Rng) -> Value {
    let n = 1 + rng.index(3);
    let mut c: Vec<&str> = Vec::new();
    while c.len() < n {
        let x = *rng.pick(CHOICES);
        if !c.contains(&x) {
            c.push(x);
        }
    }
    strs(&c)
}

fn default_value(rng: &mut Rng) -> Value {
    match rng.index(9) {
        0 => Value::Null,
        1 => s(rng.pick(&["5", "x", "a", "", "a//b", "1.5"])),
        2 => int(rng.range(-3, 300)),
        3 => obj(vec![(
            "float",
            s(rng.pick(&["0.0", "1.5", "1e+16", "-0.0"])),
        )]),
        4 => Value::Bool(rng.chance(0.5)),
        5 => strs(&["a", "b"]),
        6 => Value::Array(Vec::new()),
        _ => s(rng.pick(CHOICES)),
    }
}

/// One option or positional; `used` holds the option strings and dests taken.
fn random_arg(rng: &mut Rng, used: &mut Vec<String>, positional: bool, in_group: bool) -> Value {
    let mut pairs: Vec<(&str, Value)> = Vec::new();
    let nargs_pick = |rng: &mut Rng, opts: &[i64]| -> Option<Value> {
        match *rng.pick(opts) {
            -1 => None,
            -2 => Some(s("?")),
            -3 => Some(s("*")),
            -4 => Some(s("+")),
            n => Some(int(n)),
        }
    };
    let mut nargs: Option<Value> = None;
    let mut action = "store";
    if positional {
        let name = POSITIONALS
            .iter()
            .find(|p| !used.contains(&p.to_string()))
            .copied()
            .unwrap_or("rest");
        used.push(name.to_string());
        pairs.push(("name", s(name)));
        nargs = if in_group {
            nargs_pick(rng, &[-2, -3])
        } else {
            nargs_pick(rng, &[-1, -1, -2, -3, -4, 1, 2])
        };
    } else {
        let mut flags: Vec<&str> = Vec::new();
        for pool in [SHORTS, LONGS] {
            if rng.chance(0.65) || (flags.is_empty() && pool == LONGS) {
                let free: Vec<&str> = pool
                    .iter()
                    .filter(|f| !used.contains(&f.to_string()))
                    .copied()
                    .collect();
                if !free.is_empty() {
                    flags.push(*rng.pick(&free));
                }
            }
        }
        if flags.is_empty() {
            flags.push(
                ["--o1", "--o2", "--o3", "--o4", "--o5", "--o6"]
                    .iter()
                    .find(|f| !used.contains(&f.to_string()))
                    .copied()
                    .unwrap_or("--o7"),
            );
        }
        used.extend(flags.iter().map(|f| f.to_string()));
        pairs.push(("flags", strs(&flags)));
        action = *rng.pick(&[
            "store",
            "store",
            "store",
            "store_true",
            "store_true",
            "store_false",
            "store_const",
            "append",
        ]);
        if action != "store" {
            pairs.push(("action", s(action)));
        }
        if action == "store" {
            nargs = nargs_pick(rng, &[-1, -1, -1, -2, -3, -4, 1, 2, 3]);
        } else if action == "append" {
            nargs = nargs_pick(rng, &[-1, -1, -2, -4, 2]);
        }
        if action == "store_const" {
            pairs.push(("const", default_value(rng)));
        }
        if !in_group && rng.chance(0.15) {
            pairs.push(("required", Value::Bool(true)));
        }
    }
    let takes_values = action == "store" || action == "append";
    if let Some(n) = &nargs {
        pairs.push(("nargs", n.clone()));
    }
    if takes_values
        && !positional
        && nargs.as_ref().and_then(Value::as_str) == Some("?")
        && rng.chance(0.5)
    {
        pairs.push(("const", default_value(rng)));
    }
    if takes_values && rng.chance(0.25) {
        pairs.push(("type", s(rng.pick(&["int", "float", "path"]))));
    }
    if takes_values && rng.chance(0.2) {
        pairs.push(("choices", choices_value(rng)));
    }
    let optional_positional = matches!(nargs.as_ref().and_then(Value::as_str), Some("?" | "*"));
    if (!positional || optional_positional) && rng.chance(0.35) {
        // An `append` default is a list or None (Python appends to it).
        let d = if action == "append" {
            if rng.chance(0.5) {
                Value::Null
            } else {
                strs(&["a"][..rng.index(2)])
            }
        } else {
            default_value(rng)
        };
        pairs.push(("default", d));
    }
    if rng.chance(0.05) {
        pairs.push(("help_suppress", Value::Bool(true)));
    } else if rng.chance(0.7) {
        pairs.push(("help", s(&text(rng, HELP_KEYS, 14))));
    }
    if takes_values && rng.chance(0.3) {
        let n = nargs.as_ref().and_then(Value::as_i64);
        match n {
            Some(n) if n >= 2 && rng.chance(0.5) => {
                let m: Vec<&str> = (0..n).map(|_| *rng.pick(METAVARS)).collect();
                pairs.push(("metavar", strs(&m)));
            }
            _ => pairs.push(("metavar", s(rng.pick(METAVARS)))),
        }
    }
    obj(pairs)
}

fn random_spec(rng: &mut Rng, prog: Option<&str>, depth: usize) -> Value {
    let mut pairs: Vec<(&str, Value)> = Vec::new();
    if let Some(p) = prog {
        pairs.push(("prog", s(p)));
    }
    if rng.chance(0.5) {
        let keys: &[&str] = if rng.chance(0.3) {
            &["%(prog)s", "%%"]
        } else {
            &[]
        };
        pairs.push(("description", s(&text(rng, keys, 30))));
    }
    if rng.chance(0.15) {
        pairs.push(("epilog", s(&text(rng, &[], 12))));
    }
    if rng.chance(0.2) {
        pairs.push(("allow_abbrev", Value::Bool(false)));
    }
    let mut used: Vec<String> = Vec::new();
    let mut items = Vec::new();
    let mut sub_done = depth > 1;
    for _ in 0..rng.index(7) {
        let k = rng.f64();
        if k < 0.12 {
            let n = 2 + rng.index(2);
            let args: Vec<Value> = (0..n)
                .map(|_| {
                    let positional = rng.chance(0.1);
                    random_arg(rng, &mut used, positional, true)
                })
                .collect();
            items.push(obj(vec![(
                "mutex",
                obj(vec![
                    ("required", Value::Bool(rng.chance(0.3))),
                    ("args", Value::Array(args)),
                ]),
            )]));
        } else if k < 0.25 && !sub_done {
            sub_done = true;
            let mut sp: Vec<(&str, Value)> = Vec::new();
            if rng.chance(0.7) {
                sp.push(("dest", s(rng.pick(&["command", "cmd"]))));
            }
            if rng.chance(0.6) {
                sp.push(("required", Value::Bool(true)));
            }
            if rng.chance(0.2) {
                sp.push(("help", s(&text(rng, &[], 6))));
            }
            if rng.chance(0.15) {
                sp.push(("metavar", s("CMD")));
            }
            let mut names: Vec<&str> = Vec::new();
            let mut parsers = Vec::new();
            for _ in 0..1 + rng.index(3) {
                let name = *rng.pick(&["run", "build", "b", "r", "x-y", "rerun"]);
                if names.contains(&name) {
                    continue;
                }
                names.push(name);
                let mut p = vec![("name", s(name))];
                if rng.chance(0.6) {
                    p.push(("help", s(&text(rng, &[], 8))));
                }
                p.push(("spec", random_spec(rng, None, depth + 1)));
                parsers.push(obj(p));
            }
            sp.push(("parsers", Value::Array(parsers)));
            items.push(obj(vec![("sub", obj(sp))]));
        } else {
            let positional = rng.chance(0.35);
            items.push(random_arg(rng, &mut used, positional, false));
        }
    }
    pairs.push(("args", Value::Array(items)));
    obj(pairs)
}

const COLUMNS: &[i64] = &[80, 80, 80, 60, 40, 120, 30, 200, 20];

impl Surface for PycompatArgparse {
    fn name(&self) -> &'static str {
        NAME
    }

    fn seeds(&self) -> Vec<Case> {
        let mut out = Vec::new();
        for (cli, argvs) in CLI_SEEDS {
            for (i, argv) in argvs.iter().enumerate() {
                out.push(Case::new(
                    format!("{cli}:{}", i + 1),
                    parse_case(("cli", s(cli)), argv, 80),
                ));
                if argv.first().is_some_and(|a| *a == "-h") || argv.is_empty() {
                    for columns in [40, 120] {
                        out.push(Case::new(
                            format!("{cli}:{}:{columns}", i + 1),
                            parse_case(("cli", s(cli)), argv, columns),
                        ));
                    }
                }
            }
        }
        out
    }

    fn generate(&self, rng: &mut Rng, n: usize) -> Vec<Case> {
        gen_cases(n, || {
            let columns = *rng.pick(COLUMNS);
            if rng.chance(0.4) {
                let (cli, argvs) = rng.pick(CLI_SEEDS);
                let mut words = Vec::new();
                spec_words(cli_spec(cli), &mut words);
                let seed = *rng.pick(argvs);
                let argv = mutate(rng, seed, &words);
                let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
                parse_case(("cli", s(cli)), &argv, columns)
            } else {
                let prog = *rng.pick(&["p", "tool.py", "a-rather-long-program-name.py", "prög"]);
                let spec = random_spec(rng, Some(prog), 0);
                let mut words = Vec::new();
                spec_words(&spec, &mut words);
                let argv = if rng.chance(0.55) {
                    let good = good_argv(rng, &spec);
                    let seed: Vec<&str> = good.iter().map(String::as_str).collect();
                    if rng.chance(0.3) {
                        mutate(rng, &seed, &words)
                    } else {
                        good
                    }
                } else {
                    argv_for(rng, &words)
                };
                let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
                parse_case(("spec", spec), &argv, columns)
            }
        })
    }

    fn run_python(&self, c: &Case, oracle: &mut Oracle) -> Observed {
        python(c, oracle)
    }

    fn run_rust(&self, c: &Case) -> Observed {
        run(&c.data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_cli_spec_builds() {
        for (cli, _) in CLI_SEEDS {
            let _ = spec_from(cli_spec(cli));
        }
        let mut words = Vec::new();
        spec_words(cli_spec("memory_index"), &mut words);
        assert!(words.contains(&"--depth".to_string()));
    }
}
