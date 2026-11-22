//! `anthology-dev eval-baseline record|compare|candidate` (scripts/eval_baseline.py). Owner: DV2.
//!
//! Baseline gate for the behaviour evals: record scores, then fail on a regression. Exit codes
//! of `compare` (GC-5): 0 no regression, 1 regression, 2 invalid or partial result (or an
//! invalid baseline), 3 baseline file missing. `record` exits 2 when it refuses the result.

use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anthology_core::Env;
use anthology_pycompat::argparse::{self, Arg, Subcommands, Type, Value as ArgValue};
use anthology_pycompat::json::{self, INDENT2_SORTED, Int, Map, Value};
use anthology_pycompat::pyerr::PyExc;
use regex::Regex;

use crate::Output;
use crate::json_edit::{Uncaught, py_repr, str_repr, utf8_decode_error};
use anthology_pycompat::str as pystr;

static CASE_NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\A[a-z0-9]+(?:-[a-z0-9]+)*\z").expect("CASE_NAME_RE"));
pub const EPSILON: f64 = 1e-9;

/// `NO_BASELINE.format(path=...)`.
pub fn no_baseline(path: &str) -> String {
    format!(
        "no eval baseline recorded at {path}: run scripts/run-evals.sh --record on a full run and \
         commit evals/baseline.json"
    )
}

/// Why a command stopped: `InvalidResult` (exit 2) or an exception Python does not catch (R10).
#[derive(Debug)]
pub enum Failure {
    Invalid(String),
    Uncaught(Uncaught),
}

type Res<T> = Result<T, Failure>;

fn invalid<T>(msg: impl Into<String>) -> Res<T> {
    Err(Failure::Invalid(msg.into()))
}

/// Python's `str(value)` for a JSON value.
pub fn py_str(v: &Value) -> String {
    match v {
        Value::Str(s) => s.clone(),
        other => py_repr(other),
    }
}

/// `_clean(value, limit)`: a printable one-line rendering of untrusted text.
pub fn clean_text(text: &str, limit: usize) -> String {
    let printable: String = text
        .chars()
        .map(|c| if pystr::is_printable(c) { c } else { ' ' })
        .collect();
    let text = pystr::split_ws(&printable).join(" ");
    if text.chars().count() <= limit {
        text
    } else {
        let cut: String = text.chars().take(limit).collect();
        cut + "..."
    }
}

/// `_clean(value, limit)` for a JSON value.
pub fn clean(v: &Value, limit: usize) -> String {
    clean_text(&py_str(v), limit)
}

/// `_is_number`: an int or a float, not a bool.
pub fn is_number(v: &Value) -> bool {
    matches!(v, Value::Int(_) | Value::Float(_))
}

/// `float(v)` for a number: an int converts with correct rounding; one beyond the float range
/// is Python's uncaught `OverflowError`.
pub fn to_float(v: &Value) -> Res<f64> {
    match v {
        Value::Float(f) => Ok(*f),
        Value::Int(Int::Small(n)) => Ok(*n as f64),
        Value::Int(Int::Big(s)) => match s.parse::<f64>() {
            Ok(f) if f.is_finite() => Ok(f),
            _ => Err(Failure::Uncaught(Uncaught::new(
                "OverflowError",
                "int too large to convert to float",
            ))),
        },
        _ => unreachable!("to_float takes a number"),
    }
}

/// Python truthiness of a JSON value (`x or default`).
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Int(Int::Small(n)) => *n != 0,
        Value::Int(Int::Big(_)) => true,
        Value::Float(f) => *f != 0.0,
        Value::Str(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(m) => !m.is_empty(),
    }
}

/// Python's `a == b` for two separately parsed JSON values: `1 == 1.0 == True`, a top-level NaN
/// is unequal to everything (a NaN inside a list or dict is not), dicts ignore order.
pub fn py_eq(a: &Value, b: &Value) -> bool {
    eq_in(a, b, false)
}

/// `nested`: inside a list or dict, where Python compares by identity first, and every NaN
/// `json.loads` returns is its one `NaN` constant, so `[NaN] == [NaN]`.
fn eq_in(a: &Value, b: &Value, nested: bool) -> bool {
    fn num(v: &Value) -> Option<NumKey<'_>> {
        match v {
            Value::Bool(b) => Some(NumKey::Int(i64::from(*b))),
            Value::Int(Int::Small(n)) => Some(NumKey::Int(*n)),
            Value::Int(Int::Big(s)) => Some(NumKey::Big(s)),
            Value::Float(f) => Some(NumKey::Float(*f)),
            _ => None,
        }
    }
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| eq_in(p, q, true))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| eq_in(v, w, true)))
        }
        (Value::Float(x), Value::Float(y)) if nested && x.is_nan() && y.is_nan() => true,
        _ => match (num(a), num(b)) {
            (Some(x), Some(y)) => x.eq(&y),
            _ => false,
        },
    }
}

enum NumKey<'a> {
    Int(i64),
    Big(&'a str),
    Float(f64),
}

impl NumKey<'_> {
    fn eq(&self, other: &NumKey<'_>) -> bool {
        match (self, other) {
            (NumKey::Int(a), NumKey::Int(b)) => a == b,
            (NumKey::Big(a), NumKey::Big(b)) => a == b,
            (NumKey::Float(a), NumKey::Float(b)) => a == b,
            (NumKey::Int(i), NumKey::Float(f)) | (NumKey::Float(f), NumKey::Int(i)) => {
                int_eq_float(&i.to_string(), *f)
            }
            (NumKey::Big(s), NumKey::Float(f)) | (NumKey::Float(f), NumKey::Big(s)) => {
                int_eq_float(s, *f)
            }
            _ => false, // a Big is outside the i64 range by construction
        }
    }
}

/// Exact `int == float`: the float must be integral and its exact decimal equal the int.
fn int_eq_float(int: &str, f: f64) -> bool {
    if !f.is_finite() || f.fract() != 0.0 {
        return false;
    }
    let exact = format!("{f:.0}");
    let exact = if exact == "-0" {
        "0".to_string()
    } else {
        exact
    };
    exact == int
}

/// `_fmt(score)`: `-`, or `f"{score:.3f}"`.
pub fn fmt_score(score: Option<f64>) -> String {
    match score {
        None => "-".to_string(),
        Some(s) if s.is_nan() => "nan".to_string(),
        Some(s) => format!("{s:.3}"),
    }
}

/// `open(path, encoding="utf-8")` + `json.load`, with `_read_json`'s messages.
fn read_json(cwd: &Path, path: &OsStr, what: &str) -> Res<Value> {
    let shown = clean_text(&path.to_string_lossy(), 200);
    let bytes = match open_read(cwd, path) {
        Ok(b) => b,
        Err(e) => {
            let errno = PyExc::from(e);
            let strerror = strerror(&errno);
            return invalid(format!(
                "cannot read the {what} {shown}: {}",
                clean_text(&strerror, 200)
            ));
        }
    };
    if utf8_decode_error(&bytes).is_some() {
        return invalid(format!("the {what} {shown} is not valid JSON"));
    }
    let text = String::from_utf8(bytes).expect("checked");
    json::loads(&text).or_else(|_| invalid(format!("the {what} {shown} is not valid JSON")))
}

/// `open(path)` then read: an empty path is `ENOENT`, as `open('')` is.
fn open_read(cwd: &Path, path: &OsStr) -> std::io::Result<Vec<u8>> {
    if path.is_empty() {
        return Err(std::io::Error::from_raw_os_error(2));
    }
    let full = cwd.join(path);
    let mut f = std::fs::File::open(&full)?;
    let mut b = Vec::new();
    f.read_to_end(&mut b)?;
    Ok(b)
}

/// `exc.strerror` (ruling R3): `py_str` without a filename, minus `[Errno N] `.
fn strerror(e: &PyExc) -> String {
    let text = match e {
        PyExc::OSError { errno, .. } => PyExc::OSError {
            errno: *errno,
            filename: None,
        }
        .py_str(),
        other => other.py_str(),
    };
    match text.strip_prefix("[Errno ") {
        Some(rest) => rest
            .split_once("] ")
            .map_or(text.clone(), |(_, t)| t.to_string()),
        None => text,
    }
}

fn os_uncaught(e: std::io::Error, path: &OsStr) -> Failure {
    let mut exc = PyExc::from(e);
    if let PyExc::OSError { filename, .. } = &mut exc {
        *filename = Some(path.to_string_lossy().into_owned());
    }
    Failure::Uncaught(Uncaught::from_exc(&exc))
}

/// `read_result`.
pub fn read_result(cwd: &Path, path: &OsStr) -> Res<Map> {
    match read_json(cwd, path, "result")?.into_object() {
        Some(m) => Ok(m),
        None => invalid("the result is not a JSON object"),
    }
}

static NULL: Value = Value::Null;

fn get<'a>(m: &'a Map, k: &str) -> &'a Value {
    m.get(k).unwrap_or(&NULL)
}

/// `result_scores(result, strict)`: case name → score, in result order.
pub fn result_scores(result: &Map, strict: bool) -> Res<Vec<(String, f64)>> {
    let Some(cases) = get(result, "cases").as_array() else {
        return invalid("the result has no cases list");
    };
    let mut scores: Vec<(String, f64)> = Vec::new();
    for case in cases {
        let name = case.as_object().map_or(&NULL, |c| get(c, "name"));
        let name = match name.as_str() {
            Some(n) if CASE_NAME_RE.is_match(n) => n.to_string(),
            _ => {
                return invalid(format!(
                    "a case name in the result is not a valid case name: {}",
                    str_repr(&clean(name, 60))
                ));
            }
        };
        if scores.iter().any(|(n, _)| *n == name) {
            return invalid(format!("case {name} appears twice in the result"));
        }
        let aggregates = case.as_object().map_or(&NULL, |c| get(c, "aggregates"));
        let score = aggregates.as_object().map_or(&NULL, |a| get(a, "score"));
        if !is_number(score) {
            if strict {
                return invalid(format!("case {name} has no numeric aggregates.score"));
            }
            continue;
        }
        scores.push((name, to_float(score)?));
    }
    Ok(scores)
}

fn clone_value(v: &Value) -> Value {
    json::loads(&json::dumps(v, &json::PY_DEFAULT_UNICODE)).expect("round trip")
}

/// `build_baseline(result, scores)` (with `candidate` set for `cmd_candidate`).
pub fn build_baseline(result: &Map, scores: &[(String, f64)], candidate: bool) -> Value {
    let recorded_at = match get(result, "startedAt") {
        Value::Str(s) => Value::Str(clean_text(s.split('T').next().unwrap_or(""), 10)),
        _ => Value::Null,
    };
    let mut cases = Map::new();
    for (name, score) in scores {
        cases.insert(name.clone(), Value::Float(*score));
    }
    let mut m = Map::new();
    m.insert("schema_version".into(), Value::Int(Int::Small(1)));
    m.insert(
        "claude_code_version".into(),
        clone_value(get(result, "claudeVersion")),
    );
    m.insert("recorded_at".into(), recorded_at);
    m.insert("cost_usd".into(), clone_value(get(result, "costUsd")));
    m.insert("cases".into(), Value::Object(cases));
    if candidate {
        m.insert("candidate".into(), Value::Bool(true));
    }
    Value::Object(m)
}

/// `_write_json(path, data)`: `indent=2, sort_keys=True` + newline; an error is uncaught.
fn write_json(cwd: &Path, path: &OsStr, data: &Value) -> Res<()> {
    let text = json::dumps(data, &INDENT2_SORTED) + "\n";
    if path.is_empty() {
        return Err(os_uncaught(std::io::Error::from_raw_os_error(2), path));
    }
    std::fs::write(cwd.join(path), text).map_err(|e| os_uncaught(e, path))
}

/// `check_complete(result)`: the scores of a full run.
pub fn check_complete(result: &Map) -> Res<Vec<(String, f64)>> {
    if matches!(get(result, "partial"), Value::Bool(true)) {
        let reason = get(result, "partialReason");
        let reason = if truthy(reason) {
            clean(reason, 300)
        } else {
            clean_text("no reason given", 300)
        };
        return invalid(format!("the result is partial: {reason}"));
    }
    let scores = result_scores(result, true)?;
    if scores.is_empty() {
        return invalid("the result has no cases");
    }
    Ok(scores)
}

/// `cmd_record`.
fn cmd_record(cwd: &Path, result: &OsStr, out: &OsStr) -> Res<(i32, String)> {
    let r = read_result(cwd, result)?;
    let scores = check_complete(&r)?;
    write_json(cwd, out, &build_baseline(&r, &scores, false))?;
    Ok((
        0,
        format!(
            "recorded {} cases in {}\n",
            scores.len(),
            out.to_string_lossy()
        ),
    ))
}

/// `cmd_candidate`.
fn cmd_candidate(cwd: &Path, result: &OsStr, out: &OsStr) -> Res<(i32, String)> {
    let r = read_result(cwd, result)?;
    let scores = result_scores(&r, false)?;
    write_json(cwd, out, &build_baseline(&r, &scores, true))?;
    Ok((
        0,
        format!(
            "wrote a candidate baseline with {} cases to {}\n",
            scores.len(),
            out.to_string_lossy()
        ),
    ))
}

/// `_read_baseline(path)`: the baseline's `cases` as (name, score), and its Claude Code version.
fn read_baseline(cwd: &Path, path: &OsStr) -> Res<(Vec<(String, f64)>, Value)> {
    let shown = clean_text(&path.to_string_lossy(), 200);
    let mut data = read_json(cwd, path, "baseline")?;
    let Some(obj) = data.as_object() else {
        return invalid(format!("the baseline {shown} has no cases object"));
    };
    let Some(cases) = get(obj, "cases").as_object() else {
        return invalid(format!("the baseline {shown} has no cases object"));
    };
    for (name, score) in cases.iter() {
        if !CASE_NAME_RE.is_match(name) || !is_number(score) {
            return invalid(format!(
                "the baseline {shown} has an invalid entry: {}",
                str_repr(&clean_text(name, 60))
            ));
        }
    }
    let mut base = Vec::new();
    for (name, score) in cases.iter() {
        base.push((name.to_string(), to_float(score)?));
    }
    let version = data
        .as_object_mut()
        .and_then(|m| m.remove("claude_code_version"))
        .unwrap_or(Value::Null);
    Ok((base, version))
}

/// The `compare` options.
pub struct CompareOpts<'a> {
    pub tolerance: f64,
    pub allow_missing: bool,
    pub summary: Option<&'a OsStr>,
}

/// `cmd_compare`: (exit code, stdout, stderr).
fn cmd_compare(
    cwd: &Path,
    result: &OsStr,
    baseline: &OsStr,
    o: &CompareOpts<'_>,
    stdout: &mut String,
) -> Res<(i32, String)> {
    let r = read_result(cwd, result)?;
    let now = check_complete(&r)?;
    // `open(args.baseline)` only (a pipe is not drained): ENOENT is "missing".
    let opened = if baseline.is_empty() {
        Err(std::io::Error::from_raw_os_error(2))
    } else {
        std::fs::File::open(cwd.join(baseline)).map(drop)
    };
    if let Err(e) = opened
        && e.raw_os_error() == Some(2)
    {
        return Ok((3, format!("{}\n", no_baseline(&baseline.to_string_lossy()))));
    }
    let (mut base, old_version) = read_baseline(cwd, baseline)?;
    base.sort_by(|a, b| a.0.cmp(&b.0));
    let now_of = |name: &str| now.iter().find(|(n, _)| n == name).map(|&(_, s)| s);

    let mut rows: Vec<(String, Option<f64>, Option<f64>, &str)> = Vec::new();
    let mut regressions = 0;
    for (name, was) in &base {
        match now_of(name) {
            None if o.allow_missing => rows.push((name.clone(), Some(*was), None, "not run")),
            None => {
                rows.push((name.clone(), Some(*was), None, "MISSING"));
                regressions += 1;
            }
            Some(is_now) if is_now < *was - o.tolerance - EPSILON => {
                rows.push((name.clone(), Some(*was), Some(is_now), "REGRESSION"));
                regressions += 1;
            }
            Some(is_now) => rows.push((name.clone(), Some(*was), Some(is_now), "ok")),
        }
    }
    let mut new: Vec<&(String, f64)> = now
        .iter()
        .filter(|(n, _)| !base.iter().any(|(b, _)| b == n))
        .collect();
    new.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, score) in new {
        rows.push((name.clone(), None, Some(*score), "new"));
    }

    for (name, was, is_now, status) in &rows {
        stdout.push_str(&format!(
            "{status:10} {name}: baseline {}, now {}\n",
            fmt_score(*was),
            fmt_score(*is_now)
        ));
    }
    let new_version = get(&r, "claudeVersion");
    if !py_eq(&old_version, new_version) {
        stdout.push_str(&format!(
            "note: Claude Code {} differs from the baseline's {}; this alone is not a failure\n",
            clean(new_version, 40),
            clean(&old_version, 40)
        ));
    }
    stdout.push_str(&format!(
        "{regressions} regression(s) across {} baseline case(s), {} case(s) scored\n",
        base.len(),
        now.len()
    ));

    if let Some(summary) = o.summary.filter(|s| !s.is_empty()) {
        let mut lines = vec![
            "### Behaviour eval baseline".to_string(),
            String::new(),
            "| case | baseline | now | status |".to_string(),
            "| --- | --- | --- | --- |".to_string(),
        ];
        for (name, was, is_now, status) in &rows {
            lines.push(format!(
                "| {name} | {} | {} | {status} |",
                fmt_score(*was),
                fmt_score(*is_now)
            ));
        }
        lines.push(String::new());
        let text = lines.join("\n") + "\n";
        std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(cwd.join(summary))
            .and_then(|mut f| f.write_all(text.as_bytes()))
            .map_err(|e| os_uncaught(e, summary))?;
    }
    Ok((i32::from(regressions > 0), String::new()))
}

/// `main`'s `ArgumentParser` and its subparsers (`prog` is the script's name, as `sys.argv[0]`
/// gives it). `COLUMNS` sets the help width, as `shutil.get_terminal_size` reads it.
fn spec(env: &Env) -> argparse::Spec {
    let subcommands = Subcommands::new()
        .dest("command")
        .required()
        .parser(
            "record",
            Some("write the baseline from a full run"),
            argparse::Spec::command()
                .arg(Arg::positional("result"))
                .arg(Arg::positional("out").metavar("baseline")),
        )
        .parser(
            "candidate",
            Some("write a candidate baseline, partial runs allowed"),
            argparse::Spec::command()
                .arg(Arg::positional("result"))
                .arg(Arg::positional("out")),
        )
        .parser(
            "compare",
            Some("compare a run with the baseline"),
            argparse::Spec::command()
                .arg(Arg::positional("result"))
                .arg(Arg::positional("baseline"))
                .arg(
                    Arg::option(&["--tolerance"])
                        .ty(Type::Float)
                        .default(ArgValue::Float(0.0)),
                )
                .arg(
                    Arg::option(&["--allow-missing"])
                        .store_true()
                        .help("a baseline case absent from the result is not a regression"),
                )
                .arg(
                    Arg::option(&["--summary"])
                        .metavar("PATH")
                        .help("append a Markdown table to PATH"),
                ),
        );
    argparse::Spec::new("eval_baseline.py")
        .description(
            "Baseline gate for the behaviour evals: record scores, then fail on a regression.",
        )
        .width(argparse::help_width(env.get("COLUMNS"), None))
        .subcommands(subcommands)
}

/// Runs `anthology-dev eval-baseline …`; `args` are the arguments after `eval-baseline`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    run_in(args, stdin, &Env::from_process(), &cwd)
}

/// `eval_baseline.main(args)`, with relative paths resolved against `cwd`.
pub fn run_in(args: &[OsString], stdin: &mut dyn Read, env: &Env, cwd: &Path) -> Output {
    let _ = stdin;
    let parsed = match argparse::parse(&spec(env), args) {
        Ok(p) => p,
        Err(e) => {
            return Output {
                stdout: e.stdout.into_bytes(),
                stderr: e.stderr.into_bytes(),
                code: e.code,
            };
        }
    };
    let cmd = parsed.str("command").unwrap_or_default().to_string();
    let result = parsed.os_str("result").unwrap_or_default();
    // `record` and `candidate` write to `out`; `compare` reads `baseline`.
    let other = parsed
        .os_str("out")
        .or_else(|| parsed.os_str("baseline"))
        .unwrap_or_default();
    let (a, b) = (result, other);
    let mut stdout = String::new();
    let outcome = match cmd.as_str() {
        "record" => cmd_record(cwd, a, b),
        "candidate" => cmd_candidate(cwd, a, b),
        _ => {
            let opts = CompareOpts {
                tolerance: parsed.f64("tolerance").unwrap_or(0.0),
                allow_missing: parsed.flag("allow_missing"),
                summary: parsed.os_str("summary"),
            };
            cmd_compare(cwd, a, b, &opts, &mut stdout)
        }
    };
    match outcome {
        Ok((code, text)) if code == 3 => Output {
            stdout: stdout.into_bytes(),
            stderr: text.into_bytes(),
            code,
        },
        Ok((code, text)) => Output {
            stdout: (stdout + &text).into_bytes(),
            stderr: Vec::new(),
            code,
        },
        Err(Failure::Invalid(msg)) => Output {
            stdout: stdout.into_bytes(),
            stderr: format!("eval_baseline: {msg}\n").into_bytes(),
            code: 2,
        },
        Err(Failure::Uncaught(u)) => u.into_output("eval-baseline", stdout.into_bytes()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scores_format_as_python() {
        assert_eq!(fmt_score(Some(0.0625)), "0.062");
        assert_eq!(fmt_score(Some(0.1875)), "0.188");
        assert_eq!(fmt_score(Some(-0.0)), "-0.000");
        assert_eq!(fmt_score(Some(1e22)), "10000000000000000000000.000");
        assert_eq!(fmt_score(Some(f64::INFINITY)), "inf");
        assert_eq!(fmt_score(Some(f64::NEG_INFINITY)), "-inf");
        assert_eq!(fmt_score(Some(f64::NAN)), "nan");
        assert_eq!(fmt_score(Some(2.5e-4)), "0.000");
        assert_eq!(fmt_score(Some(1.0005)), "1.000");
        assert_eq!(fmt_score(None), "-");
    }

    #[test]
    fn clean_matches_python() {
        let v = json::loads(r#"{"a": [1, "x\ny"], "b": null}"#).expect("json");
        assert_eq!(clean(&v, 200), r"{'a': [1, 'x\ny'], 'b': None}");
        assert_eq!(clean_text("a\n\u{a0}b\t\tc", 200), "a b c");
        assert_eq!(clean_text("abcdef", 3), "abc...");
        assert_eq!(clean(&Value::Null, 60), "None");
    }

    #[test]
    fn python_equality() {
        let l = |t: &str| json::loads(t).expect("json");
        assert!(py_eq(&l("1"), &l("1.0")) && py_eq(&l("true"), &l("1")));
        assert!(py_eq(&l(r#"{"a": [1, 2]}"#), &l(r#"{"a": [1.0, 2]}"#)));
        assert!(!py_eq(&l("NaN"), &l("NaN")) && !py_eq(&l("\"1\""), &l("1")));
        assert!(py_eq(&l("[NaN]"), &l("[NaN]")) && py_eq(&l(r#"{"a": NaN}"#), &l(r#"{"a": NaN}"#)));
        assert!(!py_eq(&l("[NaN]"), &l("[1]")) && !py_eq(&l("NaN"), &l("1")));
        assert!(py_eq(
            &l("12345678901234567890"),
            &l("12345678901234567890")
        ));
        assert!(py_eq(&l("9007199254740993"), &l("9007199254740993")));
        assert!(!py_eq(&l("9007199254740993"), &l("9007199254740992.0")));
        assert!(py_eq(&l("null"), &Value::Null) && py_eq(&l("0"), &l("false")));
    }
}
