//! The `vendor` command line: vendor.py's `main` (argparse, then one command).
//!
//! The parser is `pycompat::argparse` (Python 3.12's argparse): the help and usage texts, the
//! matching and the errors are its, for the grammar `spec` builds (prog `vendor.py`).
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
use anthology_pycompat::argparse::{self, Arg, Nargs, Parsed, Spec, Subcommands, Type, UsageError};
use anthology_pycompat::json::Value;

/// The parser: vendor.py's `ArgumentParser`, built as `main` builds it (prog `vendor.py`).
///
/// `set-rtk-upstream` is the one subcommand vendor.py does not have (it replaces the inline
/// Python in `sync-rtk.sh`). `with_rtk` adds it, unlisted: it has no help line and the usage
/// keeps vendor.py's choices, so only a command line naming it ever sees it (see `parse`).
fn spec(with_rtk: bool) -> Spec {
    let name = || Arg::positional("name");
    let mut sub = Subcommands::new()
        .dest("command")
        .required()
        .parser(
            "fetch",
            Some("fetch pinned upstreams and copy their license files"),
            Spec::command().arg(Arg::positional("names").nargs(Nargs::ZeroOrMore)),
        )
        .parser(
            "build",
            Some("pristine -> transforms -> patch -> target"),
            Spec::command()
                .arg(
                    Arg::positional("name")
                        .nargs(Nargs::Optional)
                        .help("default: every source"),
                )
                .arg(
                    Arg::option(&["--out"])
                        .ty(Type::Path)
                        .help("write <out>/<target> instead of the repo"),
                ),
        )
        .parser(
            "check",
            Some("rebuild all into a temp dir and diff against the repo"),
            Spec::command(),
        )
        .parser(
            "sync",
            Some("re-pin to REF and rebuild; never commits"),
            Spec::command()
                .arg(name())
                .arg(Arg::option(&["--ref"]).required()),
        )
        .parser(
            "diff",
            Some("pristine+transforms vs shipped, as a patch"),
            Spec::command().arg(name()),
        );
    if with_rtk {
        sub = sub.metavar("{fetch,build,check,sync,diff}").parser(
            "set-rtk-upstream",
            None,
            Spec::command()
                .arg(
                    Arg::option(&["--ref"])
                        .required()
                        .help("the rtk upstream tag"),
                )
                .arg(
                    Arg::option(&["--commit"])
                        .required()
                        .help("the commit the tag points at"),
                ),
        );
    }
    Spec::new("vendor.py")
        .description(
            "Reproducible vendoring: every vendored tree is generated from a pinned\nupstream.",
        )
        .arg(Arg::option(&["--root"]).ty(Type::Path).help_suppress())
        .arg(
            Arg::option(&["--cache"])
                .ty(Type::Path)
                .help("pristine cache (default: <root>/.vendor-cache)"),
        )
        .subcommands(sub)
}

/// `parser.parse_args(args)`. A command line naming `set-rtk-upstream` is an invalid choice to
/// vendor.py's own grammar, so that one failure is parsed again by the grammar that has it.
fn parse(args: &[OsString]) -> std::result::Result<Parsed, UsageError> {
    match argparse::parse(&spec(false), args) {
        Err(e) if e.code == 2 && e.stderr.contains("invalid choice: 'set-rtk-upstream'") => {
            argparse::parse(&spec(true), args)
        }
        other => other,
    }
}

/// A `type=Path` argument to join onto: `Path(".") / x` is `x`, so `.` is the empty path.
fn join_base(path: &OsStr) -> PathBuf {
    if path == "." {
        PathBuf::new()
    } else {
        PathBuf::from(path)
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
        Err(e) => {
            return Output {
                stdout: e.stdout.into_bytes(),
                stderr: e.stderr.into_bytes(),
                code: e.code,
            };
        }
    };
    let root = resolve(Path::new(parsed.os_str("root").unwrap_or(OsStr::new("."))));
    let cache = parsed
        .os_str("cache")
        .map_or_else(|| root.join(CACHE_DIRNAME), join_base);
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

fn run_command(root: &Path, args: &Parsed, fetch: &Fetch, out: &mut Output) -> Result<()> {
    let text = |dest: &str| args.os_str(dest).map(lossy);
    match args.str("command").unwrap_or_default() {
        "fetch" => {
            let requested = args
                .list("names")
                .iter()
                .filter_map(|v| v.as_os_str().map(lossy))
                .collect();
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
            // `[args.name] if args.name else []`: an empty name is every source.
            let requested = text("name").filter(|n| !n.is_empty()).into_iter().collect();
            let dest = args.os_str("out").map(join_base);
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
            let name = text("name").unwrap_or_default();
            let git_ref = text("ref").unwrap_or_default();
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
            let name = text("name").unwrap_or_default();
            out.stdout
                .extend(diff_entry(root, &name, Some(fetch))?.into_bytes());
        }
        "set-rtk-upstream" => {
            let get = |d: &str| text(d).unwrap_or_default();
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

    /// The parsed namespace as Python's oracle renders it (`root` is not in the oracle's).
    fn namespace(a: &Parsed) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = a
            .values
            .iter()
            .filter(|(k, _)| k != "root")
            .map(|(k, v)| {
                let v = match v {
                    argparse::Value::None => Value::Null,
                    argparse::Value::List(l) => Value::Array(
                        l.iter()
                            .map(|x| Value::Str(x.as_os_str().map(lossy).unwrap_or_default()))
                            .collect(),
                    ),
                    other => Value::Str(other.as_os_str().map(lossy).unwrap_or_default()),
                };
                (k.clone(), json::dumps(&v, &json::INDENT2))
            })
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
