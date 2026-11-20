#!/usr/bin/env python3
"""Baseline gate for the behaviour evals: record scores, then fail on a regression.

Usage:
  eval_baseline.py record    <result.json> <baseline.json>
  eval_baseline.py compare   <result.json> <baseline.json> [--tolerance 0.0] [--allow-missing]
                             [--summary PATH]
  eval_baseline.py candidate <result.json> <out.json>

<result.json> is the `--json` output of one `claude plugin eval` run. `record` writes the
per-case scores of a full run as the baseline (evals/baseline.json). `compare` checks a later
run against it. `candidate` writes the same file as `record` plus `"candidate": true` and never
refuses a partial run; CI uploads it when no baseline exists yet.

Exit codes of `compare`: 0 no regression, 1 regression, 2 invalid or partial result (or an
invalid baseline), 3 baseline file missing. `record` exits 2 when it refuses the result.

A regression is a baseline case whose score fell below the baseline score minus the tolerance,
or (without --allow-missing) a baseline case that is absent from the result. A case that is new
in the result, or a different Claude Code version, is reported and never a failure.

Case names are checked against ^[a-z0-9]+(-[a-z0-9]+)*$ before anything is printed or written
to the job summary, so result content cannot inject Markdown or workflow commands.

Standard library only; runs on Python 3.9+.
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from typing import Dict, List, Optional

CASE_NAME_RE = re.compile(r"[a-z0-9]+(?:-[a-z0-9]+)*")
EPSILON = 1e-9
NO_BASELINE = ("no eval baseline recorded at {path}: run scripts/run-evals.sh --record on a full "
               "run and commit evals/baseline.json")


class InvalidResult(Exception):
    """The result (or baseline) file is unusable; the message is safe to print."""


def _clean(value, limit: int = 200) -> str:
    """A printable one-line rendering of untrusted text."""
    text = "".join(ch if ch.isprintable() else " " for ch in str(value))
    text = " ".join(text.split())
    return text if len(text) <= limit else text[:limit] + "..."


def _is_number(value) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool)


def _read_json(path: str, what: str):
    try:
        with open(path, encoding="utf-8") as fh:
            return json.load(fh)
    except OSError as exc:
        raise InvalidResult(f"cannot read the {what} {_clean(path)}: {_clean(exc.strerror or exc)}")
    except ValueError:
        raise InvalidResult(f"the {what} {_clean(path)} is not valid JSON")


def read_result(path: str) -> dict:
    data = _read_json(path, "result")
    if not isinstance(data, dict):
        raise InvalidResult("the result is not a JSON object")
    return data


def result_scores(result: dict, strict: bool = True) -> Dict[str, float]:
    """Case name -> score. With strict, a case without a numeric score is an error;
    otherwise such a case is left out."""
    cases = result.get("cases")
    if not isinstance(cases, list):
        raise InvalidResult("the result has no cases list")
    scores: Dict[str, float] = {}
    for case in cases:
        name = case.get("name") if isinstance(case, dict) else None
        if not isinstance(name, str) or not CASE_NAME_RE.fullmatch(name):
            raise InvalidResult(f"a case name in the result is not a valid case name: {_clean(name, 60)!r}")
        if name in scores:
            raise InvalidResult(f"case {name} appears twice in the result")
        aggregates = case.get("aggregates")
        score = aggregates.get("score") if isinstance(aggregates, dict) else None
        if not _is_number(score):
            if strict:
                raise InvalidResult(f"case {name} has no numeric aggregates.score")
            continue
        scores[name] = float(score)
    return scores


def build_baseline(result: dict, scores: Dict[str, float]) -> dict:
    started = result.get("startedAt")
    recorded_at = _clean(started.split("T")[0], 10) if isinstance(started, str) else None
    return {
        "schema_version": 1,
        "claude_code_version": result.get("claudeVersion"),
        "recorded_at": recorded_at,
        "cost_usd": result.get("costUsd"),
        "cases": scores,
    }


def _write_json(path: str, data: dict) -> None:
    with open(path, "w", encoding="utf-8") as fh:
        fh.write(json.dumps(data, indent=2, sort_keys=True) + "\n")


def _partial_message(result: dict) -> str:
    return "the result is partial: " + _clean(result.get("partialReason") or "no reason given", 300)


def check_complete(result: dict) -> Dict[str, float]:
    """Scores of a full run; raise InvalidResult for a partial run or an unusable one."""
    if result.get("partial") is True:
        raise InvalidResult(_partial_message(result))
    scores = result_scores(result, strict=True)
    if not scores:
        raise InvalidResult("the result has no cases")
    return scores


# ---- commands --------------------------------------------------------------------------

def cmd_record(args) -> int:
    result = read_result(args.result)
    scores = check_complete(result)
    _write_json(args.out, build_baseline(result, scores))
    print(f"recorded {len(scores)} cases in {args.out}")
    return 0


def cmd_candidate(args) -> int:
    result = read_result(args.result)
    scores = result_scores(result, strict=False)
    baseline = build_baseline(result, scores)
    baseline["candidate"] = True
    _write_json(args.out, baseline)
    print(f"wrote a candidate baseline with {len(scores)} cases to {args.out}")
    return 0


def _read_baseline(path: str) -> dict:
    data = _read_json(path, "baseline")
    cases = data.get("cases") if isinstance(data, dict) else None
    if not isinstance(cases, dict):
        raise InvalidResult(f"the baseline {_clean(path)} has no cases object")
    for name, score in cases.items():
        if not CASE_NAME_RE.fullmatch(name) or not _is_number(score):
            raise InvalidResult(f"the baseline {_clean(path)} has an invalid entry: {_clean(name, 60)!r}")
    return data


def _fmt(score: Optional[float]) -> str:
    return "-" if score is None else f"{score:.3f}"


def cmd_compare(args) -> int:
    result = read_result(args.result)
    now = check_complete(result)
    try:
        with open(args.baseline, encoding="utf-8"):
            pass
    except FileNotFoundError:
        print(NO_BASELINE.format(path=args.baseline), file=sys.stderr)
        return 3
    except OSError:
        pass  # let _read_baseline report it as invalid
    baseline = _read_baseline(args.baseline)
    base = {name: float(score) for name, score in baseline["cases"].items()}

    rows: List[tuple] = []
    regressions = 0
    for name in sorted(base):
        if name not in now:
            if args.allow_missing:
                rows.append((name, base[name], None, "not run"))
            else:
                rows.append((name, base[name], None, "MISSING"))
                regressions += 1
        elif now[name] < base[name] - args.tolerance - EPSILON:
            rows.append((name, base[name], now[name], "REGRESSION"))
            regressions += 1
        else:
            rows.append((name, base[name], now[name], "ok"))
    for name in sorted(set(now) - set(base)):
        rows.append((name, None, now[name], "new"))

    for name, was, is_now, status in rows:
        print(f"{status:10} {name}: baseline {_fmt(was)}, now {_fmt(is_now)}")
    old_version, new_version = baseline.get("claude_code_version"), result.get("claudeVersion")
    if old_version != new_version:
        print(f"note: Claude Code {_clean(new_version, 40)} differs from the baseline's "
              f"{_clean(old_version, 40)}; this alone is not a failure")
    print(f"{regressions} regression(s) across {len(base)} baseline case(s), {len(now)} case(s) scored")

    if args.summary:
        lines = ["### Behaviour eval baseline", "", "| case | baseline | now | status |", "| --- | --- | --- | --- |"]
        lines += [f"| {name} | {_fmt(was)} | {_fmt(is_now)} | {status} |" for name, was, is_now, status in rows]
        lines.append("")
        with open(args.summary, "a", encoding="utf-8") as fh:
            fh.write("\n".join(lines) + "\n")
    return 1 if regressions else 0


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    sub = parser.add_subparsers(dest="command", required=True)

    rec = sub.add_parser("record", help="write the baseline from a full run")
    rec.add_argument("result")
    rec.add_argument("out", metavar="baseline")
    rec.set_defaults(func=cmd_record)

    cand = sub.add_parser("candidate", help="write a candidate baseline, partial runs allowed")
    cand.add_argument("result")
    cand.add_argument("out")
    cand.set_defaults(func=cmd_candidate)

    cmp_ = sub.add_parser("compare", help="compare a run with the baseline")
    cmp_.add_argument("result")
    cmp_.add_argument("baseline")
    cmp_.add_argument("--tolerance", type=float, default=0.0)
    cmp_.add_argument("--allow-missing", action="store_true",
                      help="a baseline case absent from the result is not a regression")
    cmp_.add_argument("--summary", metavar="PATH", help="append a Markdown table to PATH")
    cmp_.set_defaults(func=cmd_compare)

    args = parser.parse_args(argv)
    try:
        return args.func(args)
    except InvalidResult as exc:
        print(f"eval_baseline: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
