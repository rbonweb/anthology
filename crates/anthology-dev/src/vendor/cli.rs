//! The `vendor` command line: vendor.py's `main` (argparse, then one command).
//!
//! The parser stands in for `pycompat::argparse` (P1: swap). It does what Python 3.12's
//! argparse does for this grammar:
//! - the help and usage texts, verbatim (prog `vendor.py`);
//! - top-level options before the subcommand, the subcommand taking everything after it;
//! - `--opt value`, `--opt=value` and unique prefixes (`--ou`);
//! - a value that looks like an option is "expected one argument";
//! - positionals in chunks between options (an empty chunk defers an optional positional);
//! - unknown arguments reported by the top-level parser after the subcommand parsed;
//! - `--` ending the options;
//! - `-h` printing the help and exiting 0 as soon as it is seen.
//!
//! `set-rtk-upstream` is the one subcommand vendor.py does not have (it replaces the inline
//! Python in `sync-rtk.sh`). It is accepted but not listed, so the help and the invalid-choice
//! text stay Python's.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use super::build::build;
use super::compare::check;
use super::diff::diff_entry;
use super::fetch::{dispatch_fetcher, fetch_entry, git_fetcher};
use super::licenses::fetch_licenses;
use super::manifest::{CACHE_DIRNAME, find_entry, load_sources, vendored};
use super::py;
use super::set_rtk_upstream::set_rtk_upstream;
use super::sync::sync;
use super::{Fetch, Result, VendorError};
use crate::Output;
use anthology_pycompat::json::Value;

const TOP_USAGE: &str = "usage: vendor.py [-h] [--cache CACHE] {fetch,build,check,sync,diff} ...\n";
const TOP_HELP: &str = "\
usage: vendor.py [-h] [--cache CACHE] {fetch,build,check,sync,diff} ...

Reproducible vendoring: every vendored tree is generated from a pinned
upstream.

positional arguments:
  {fetch,build,check,sync,diff}
    fetch               fetch pinned upstreams and copy their license files
    build               pristine -> transforms -> patch -> target
    check               rebuild all into a temp dir and diff against the repo
    sync                re-pin to REF and rebuild; never commits
    diff                pristine+transforms vs shipped, as a patch

options:
  -h, --help            show this help message and exit
  --cache CACHE         pristine cache (default: <root>/.vendor-cache)
";

/// One subcommand's grammar.
struct Spec {
    name: &'static str,
    usage: &'static str,
    help: &'static str,
    /// Long options taking one value: (option, dest, required).
    options: &'static [(&'static str, &'static str, bool)],
    /// Positionals in order.
    positionals: &'static [(&'static str, Nargs)],
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Nargs {
    One,
    Optional,
    Star,
}

const SPECS: &[Spec] = &[
    Spec {
        name: "fetch",
        usage: "usage: vendor.py fetch [-h] [names ...]\n",
        help: "\
usage: vendor.py fetch [-h] [names ...]

positional arguments:
  names

options:
  -h, --help  show this help message and exit
",
        options: &[],
        positionals: &[("names", Nargs::Star)],
    },
    Spec {
        name: "build",
        usage: "usage: vendor.py build [-h] [--out OUT] [name]\n",
        help: "\
usage: vendor.py build [-h] [--out OUT] [name]

positional arguments:
  name        default: every source

options:
  -h, --help  show this help message and exit
  --out OUT   write <out>/<target> instead of the repo
",
        options: &[("--out", "out", false)],
        positionals: &[("name", Nargs::Optional)],
    },
    Spec {
        name: "check",
        usage: "usage: vendor.py check [-h]\n",
        help: "\
usage: vendor.py check [-h]

options:
  -h, --help  show this help message and exit
",
        options: &[],
        positionals: &[],
    },
    Spec {
        name: "sync",
        usage: "usage: vendor.py sync [-h] --ref REF name\n",
        help: "\
usage: vendor.py sync [-h] --ref REF name

positional arguments:
  name

options:
  -h, --help  show this help message and exit
  --ref REF
",
        options: &[("--ref", "ref", true)],
        positionals: &[("name", Nargs::One)],
    },
    Spec {
        name: "diff",
        usage: "usage: vendor.py diff [-h] name\n",
        help: "\
usage: vendor.py diff [-h] name

positional arguments:
  name

options:
  -h, --help  show this help message and exit
",
        options: &[],
        positionals: &[("name", Nargs::One)],
    },
    Spec {
        name: "set-rtk-upstream",
        usage: "usage: vendor.py set-rtk-upstream [-h] --ref REF --commit COMMIT\n",
        help: "\
usage: vendor.py set-rtk-upstream [-h] --ref REF --commit COMMIT

options:
  -h, --help       show this help message and exit
  --ref REF        the rtk upstream tag
  --commit COMMIT  the commit the tag points at
",
        options: &[("--ref", "ref", true), ("--commit", "commit", true)],
        positionals: &[],
    },
];

/// What parsing ended with, short of a namespace.
enum Exit {
    Help(&'static str),
    /// (usage, prog, message)
    Error(&'static str, String, String),
}

fn error(usage: &'static str, prog: &str, msg: impl Into<String>) -> Exit {
    Exit::Error(usage, prog.to_string(), msg.into())
}

/// argparse's `_negative_number_matcher`.
fn negative_number(s: &str) -> bool {
    let Some(rest) = s.strip_prefix('-') else {
        return false;
    };
    let digits = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
    match rest.split_once('.') {
        None => digits(rest),
        Some((int, frac)) => int.bytes().all(|b| b.is_ascii_digit()) && digits(frac),
    }
}

/// Whether argparse reads `arg` as an option string (`_parse_optional` not returning None).
fn looks_like_option(arg: &str, known: &[&str]) -> bool {
    if !arg.starts_with('-') || arg == "-" {
        return false;
    }
    if known.contains(&arg) || arg == "--" {
        return true;
    }
    if let Some((name, _)) = arg.split_once('=')
        && known.contains(&name)
    {
        return true;
    }
    if arg.starts_with("--")
        && known
            .iter()
            .any(|k| k.starts_with(arg.split('=').next().unwrap_or(arg)))
    {
        return true;
    }
    !(negative_number(arg) || arg.contains(' '))
}

/// The option `arg` names among `known` (exact, `--x=v`, or a unique `--` prefix), with an
/// explicit `=value`.
fn match_option<'a>(arg: &str, known: &[&'a str]) -> Option<(&'a str, Option<String>)> {
    let (name, value) = match arg.split_once('=') {
        Some((n, v)) if arg.starts_with("--") => (n, Some(v.to_string())),
        _ => (arg, None),
    };
    if let Some(k) = known.iter().find(|k| **k == name) {
        return Some((k, value));
    }
    if name.starts_with("--") && name.len() > 2 {
        let hits: Vec<&&str> = known.iter().filter(|k| k.starts_with(name)).collect();
        if hits.len() == 1 {
            return Some((hits[0], value));
        }
    }
    None
}

/// A parsed subcommand: option values and positionals by dest.
#[derive(Default)]
struct Parsed {
    values: Vec<(&'static str, OsString)>,
    positionals: Vec<(&'static str, Vec<OsString>)>,
}

impl Parsed {
    fn value(&self, dest: &str) -> Option<&OsString> {
        self.values
            .iter()
            .rev()
            .find(|(d, _)| *d == dest)
            .map(|(_, v)| v)
    }
    fn positional(&self, dest: &str) -> Vec<OsString> {
        self.positionals
            .iter()
            .find(|(d, _)| *d == dest)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    }
}

/// One parser's pass over `args` (a subcommand's, or the top level up to the subcommand).
/// Unknown arguments go to `extras`.
fn parse_sub(
    spec: &Spec,
    args: &[OsString],
    extras: &mut Vec<OsString>,
) -> std::result::Result<Parsed, Exit> {
    let prog = format!("vendor.py {}", spec.name);
    let mut known: Vec<&str> = vec!["-h", "--help"];
    known.extend(spec.options.iter().map(|o| o.0));
    let mut parsed = Parsed::default();
    let mut next_positional = 0;
    let mut chunk: Vec<OsString> = Vec::new();
    let mut only_positionals = false;
    let mut i = 0;
    let flush = |chunk: &mut Vec<OsString>,
                 parsed: &mut Parsed,
                 extras: &mut Vec<OsString>,
                 next_positional: &mut usize| {
        let mut rest = std::mem::take(chunk).into_iter().peekable();
        while *next_positional < spec.positionals.len() && rest.peek().is_some() {
            let (dest, nargs) = spec.positionals[*next_positional];
            let taken: Vec<OsString> = match nargs {
                Nargs::One | Nargs::Optional => rest.next().into_iter().collect(),
                Nargs::Star => rest.by_ref().collect(),
            };
            parsed.positionals.push((dest, taken));
            *next_positional += 1;
        }
        extras.extend(rest);
    };
    while i < args.len() {
        let raw = &args[i];
        let arg = raw.to_string_lossy();
        if only_positionals {
            chunk.push(raw.clone());
        } else if arg == "--" {
            if next_positional < spec.positionals.len() {
                only_positionals = true;
            } else {
                flush(&mut chunk, &mut parsed, extras, &mut next_positional);
                extras.push(raw.clone());
            }
        } else if looks_like_option(&arg, &known) {
            flush(&mut chunk, &mut parsed, extras, &mut next_positional);
            match match_option(&arg, &known) {
                Some(("-h" | "--help", _)) => return Err(Exit::Help(spec.help)),
                Some((opt, value)) => {
                    let (_, dest, _) = spec
                        .options
                        .iter()
                        .find(|o| o.0 == opt)
                        .copied()
                        .unwrap_or(("", "", false));
                    let value = match value {
                        Some(v) => OsString::from(v),
                        None => match args.get(i + 1) {
                            Some(next) if !looks_like_option(&next.to_string_lossy(), &known) => {
                                i += 1;
                                next.clone()
                            }
                            _ => {
                                return Err(error(
                                    spec.usage,
                                    &prog,
                                    format!("argument {opt}: expected one argument"),
                                ));
                            }
                        },
                    };
                    parsed.values.push((dest, value));
                }
                None => extras.push(raw.clone()),
            }
        } else {
            chunk.push(raw.clone());
        }
        i += 1;
    }
    flush(&mut chunk, &mut parsed, extras, &mut next_positional);
    let mut missing: Vec<&str> = Vec::new();
    for (dest, nargs) in &spec.positionals[next_positional..] {
        if *nargs == Nargs::One {
            missing.push(dest);
        }
    }
    for (opt, dest, required) in spec.options {
        if *required && parsed.value(dest).is_none() {
            missing.push(opt);
        }
    }
    if !missing.is_empty() {
        return Err(error(
            spec.usage,
            &prog,
            format!(
                "the following arguments are required: {}",
                missing.join(", ")
            ),
        ));
    }
    Ok(parsed)
}

/// The parsed command line.
struct Args {
    root: Option<OsString>,
    cache: Option<OsString>,
    command: &'static str,
    sub: Parsed,
}

fn parse(args: &[OsString]) -> std::result::Result<Args, Exit> {
    let known = ["-h", "--help", "--root", "--cache"];
    let mut extras: Vec<OsString> = Vec::new();
    let (mut root, mut cache) = (None, None);
    let mut i = 0;
    let mut only_positionals = false;
    while i < args.len() {
        let raw = &args[i];
        let arg = raw.to_string_lossy();
        if !only_positionals && arg == "--" {
            only_positionals = true;
            i += 1;
            continue;
        }
        if !only_positionals && looks_like_option(&arg, &known) {
            match match_option(&arg, &known) {
                Some(("-h" | "--help", _)) => return Err(Exit::Help(TOP_HELP)),
                Some((opt, value)) => {
                    let value = match value {
                        Some(v) => OsString::from(v),
                        None => match args.get(i + 1) {
                            Some(next) if !looks_like_option(&next.to_string_lossy(), &known) => {
                                i += 1;
                                next.clone()
                            }
                            _ => {
                                return Err(error(
                                    TOP_USAGE,
                                    "vendor.py",
                                    format!("argument {opt}: expected one argument"),
                                ));
                            }
                        },
                    };
                    if opt == "--root" {
                        root = Some(value);
                    } else {
                        cache = Some(value);
                    }
                }
                None => extras.push(raw.clone()),
            }
            i += 1;
            continue;
        }
        // The subcommand: it takes every argument after it.
        let Some(spec) = SPECS.iter().find(|s| *s.name == *arg) else {
            return Err(error(
                TOP_USAGE,
                "vendor.py",
                format!(
                    "argument command: invalid choice: {} (choose from fetch, build, check, sync, diff)",
                    py::repr_str(&arg)
                ),
            ));
        };
        let sub = parse_sub(spec, &args[i + 1..], &mut extras)?;
        if !extras.is_empty() {
            let text: Vec<String> = extras
                .iter()
                .map(|e| e.to_string_lossy().into_owned())
                .collect();
            return Err(error(
                TOP_USAGE,
                "vendor.py",
                format!("unrecognized arguments: {}", text.join(" ")),
            ));
        }
        return Ok(Args {
            root,
            cache,
            command: spec.name,
            sub,
        });
    }
    Err(error(
        TOP_USAGE,
        "vendor.py",
        "the following arguments are required: command",
    ))
}

/// `Path(arg)`: an empty argument is `.`.
fn path_arg(arg: &OsStr) -> PathBuf {
    if arg.is_empty() {
        PathBuf::from(".")
    } else {
        PathBuf::from(arg)
    }
}

/// `Path.resolve()` (non-strict): canonical where it exists, else absolute.
fn resolve(path: &Path) -> PathBuf {
    std::fs::canonicalize(path)
        .or_else(|_| std::path::absolute(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

fn lossy(s: &OsStr) -> String {
    s.to_string_lossy().into_owned()
}

/// `_names(root, requested)`: every vendored source, or the requested ones (checked).
fn names(root: &Path, requested: Vec<String>) -> Result<Vec<String>> {
    let entries = load_sources(root)?;
    let built = vendored(&entries)?;
    if requested.is_empty() {
        return Ok(built.iter().map(|e| py::str_of(e.get("name"))).collect());
    }
    // `find_entry` over the vendored entries only: a subtree source is unknown here.
    let built: Vec<Value> = built
        .into_iter()
        .map(|e| Value::Object(e.clone()))
        .collect();
    for n in &requested {
        find_entry(&built, n)?;
    }
    Ok(requested)
}

/// `main(argv)`: parse, run one command; a VendorError is `vendor.py: <message>`, exit 1.
pub fn main(args: &[OsString]) -> Output {
    let parsed = match parse(args) {
        Ok(p) => p,
        Err(Exit::Help(text)) => {
            return Output {
                stdout: text.as_bytes().to_vec(),
                ..Output::default()
            };
        }
        Err(Exit::Error(usage, prog, msg)) => {
            return Output {
                stderr: format!("{usage}{prog}: error: {msg}\n").into_bytes(),
                code: 2,
                ..Output::default()
            };
        }
    };
    let root = resolve(
        &parsed
            .root
            .as_deref()
            .map_or_else(|| PathBuf::from("."), path_arg),
    );
    let cache = parsed
        .cache
        .as_deref()
        .map_or_else(|| root.join(CACHE_DIRNAME), path_arg);
    let fetch = dispatch_fetcher(&root, Box::new(git_fetcher(cache)));
    let mut out = Output::default();
    if let Err(e) = run_command(&root, &parsed, &*fetch, &mut out) {
        out.stderr
            .extend(format!("vendor.py: {}\n", e.0).into_bytes());
        out.code = 1;
    }
    out
}

fn say(buf: &mut Vec<u8>, line: impl AsRef<str>) {
    buf.extend_from_slice(line.as_ref().as_bytes());
    buf.push(b'\n');
}

fn run_command(root: &Path, args: &Args, fetch: &Fetch, out: &mut Output) -> Result<()> {
    let sub = &args.sub;
    let first = |dest: &str| sub.positional(dest).first().map(|s| lossy(s));
    match args.command {
        "fetch" => {
            let requested = sub.positional("names").iter().map(|s| lossy(s)).collect();
            let names = names(root, requested)?;
            for n in &names {
                let entries = load_sources(root)?;
                let path = fetch_entry(root, find_entry(&entries, n)?, Some(fetch), None)?;
                say(&mut out.stdout, format!("fetched {n}: {}", path.display()));
            }
            for path in fetch_licenses(root, Some(&names), Some(fetch))? {
                let rel = path.strip_prefix(root).unwrap_or(&path);
                say(&mut out.stdout, format!("license {}", rel.display()));
            }
        }
        "build" => {
            let requested = first("name").into_iter().collect();
            let dest = sub.value("out").map(|o| path_arg(o));
            for n in names(root, requested)? {
                let stats = build(root, &n, dest.as_deref(), Some(fetch), false)?;
                say(&mut out.stdout, format!("built {n} ({})", stats.detail()));
            }
        }
        "check" => {
            let problems = check(root, Some(fetch))?;
            for line in &problems {
                say(&mut out.stderr, line);
            }
            if problems.is_empty() {
                say(&mut out.stdout, "vendor check: clean");
            } else {
                say(
                    &mut out.stdout,
                    format!("vendor check: {} problem(s)", problems.len()),
                );
                out.code = 1;
            }
        }
        "sync" => {
            let name = first("name").unwrap_or_default();
            let git_ref = sub.value("ref").map(|r| lossy(r)).unwrap_or_default();
            let report = sync(root, &name, &git_ref, Some(fetch), None)?;
            say(
                &mut out.stdout,
                format!(
                    "re-pinned {name}: {} -> {git_ref} ({})",
                    py::str_of(Some(&report.old_ref)),
                    report.new_commit
                ),
            );
            for r in &report.rejects {
                say(&mut out.stderr, format!("REJECT {r}"));
            }
            for r in &report.portmap {
                say(&mut out.stderr, format!("PORTMAP {r}"));
            }
            say(
                &mut out.stdout,
                "Rebuilt the target and refreshed the license. Nothing was committed.\n\
                 Review the diff; if the output changed, bump plugin_version in sources.json \
                 where it applies.",
            );
            if !report.rejects.is_empty() || !report.portmap.is_empty() {
                out.code = 1;
            }
        }
        "diff" => {
            let name = first("name").unwrap_or_default();
            out.stdout
                .extend(diff_entry(root, &name, Some(fetch))?.into_bytes());
        }
        "set-rtk-upstream" => {
            let get = |d: &str| sub.value(d).map(|v| lossy(v)).unwrap_or_default();
            let old = set_rtk_upstream(root, &get("ref"), &get("commit"))?;
            say(&mut out.stdout, old);
        }
        other => return Err(VendorError(format!("unknown command {other}"))),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anthology_pycompat::json;

    /// `[argv, ["exit", code, stdout, stderr]]` or `[argv, ["ns", {dest: value}]]` per line:
    /// vendor.py's parser on `python:3.12-slim` (`python3 -I`, COLUMNS=80).
    const CASES: &str = include_str!("../../tests/vendor/data/argparse-3.12.jsonl");

    fn text(v: &Value) -> String {
        v.as_str()
            .map(str::to_string)
            .unwrap_or_else(|| py::str_of(Some(v)))
    }

    /// The parsed namespace as Python's oracle renders it (`str(Path)` for paths).
    fn namespace(a: &Args) -> Vec<(String, String)> {
        let path = |v: Option<&OsString>| match v {
            None => Value::Null,
            Some(v) => Value::Str(path_arg(v).display().to_string()),
        };
        let mut ns = vec![
            ("cache".to_string(), path(a.cache.as_ref())),
            ("command".to_string(), Value::Str(a.command.to_string())),
        ];
        let spec = SPECS.iter().find(|s| s.name == a.command).unwrap();
        for (opt, dest, _) in spec.options {
            let value = a.sub.value(dest);
            let v = if *opt == "--out" {
                path(value)
            } else {
                value.map_or(Value::Null, |v| Value::Str(lossy(v)))
            };
            ns.push((dest.to_string(), v));
        }
        for (dest, nargs) in spec.positionals {
            let got = a.sub.positionals.iter().find(|(d, _)| d == dest);
            let v = match (nargs, got) {
                (Nargs::Star, g) => Value::Array(
                    g.map(|(_, v)| v.iter().map(|s| Value::Str(lossy(s))).collect())
                        .unwrap_or_default(),
                ),
                (_, Some((_, v))) => Value::Str(lossy(&v[0])),
                (_, None) => Value::Null,
            };
            ns.push((dest.to_string(), v));
        }
        let mut out: Vec<(String, String)> = ns
            .into_iter()
            .map(|(k, v)| (k, json::dumps(&v, &json::INDENT2)))
            .collect();
        out.sort();
        out
    }

    #[test]
    fn parser_matches_argparse_312() {
        let mut failures = Vec::new();
        for line in CASES.lines().filter(|l| !l.trim().is_empty()) {
            let row = json::loads(line).unwrap();
            let row = row.as_array().unwrap();
            let argv: Vec<OsString> = row[0]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| OsString::from(text(a)))
                .collect();
            let want = row[1].as_array().unwrap();
            if text(&want[0]) == "ns" {
                let mut expected: Vec<(String, String)> = want[1]
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(k, v)| (k.to_string(), json::dumps(v, &json::INDENT2)))
                    .collect();
                expected.sort();
                match parse(&argv) {
                    Ok(a) if namespace(&a) == expected => {}
                    Ok(a) => {
                        failures.push(format!("{argv:?}: {:?} != {expected:?}", namespace(&a)))
                    }
                    Err(_) => failures.push(format!("{argv:?}: parse failed, Python parsed")),
                }
            } else {
                let out = main(&argv);
                let got = (
                    out.code.to_string(),
                    String::from_utf8_lossy(&out.stdout).into_owned(),
                    String::from_utf8_lossy(&out.stderr).into_owned(),
                );
                let expected = (text(&want[1]), text(&want[2]), text(&want[3]));
                if got != expected {
                    failures.push(format!("{argv:?}: {got:?} != {expected:?}"));
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}
