//! `pycompat::argparse` against CPython 3.12's `argparse` (expected values from the pinned
//! image, `COLUMNS=80`). The `pycompat.argparse` difftest surface covers the rest; these are
//! the cases it cannot send (bytes that are not UTF-8) and a readable sample.

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;

use anthology_pycompat::argparse::{self, Arg, Spec, Subcommands, Type, UsageError, Value};
use anthology_pycompat::json::Int;

fn tool() -> Spec {
    Spec::new("tool")
        .description("Do things.")
        .arg(Arg::positional("src"))
        .arg(Arg::option(&["--mode"]).choices(&["a", "b"]))
        .arg(
            Arg::option(&["-n", "--num"])
                .ty(Type::Int)
                .default(Value::str("3")),
        )
        .arg(Arg::option(&["--path"]).ty(Type::Path))
        .arg(
            Arg::option(&["--tag"])
                .append()
                .default(Value::List(Vec::new())),
        )
        .arg(Arg::option(&["-v"]).store_true())
}

const TOOL_USAGE: &str =
    "usage: tool [-h] [--mode {a,b}] [-n NUM] [--path PATH] [--tag TAG] [-v] src\n";

fn args(v: &[&str]) -> Vec<OsString> {
    v.iter().map(OsString::from).collect()
}

fn err(code: i32, stdout: &str, stderr: &str) -> UsageError {
    UsageError {
        code,
        stdout: stdout.into(),
        stderr: stderr.into(),
    }
}

#[test]
fn values_by_type() {
    let p = argparse::parse(
        &tool(),
        &args(&[
            "x", "-n", "\u{663}", "--path", "a//b/", "--tag", "t1", "--tag=t2", "-vn5",
        ]),
    )
    .unwrap();
    assert_eq!(p.str("src"), Some("x"));
    assert!(p.get("mode").unwrap().is_none());
    assert_eq!(p.i64("num"), Some(5));
    assert_eq!(p.str("path"), Some("a/b"));
    assert_eq!(p.strs("tag"), ["t1", "t2"]);
    assert!(p.flag("v"));
    let order: Vec<&str> = p.values.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(order, ["src", "mode", "num", "path", "tag", "v"]);
}

#[test]
fn str_default_converted_by_type() {
    let p = argparse::parse(&tool(), &args(&["x"])).unwrap();
    assert_eq!(p.get("num"), Some(&Value::Int(Int::Small(3))));
    assert!(p.get("path").unwrap().is_none());
    assert_eq!(p.list("tag"), &[] as &[Value]);
    assert!(!p.flag("v"));
}

#[test]
fn errors_are_python_text() {
    assert_eq!(
        argparse::parse(&tool(), &args(&[])).unwrap_err(),
        err(
            2,
            "",
            &format!("{TOOL_USAGE}tool: error: the following arguments are required: src\n")
        )
    );
    assert_eq!(
        argparse::parse(&tool(), &args(&["x", "--m", "a", "--nu", "x"])).unwrap_err(),
        err(
            2,
            "",
            &format!("{TOOL_USAGE}tool: error: argument -n/--num: invalid int value: 'x'\n")
        )
    );
}

#[test]
fn bytes_that_are_not_utf8() {
    let bad = |b: &[u8]| OsString::from_vec(b.to_vec());
    assert_eq!(
        argparse::parse(&tool(), &[bad(b"x"), bad(b"--mode"), bad(b"\xff")]).unwrap_err(),
        err(
            2,
            "",
            &format!(
                "{TOOL_USAGE}tool: error: argument --mode: invalid choice: '\\udcff' (choose \
                 from a, b)\n"
            )
        )
    );
    assert_eq!(
        argparse::parse(&tool(), &[bad(b"x"), bad(b"\xfe")]).unwrap_err(),
        err(
            2,
            "",
            &format!("{TOOL_USAGE}tool: error: unrecognized arguments: \\udcfe\n")
        )
    );
    let p = argparse::parse(&tool(), &[bad(b"a\xffb")]).unwrap();
    assert_eq!(p.os_str("src").unwrap().len(), 3);
    assert_eq!(p.str("src"), None);
}

#[test]
fn help() {
    assert_eq!(
        argparse::parse(&tool(), &args(&["-h"])).unwrap_err(),
        err(
            0,
            &format!(
                "{TOOL_USAGE}\nDo things.\n\npositional arguments:\n  src\n\noptions:\n  -h, \
                 --help         show this help message and exit\n  --mode {{a,b}}\n  -n NUM, \
                 --num NUM\n  --path PATH\n  --tag TAG\n  -v\n"
            ),
            ""
        )
    );
}

fn top() -> Spec {
    Spec::new("top").subcommands(Subcommands::new().dest("cmd").required().parser(
        "run",
        Some("run it"),
        Spec::command().arg(Arg::option(&["--fast"]).store_true()),
    ))
}

#[test]
fn subcommands() {
    let p = argparse::parse(&top(), &args(&["run", "--fast"])).unwrap();
    assert_eq!(p.str("cmd"), Some("run"));
    assert!(p.flag("fast"));
    let usage = "usage: top [-h] {run} ...\n";
    assert_eq!(
        argparse::parse(&top(), &args(&["run", "--slow"])).unwrap_err(),
        err(
            2,
            "",
            &format!("{usage}top: error: unrecognized arguments: --slow\n")
        )
    );
    assert_eq!(
        argparse::parse(&top(), &args(&["run", "--fast=1"])).unwrap_err(),
        err(
            2,
            "",
            "usage: top run [-h] [--fast]\ntop run: error: argument --fast: ignored explicit \
             argument '1'\n"
        )
    );
    assert_eq!(
        argparse::parse(&top(), &args(&["walk"])).unwrap_err(),
        err(
            2,
            "",
            &format!("{usage}top: error: argument cmd: invalid choice: 'walk' (choose from run)\n")
        )
    );
    assert_eq!(
        argparse::parse(&top(), &args(&["-h"])).unwrap_err(),
        err(
            0,
            &format!(
                "{usage}\npositional arguments:\n  {{run}}\n    run       run it\n\noptions:\n  \
                 -h, --help  show this help message and exit\n"
            ),
            ""
        )
    );
}

#[test]
fn help_width_reads_columns() {
    assert_eq!(argparse::help_width(None, None), 78);
    assert_eq!(argparse::help_width(Some("120"), Some(50)), 118);
    assert_eq!(argparse::help_width(Some(" 40 "), None), 38);
    assert_eq!(argparse::help_width(Some("0"), Some(50)), 48);
    assert_eq!(argparse::help_width(Some("x"), None), 78);
}
