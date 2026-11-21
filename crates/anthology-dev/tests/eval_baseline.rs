//! Ports of tests/test_eval_baseline.py (DV2): one `mod` per Python class, one test per method.
//!
//! `run_cli` drives `eval_baseline::run_in`, as the Python drives `eval_baseline.main`.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use anthology_core::Env;
use anthology_dev::eval_baseline;
use anthology_pycompat::json::{self, Map, PY_DEFAULT, Value};

/// `make_result(scores, **extra)`, as JSON text pieces: `extra` replaces top-level keys.
fn make_result(scores: &[(&str, &str)], extra: &[(&str, &str)]) -> Value {
    let cases: Vec<String> = scores
        .iter()
        .map(|(n, s)| {
            format!(
                r#"{{"name": {}, "aggregates": {{"score": {s}, "passRate": 1.0}}}}"#,
                json::dumps(&Value::Str(n.to_string()), &PY_DEFAULT)
            )
        })
        .collect();
    let text = format!(
        r#"{{"claudeVersion": "2.1.311", "costUsd": 4.5, "startedAt": "2026-11-20T05:41:12.000Z",
            "partial": false, "partialReason": null, "aggregates": {{"casesTotal": {}}},
            "cases": [{}]}}"#,
        scores.len(),
        cases.join(", ")
    );
    let mut v = json::loads(&text).expect("result json");
    let m = v.as_object_mut().expect("object");
    for (k, raw) in extra {
        m.insert(k.to_string(), json::loads(raw).expect("extra json"));
    }
    v
}

struct Case {
    _tmp: tempfile::TempDir,
    dir: PathBuf,
}

impl Case {
    fn new() -> Case {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().to_path_buf();
        Case { _tmp: tmp, dir }
    }

    fn write_text(&self, name: &str, text: &str) -> String {
        let path = self.dir.join(name);
        fs::write(&path, text).expect("write");
        path.to_string_lossy().into_owned()
    }

    fn write(&self, name: &str, data: &Value) -> String {
        self.write_text(name, &json::dumps(data, &PY_DEFAULT))
    }

    fn path(&self, name: &str) -> String {
        self.dir.join(name).to_string_lossy().into_owned()
    }

    fn run_cli(&self, argv: &[&str]) -> (i32, String, String) {
        let args: Vec<OsString> = argv.iter().map(OsString::from).collect();
        let out = eval_baseline::run_in(
            &args,
            &mut std::io::empty(),
            &Env::from_process(),
            &self.dir,
        );
        (
            out.code,
            String::from_utf8(out.stdout).expect("utf-8"),
            String::from_utf8(out.stderr).expect("utf-8"),
        )
    }

    fn baseline(&self, scores: &[(&str, &str)], version: &str) -> String {
        let cases: Vec<String> = scores
            .iter()
            .map(|(n, s)| {
                format!(
                    "{}: {s}",
                    json::dumps(&Value::Str(n.to_string()), &PY_DEFAULT)
                )
            })
            .collect();
        self.write_text(
            "baseline.json",
            &format!(
                r#"{{"schema_version": 1, "claude_code_version": "{version}",
                    "recorded_at": "2026-11-13", "cost_usd": 1.0, "cases": {{{}}}}}"#,
                cases.join(", ")
            ),
        )
    }

    /// `CompareTests.compare(now, base, *extra, **kw)`.
    fn compare(
        &self,
        now: &[(&str, &str)],
        base: &[(&str, &str)],
        extra: &[&str],
        kw: &[(&str, &str)],
    ) -> (i32, String, String) {
        let result = self.write("result.json", &make_result(now, kw));
        let baseline = self.baseline(base, "2.1.311");
        let mut argv = vec!["compare", &result, &baseline];
        argv.extend_from_slice(extra);
        self.run_cli(&argv)
    }
}

fn read_json(path: &Path) -> Value {
    json::loads(&fs::read_to_string(path).expect("read")).expect("json")
}

fn obj(v: &Value) -> &Map {
    v.as_object().expect("object")
}

mod record_tests {
    use super::*;

    #[test]
    fn test_round_trip() {
        let c = Case::new();
        let result = c.write(
            "result.json",
            &make_result(&[("route-scout", "1.0"), ("guard-g1", "0.5")], &[]),
        );
        let out = c.path("baseline.json");
        let (code, _, _) = c.run_cli(&["record", &result, &out]);
        assert_eq!(code, 0);
        let text = fs::read_to_string(&out).expect("baseline");
        let want = json::loads(
            r#"{"schema_version": 1, "claude_code_version": "2.1.311", "recorded_at": "2026-11-20",
                "cost_usd": 4.5, "cases": {"guard-g1": 0.5, "route-scout": 1.0}}"#,
        )
        .expect("json");
        assert_eq!(json::loads(&text).expect("json"), want);
        assert_eq!(
            text,
            json::dumps(&json::loads(&text).expect("json"), &json::INDENT2_SORTED) + "\n"
        );
        // and the recorded file passes a compare against the same result
        let (code, _, _) = c.run_cli(&["compare", &result, &out]);
        assert_eq!(code, 0);
    }

    #[test]
    fn test_partial_is_refused() {
        let c = Case::new();
        let result = c.write(
            "result.json",
            &make_result(
                &[("a", "1.0")],
                &[
                    ("partial", "true"),
                    ("partialReason", r#""cost ceiling hit""#),
                ],
            ),
        );
        let out = c.path("baseline.json");
        let (code, _, err) = c.run_cli(&["record", &result, &out]);
        assert_eq!(code, 2);
        assert!(err.contains("cost ceiling hit"), "{err}");
        assert!(!Path::new(&out).exists());
    }

    #[test]
    fn test_zero_cases_refused() {
        let c = Case::new();
        let result = c.write("result.json", &make_result(&[], &[]));
        let (code, _, _) = c.run_cli(&["record", &result, &c.path("b.json")]);
        assert_eq!(code, 2);
    }

    #[test]
    fn test_case_without_score_refused() {
        let c = Case::new();
        let mut data = make_result(&[("a", "1.0"), ("b", "1.0")], &[]);
        let cases = data
            .as_object_mut()
            .and_then(|m| m.get_mut("cases"))
            .and_then(Value::as_array_mut)
            .expect("cases");
        cases[1].as_object_mut().expect("case").insert(
            "aggregates".into(),
            json::loads(r#"{"passRate": 1.0}"#).expect("json"),
        );
        let result = c.write("result.json", &data);
        let (code, _, err) = c.run_cli(&["record", &result, &c.path("b.json")]);
        assert_eq!(code, 2);
        assert!(err.contains('b'), "{err}");
    }

    #[test]
    fn test_non_numeric_score_refused() {
        let c = Case::new();
        let result = c.write("result.json", &make_result(&[("a", r#""1.0""#)], &[]));
        let (code, _, _) = c.run_cli(&["record", &result, &c.path("b.json")]);
        assert_eq!(code, 2);
    }

    #[test]
    fn test_unreadable_result_refused() {
        let c = Case::new();
        let (code, _, _) = c.run_cli(&["record", &c.path("nope.json"), &c.path("b.json")]);
        assert_eq!(code, 2);
        let bad = c.write_text("bad.json", "{not json");
        let (code, _, _) = c.run_cli(&["record", &bad, &c.path("b.json")]);
        assert_eq!(code, 2);
    }
}

mod candidate_tests {
    use super::*;

    #[test]
    fn test_partial_is_accepted_and_marked() {
        let c = Case::new();
        let result = c.write(
            "result.json",
            &make_result(
                &[("a", "1.0")],
                &[("partial", "true"), ("partialReason", r#""ceiling""#)],
            ),
        );
        let out = c.path("candidate.json");
        let (code, _, _) = c.run_cli(&["candidate", &result, &out]);
        assert_eq!(code, 0);
        let data = read_json(Path::new(&out));
        assert!(matches!(
            obj(&data).get("candidate"),
            Some(Value::Bool(true))
        ));
        assert_eq!(
            obj(&data).get("cases").expect("cases"),
            &json::loads(r#"{"a": 1.0}"#).expect("json")
        );
    }

    #[test]
    fn test_cases_without_score_are_left_out() {
        let c = Case::new();
        let mut data = make_result(&[("a", "1.0"), ("b", "1.0")], &[]);
        let cases = data
            .as_object_mut()
            .and_then(|m| m.get_mut("cases"))
            .and_then(Value::as_array_mut)
            .expect("cases");
        cases[1]
            .as_object_mut()
            .expect("case")
            .insert("aggregates".into(), Value::Object(Map::new()));
        let out = c.path("candidate.json");
        let result = c.write("result.json", &data);
        let (code, _, _) = c.run_cli(&["candidate", &result, &out]);
        assert_eq!(code, 0);
        assert_eq!(
            obj(&read_json(Path::new(&out)))
                .get("cases")
                .expect("cases"),
            &json::loads(r#"{"a": 1.0}"#).expect("json")
        );
    }
}

mod compare_tests {
    use super::*;

    #[test]
    fn test_pass() {
        let c = Case::new();
        let (code, out, _) = c.compare(
            &[("a", "1.0"), ("b", "0.8")],
            &[("a", "1.0"), ("b", "0.8")],
            &[],
            &[],
        );
        assert_eq!(code, 0);
        assert!(out.contains("0 regression"), "{out}");
    }

    #[test]
    fn test_improvement_passes() {
        let c = Case::new();
        assert_eq!(c.compare(&[("a", "1.0")], &[("a", "0.5")], &[], &[]).0, 0);
    }

    #[test]
    fn test_regression() {
        let c = Case::new();
        let (code, out, _) = c.compare(
            &[("a", "0.5"), ("b", "1.0")],
            &[("a", "1.0"), ("b", "1.0")],
            &[],
            &[],
        );
        assert_eq!(code, 1);
        assert!(out.contains("REGRESSION"), "{out}");
    }

    #[test]
    fn test_missing_case_is_a_regression() {
        let c = Case::new();
        let (code, out, _) = c.compare(&[("a", "1.0")], &[("a", "1.0"), ("b", "1.0")], &[], &[]);
        assert_eq!(code, 1);
        assert!(out.contains("MISSING"), "{out}");
    }

    #[test]
    fn test_allow_missing() {
        let c = Case::new();
        let (code, _, _) = c.compare(
            &[("a", "1.0")],
            &[("a", "1.0"), ("b", "1.0")],
            &["--allow-missing"],
            &[],
        );
        assert_eq!(code, 0);
    }

    #[test]
    fn test_allow_missing_still_fails_a_regression() {
        let c = Case::new();
        let (code, _, _) = c.compare(
            &[("a", "0.5")],
            &[("a", "1.0"), ("b", "1.0")],
            &["--allow-missing"],
            &[],
        );
        assert_eq!(code, 1);
    }

    #[test]
    fn test_new_case_is_reported_not_failed() {
        let c = Case::new();
        let (code, out, _) =
            c.compare(&[("a", "1.0"), ("extra", "0.0")], &[("a", "1.0")], &[], &[]);
        assert_eq!(code, 0);
        assert!(out.contains("new") && out.contains("extra"), "{out}");
    }

    #[test]
    fn test_tolerance() {
        let c = Case::new();
        assert_eq!(c.compare(&[("a", "0.9")], &[("a", "1.0")], &[], &[]).0, 1);
        assert_eq!(
            c.compare(
                &[("a", "0.9")],
                &[("a", "1.0")],
                &["--tolerance", "0.1"],
                &[]
            )
            .0,
            0
        );
        assert_eq!(
            c.compare(
                &[("a", "0.8")],
                &[("a", "1.0")],
                &["--tolerance", "0.1"],
                &[]
            )
            .0,
            1
        );
    }

    #[test]
    fn test_different_claude_version_is_reported_not_failed() {
        let c = Case::new();
        let result = c.write(
            "result.json",
            &make_result(&[("a", "1.0")], &[("claudeVersion", r#""2.2.0""#)]),
        );
        let baseline = c.baseline(&[("a", "1.0")], "2.1.311");
        let (code, out, _) = c.run_cli(&["compare", &result, &baseline]);
        assert_eq!(code, 0);
        assert!(out.contains("2.2.0"), "{out}");
    }

    #[test]
    fn test_missing_baseline_exits_3() {
        let c = Case::new();
        let result = c.write("result.json", &make_result(&[("a", "1.0")], &[]));
        let path = c.path("evals-baseline.json");
        let (code, _, err) = c.run_cli(&["compare", &result, &path]);
        assert_eq!(code, 3);
        assert!(
            err.contains(&format!("no eval baseline recorded at {path}")),
            "{err}"
        );
        assert!(err.contains("scripts/run-evals.sh --record"), "{err}");
    }

    #[test]
    fn test_partial_result_exits_2_with_reason() {
        let c = Case::new();
        let (code, out, err) = c.compare(
            &[("a", "1.0")],
            &[("a", "1.0")],
            &[],
            &[
                ("partial", "true"),
                ("partialReason", r#""cost ceiling hit""#),
            ],
        );
        assert_eq!(code, 2);
        assert!(format!("{out}{err}").contains("cost ceiling hit"));
    }

    #[test]
    fn test_partial_beats_missing_baseline() {
        let c = Case::new();
        let result = c.write(
            "result.json",
            &make_result(
                &[("a", "1.0")],
                &[("partial", "true"), ("partialReason", r#""x""#)],
            ),
        );
        let (code, _, _) = c.run_cli(&["compare", &result, &c.path("none.json")]);
        assert_eq!(code, 2);
    }

    #[test]
    fn test_bad_case_name_exits_2_and_is_not_printed() {
        let c = Case::new();
        let evil = "a|b\n::error::boom";
        let result = c.write(
            "result.json",
            &make_result(&[("a", "1.0"), (evil, "1.0")], &[]),
        );
        let baseline = c.baseline(&[("a", "1.0")], "2.1.311");
        let summary = c.path("summary.md");
        let (code, out, _) = c.run_cli(&["compare", &result, &baseline, "--summary", &summary]);
        assert_eq!(code, 2);
        assert!(!out.contains("::error::") && !out.contains("boom"), "{out}");
        assert!(!Path::new(&summary).exists());
    }

    #[test]
    fn test_bad_case_name_in_baseline_exits_2() {
        let c = Case::new();
        let result = c.write("result.json", &make_result(&[("a", "1.0")], &[]));
        let baseline = c.baseline(&[("A b", "1.0")], "2.1.311");
        assert_eq!(c.run_cli(&["compare", &result, &baseline]).0, 2);
    }

    #[test]
    fn test_partial_reason_cannot_start_a_workflow_command() {
        let c = Case::new();
        let (code, out, err) = c.compare(
            &[("a", "1.0")],
            &[("a", "1.0")],
            &[],
            &[
                ("partial", "true"),
                ("partialReason", r#""x\n::error::boom""#),
            ],
        );
        assert_eq!(code, 2);
        for line in format!("{out}{err}").lines() {
            assert!(!line.starts_with("::"), "{line}");
        }
    }

    #[test]
    fn test_summary_is_appended() {
        let c = Case::new();
        let summary = c.write_text("summary.md", "earlier content\n");
        let (code, _, _) = c.compare(
            &[("a", "0.5"), ("b", "1.0"), ("n", "1.0")],
            &[("a", "1.0"), ("b", "1.0")],
            &["--summary", &summary],
            &[],
        );
        assert_eq!(code, 1);
        let text = fs::read_to_string(&summary).expect("summary");
        assert!(text.starts_with("earlier content\n"));
        assert!(text.contains("| case | baseline | now | status |"));
        assert!(text.contains("| a | 1.000 | 0.500 | REGRESSION |"));
        assert!(text.contains("| b | 1.000 | 1.000 | ok |"));
        assert!(text.contains("| n | - | 1.000 | new |"));
    }
}

/// Not ledger targets: exact output, argparse and R10 edges.
mod lead_extras {
    use super::*;

    #[test]
    fn compare_output_is_byte_exact() {
        let c = Case::new();
        let result = c.write(
            "result.json",
            &make_result(
                &[("a", "0.5"), ("n", "1")],
                &[("claudeVersion", "[1, \"x\\ny\"]")],
            ),
        );
        let baseline = c.baseline(&[("b", "1"), ("a", "1.0")], "2.1.311");
        let (code, out, err) = c.run_cli(&["compare", &result, &baseline, "--tol=0.5"]);
        assert_eq!((code, err.as_str()), (1, ""));
        assert_eq!(
            out,
            "ok         a: baseline 1.000, now 0.500\n\
             MISSING    b: baseline 1.000, now -\n\
             new        n: baseline -, now 1.000\n\
             note: Claude Code [1, 'x\\ny'] differs from the baseline's 2.1.311; this alone is not a failure\n\
             1 regression(s) across 2 baseline case(s), 2 case(s) scored\n"
        );
    }

    #[test]
    fn argparse_errors_and_float_tolerance() {
        let c = Case::new();
        let (code, _, err) = c.run_cli(&[]);
        assert_eq!(code, 2);
        assert!(
            err.ends_with(
                "eval_baseline.py: error: the following arguments are required: command\n"
            ),
            "{err}"
        );
        let (code, _, err) = c.run_cli(&["nope"]);
        assert_eq!(code, 2);
        assert!(err.ends_with(
            "error: argument command: invalid choice: 'nope' (choose from record, candidate, compare)\n"
        ), "{err}");
        let (code, _, err) = c.run_cli(&["compare", "r", "b", "--tolerance", "x"]);
        assert_eq!(code, 2);
        assert!(
            err.ends_with(
                "eval_baseline.py compare: error: argument --tolerance: invalid float value: 'x'\n"
            ),
            "{err}"
        );
        let (code, _, err) = c.run_cli(&["record", "r"]);
        assert_eq!(code, 2);
        assert!(
            err.ends_with("error: the following arguments are required: baseline\n"),
            "{err}"
        );
        let (code, _, err) = c.run_cli(&["--x", "record", "r", "b", "c"]);
        assert_eq!(code, 2);
        assert!(
            err.ends_with("eval_baseline.py: error: unrecognized arguments: --x c\n"),
            "{err}"
        );
        // From CPython 3.12.16: `-h-` is refused, `-hX` is help, a second `--` is a value.
        let (code, _, err) = c.run_cli(&["compare", "-h-"]);
        assert_eq!(code, 2);
        assert!(
            err.ends_with(
                "eval_baseline.py compare: error: argument -h/--help: ignored explicit argument '-'\n"
            ),
            "{err}"
        );
        let (code, out, _) = c.run_cli(&["compare", "-hX"]);
        assert_eq!(code, 0);
        assert!(
            out.starts_with("usage: eval_baseline.py compare [-h]"),
            "{out}"
        );
        let (code, _, err) = c.run_cli(&["record", "--", "--"]);
        assert_eq!(code, 2);
        assert!(
            err.ends_with(
                "eval_baseline.py record: error: the following arguments are required: baseline\n"
            ),
            "{err}"
        );
        for argv in [
            &["record", "a", "--", "--"][..],
            &["--", "record", "a", "b"],
        ] {
            let (code, _, err) = c.run_cli(argv);
            assert_eq!(code, 2);
            assert!(
                err.starts_with("eval_baseline: cannot read the result a: "),
                "{err}"
            );
        }
        let (code, _, err) = c.run_cli(&["compare", "r", "b", "--allow-missing=1"]);
        assert_eq!(code, 2);
        assert!(
            err.ends_with("error: argument --allow-missing: ignored explicit argument '1'\n"),
            "{err}"
        );
    }

    #[test]
    fn a_huge_int_score_is_an_uncaught_overflow() {
        let c = Case::new();
        let big = "1".repeat(400);
        let result = c.write("result.json", &make_result(&[("a", &big)], &[]));
        let (code, out, err) = c.run_cli(&["record", &result, &c.path("b.json")]);
        assert_eq!((code, out.as_str()), (1, ""));
        assert_eq!(
            err,
            "anthology-dev eval-baseline: OverflowError: int too large to convert to float\n"
        );
    }

    #[test]
    fn a_missing_result_names_the_strerror() {
        let c = Case::new();
        let (code, _, err) = c.run_cli(&["record", "nope.json", "b.json"]);
        assert_eq!(code, 2);
        assert!(
            err.starts_with("eval_baseline: cannot read the result nope.json: "),
            "{err}"
        );
        assert!(!err.contains("[Errno"), "{err}");
    }
}
