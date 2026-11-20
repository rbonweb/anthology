"""Tests for scripts/eval_baseline.py: record, compare and candidate."""
from __future__ import annotations

import contextlib
import importlib.util
import io
import json
import tempfile
import unittest
from pathlib import Path

from _repo import ROOT

_spec = importlib.util.spec_from_file_location("eval_baseline", ROOT / "scripts" / "eval_baseline.py")
eval_baseline = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(eval_baseline)


def make_result(scores, **extra):
    result = {
        "claudeVersion": "2.1.311",
        "costUsd": 4.5,
        "startedAt": "2026-11-20T05:41:12.000Z",
        "partial": False,
        "partialReason": None,
        "aggregates": {"casesTotal": len(scores)},
        "cases": [{"name": n, "aggregates": {"score": s, "passRate": 1.0}} for n, s in scores.items()],
    }
    result.update(extra)
    return result


class BaselineTestCase(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.dir = Path(tmp.name)

    def write(self, name, data):
        path = self.dir / name
        path.write_text(data if isinstance(data, str) else json.dumps(data), encoding="utf-8")
        return str(path)

    def run_cli(self, *argv):
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            try:
                code = eval_baseline.main(list(argv))
            except SystemExit as exc:  # argparse errors
                code = exc.code
        return code, out.getvalue(), err.getvalue()

    def baseline(self, scores, version="2.1.311"):
        return self.write("baseline.json", {"schema_version": 1, "claude_code_version": version,
                                            "recorded_at": "2026-11-13", "cost_usd": 1.0, "cases": scores})


class RecordTests(BaselineTestCase):
    def test_round_trip(self):
        result = self.write("result.json", make_result({"route-scout": 1.0, "guard-g1": 0.5}))
        out = self.dir / "baseline.json"
        code, _, _ = self.run_cli("record", result, str(out))
        self.assertEqual(code, 0)
        text = out.read_text(encoding="utf-8")
        self.assertEqual(json.loads(text), {
            "schema_version": 1, "claude_code_version": "2.1.311", "recorded_at": "2026-11-20",
            "cost_usd": 4.5, "cases": {"guard-g1": 0.5, "route-scout": 1.0}})
        self.assertEqual(text, json.dumps(json.loads(text), indent=2, sort_keys=True) + "\n")
        # and the recorded file passes a compare against the same result
        code, _, _ = self.run_cli("compare", result, str(out))
        self.assertEqual(code, 0)

    def test_partial_is_refused(self):
        result = self.write("result.json", make_result({"a": 1.0}, partial=True, partialReason="cost ceiling hit"))
        out = self.dir / "baseline.json"
        code, _, err = self.run_cli("record", result, str(out))
        self.assertEqual(code, 2)
        self.assertIn("cost ceiling hit", err)
        self.assertFalse(out.exists())

    def test_zero_cases_refused(self):
        result = self.write("result.json", make_result({}))
        code, _, _ = self.run_cli("record", result, str(self.dir / "b.json"))
        self.assertEqual(code, 2)

    def test_case_without_score_refused(self):
        data = make_result({"a": 1.0, "b": 1.0})
        data["cases"][1]["aggregates"] = {"passRate": 1.0}
        code, _, err = self.run_cli("record", self.write("result.json", data), str(self.dir / "b.json"))
        self.assertEqual(code, 2)
        self.assertIn("b", err)

    def test_non_numeric_score_refused(self):
        data = make_result({"a": 1.0})
        data["cases"][0]["aggregates"]["score"] = "1.0"
        code, _, _ = self.run_cli("record", self.write("result.json", data), str(self.dir / "b.json"))
        self.assertEqual(code, 2)

    def test_unreadable_result_refused(self):
        code, _, _ = self.run_cli("record", str(self.dir / "nope.json"), str(self.dir / "b.json"))
        self.assertEqual(code, 2)
        code, _, _ = self.run_cli("record", self.write("bad.json", "{not json"), str(self.dir / "b.json"))
        self.assertEqual(code, 2)


class CandidateTests(BaselineTestCase):
    def test_partial_is_accepted_and_marked(self):
        result = self.write("result.json", make_result({"a": 1.0}, partial=True, partialReason="ceiling"))
        out = self.dir / "candidate.json"
        code, _, _ = self.run_cli("candidate", result, str(out))
        self.assertEqual(code, 0)
        data = json.loads(out.read_text(encoding="utf-8"))
        self.assertIs(data["candidate"], True)
        self.assertEqual(data["cases"], {"a": 1.0})

    def test_cases_without_score_are_left_out(self):
        data = make_result({"a": 1.0, "b": 1.0})
        data["cases"][1]["aggregates"] = {}
        out = self.dir / "candidate.json"
        code, _, _ = self.run_cli("candidate", self.write("result.json", data), str(out))
        self.assertEqual(code, 0)
        self.assertEqual(json.loads(out.read_text(encoding="utf-8"))["cases"], {"a": 1.0})


class CompareTests(BaselineTestCase):
    def compare(self, now, base, *extra, **kw):
        result = self.write("result.json", make_result(now, **kw))
        return self.run_cli("compare", result, self.baseline(base), *extra)

    def test_pass(self):
        code, out, _ = self.compare({"a": 1.0, "b": 0.8}, {"a": 1.0, "b": 0.8})
        self.assertEqual(code, 0)
        self.assertIn("0 regression", out)

    def test_improvement_passes(self):
        code, _, _ = self.compare({"a": 1.0}, {"a": 0.5})
        self.assertEqual(code, 0)

    def test_regression(self):
        code, out, _ = self.compare({"a": 0.5, "b": 1.0}, {"a": 1.0, "b": 1.0})
        self.assertEqual(code, 1)
        self.assertIn("REGRESSION", out)

    def test_missing_case_is_a_regression(self):
        code, out, _ = self.compare({"a": 1.0}, {"a": 1.0, "b": 1.0})
        self.assertEqual(code, 1)
        self.assertIn("MISSING", out)

    def test_allow_missing(self):
        code, _, _ = self.compare({"a": 1.0}, {"a": 1.0, "b": 1.0}, "--allow-missing")
        self.assertEqual(code, 0)

    def test_allow_missing_still_fails_a_regression(self):
        code, _, _ = self.compare({"a": 0.5}, {"a": 1.0, "b": 1.0}, "--allow-missing")
        self.assertEqual(code, 1)

    def test_new_case_is_reported_not_failed(self):
        code, out, _ = self.compare({"a": 1.0, "extra": 0.0}, {"a": 1.0})
        self.assertEqual(code, 0)
        self.assertIn("new", out)
        self.assertIn("extra", out)

    def test_tolerance(self):
        self.assertEqual(self.compare({"a": 0.9}, {"a": 1.0})[0], 1)
        self.assertEqual(self.compare({"a": 0.9}, {"a": 1.0}, "--tolerance", "0.1")[0], 0)
        self.assertEqual(self.compare({"a": 0.8}, {"a": 1.0}, "--tolerance", "0.1")[0], 1)

    def test_different_claude_version_is_reported_not_failed(self):
        result = self.write("result.json", make_result({"a": 1.0}, claudeVersion="2.2.0"))
        code, out, _ = self.run_cli("compare", result, self.baseline({"a": 1.0}, version="2.1.311"))
        self.assertEqual(code, 0)
        self.assertIn("2.2.0", out)

    def test_missing_baseline_exits_3(self):
        result = self.write("result.json", make_result({"a": 1.0}))
        path = str(self.dir / "evals-baseline.json")
        code, _, err = self.run_cli("compare", result, path)
        self.assertEqual(code, 3)
        self.assertIn("no eval baseline recorded at " + path, err)
        self.assertIn("scripts/run-evals.sh --record", err)

    def test_partial_result_exits_2_with_reason(self):
        result = self.write("result.json", make_result({"a": 1.0}, partial=True, partialReason="cost ceiling hit"))
        code, out, err = self.run_cli("compare", result, self.baseline({"a": 1.0}))
        self.assertEqual(code, 2)
        self.assertIn("cost ceiling hit", out + err)

    def test_partial_beats_missing_baseline(self):
        result = self.write("result.json", make_result({"a": 1.0}, partial=True, partialReason="x"))
        code, _, _ = self.run_cli("compare", result, str(self.dir / "none.json"))
        self.assertEqual(code, 2)

    def test_bad_case_name_exits_2_and_is_not_printed(self):
        evil = "a|b\n::error::boom"
        result = self.write("result.json", make_result({"a": 1.0, evil: 1.0}))
        summary = self.dir / "summary.md"
        code, out, err = self.run_cli("compare", result, self.baseline({"a": 1.0}), "--summary", str(summary))
        self.assertEqual(code, 2)
        self.assertNotIn("::error::", out)
        self.assertNotIn("boom", out)
        self.assertFalse(summary.exists())

    def test_bad_case_name_in_baseline_exits_2(self):
        result = self.write("result.json", make_result({"a": 1.0}))
        code, _, _ = self.run_cli("compare", result, self.baseline({"A b": 1.0}))
        self.assertEqual(code, 2)

    def test_partial_reason_cannot_start_a_workflow_command(self):
        result = self.write("result.json", make_result({"a": 1.0}, partial=True,
                                                       partialReason="x\n::error::boom"))
        code, out, err = self.run_cli("compare", result, self.baseline({"a": 1.0}))
        self.assertEqual(code, 2)
        for line in (out + err).splitlines():
            self.assertFalse(line.startswith("::"), line)

    def test_summary_is_appended(self):
        summary = self.dir / "summary.md"
        summary.write_text("earlier content\n", encoding="utf-8")
        code, _, _ = self.compare({"a": 0.5, "b": 1.0, "n": 1.0}, {"a": 1.0, "b": 1.0},
                                  "--summary", str(summary))
        self.assertEqual(code, 1)
        text = summary.read_text(encoding="utf-8")
        self.assertTrue(text.startswith("earlier content\n"))
        self.assertIn("| case | baseline | now | status |", text)
        self.assertIn("| a | 1.000 | 0.500 | REGRESSION |", text)
        self.assertIn("| b | 1.000 | 1.000 | ok |", text)
        self.assertIn("| n | - | 1.000 | new |", text)


if __name__ == "__main__":
    unittest.main()
