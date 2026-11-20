"""Table rows of every Python test: each `self.subTest(...)` a test enters is one row.

Usage (repository root, the pinned Python 3.12 image with tests/requirements.txt installed):

    python port/extract/rows.py [--counts FILE] [--jsonl DIR] [--tests DIR]

The suite runs once, exactly as `python -m unittest discover -s tests` runs it, with
`TestCase.subTest` wrapped so every entry records its keyword parameters under the run id of the
test that entered it. The run must be green: a failing or erroring test may have stopped before
its last row, so its count would be wrong.

- `--counts FILE` writes `python_id<TAB>rows`, sorted, for every id with at least one row. This is
  the committed `port/python-test-rows.tsv`; `anthology-dev ledger check` compares the Rust rows
  (`crates/*/tests/data/<python_module>.jsonl`, tagged `"py"`) against it (design §13.3).
- `--jsonl DIR` writes `DIR/<python_module>.jsonl`, one row per line:
  `{"py": "<id>", "row": <n>, "params": {...}}`. Rows are the seeds a lane copies into its
  `tests/data/` file. Values that are not JSON are tagged (`$bytes` as hex, `$set`, `$float` for
  non-finite floats, `$repr` for anything else). The first character of every credential-shaped
  or home-path-shaped run is written as a `\\uXXXX` escape, so the file decodes to the test's own
  value while its raw text never holds a secret or a user path (the fixtures assemble them at
  run time for the same reason). These rows depend on the run (temporary paths, for example), so
  they are not committed; only the counts are.
"""
from __future__ import annotations

import argparse
import collections
import io
import json
import math
import os
import re
import sys
import unittest
from pathlib import PurePath

# Credential shapes (the union of guard G6 and the privacy scan, kept here so the extractor never
# imports the code under test before the run) and home paths. Only used to escape a character.
_SENSITIVE = [re.compile(p) for p in (
    r"(?:AKIA|ASIA)[0-9A-Z]{16}",
    r"gh[pousr]_[A-Za-z0-9]+",
    r"github_pat_",
    r"sk-[A-Za-z0-9_-]{8,}",
    r"xox[abposr]-",
    r"-----BEGIN [A-Z ]*PRIVATE KEY-----",
    r"(?i)pass(?:word|wd)?",
    r"(?i)secret",
    r"(?i)token",
    r"(?i)api[_-]?key",
    r"eyJ[A-Za-z0-9_-]{5,}",
    "/" + "Us" + "ers/",
    "/" + "home/",
)]


def iter_tests(suite):
    for item in suite:
        if isinstance(item, unittest.TestSuite):
            yield from iter_tests(item)
        else:
            yield item


class Recorder:
    def __init__(self):
        self.rows = collections.defaultdict(list)

    def install(self):
        original = unittest.TestCase.subTest
        rows = self.rows

        def subTest(test, msg=unittest.case._subtest_msg_sentinel, **params):  # noqa: N802
            entry = dict(params)
            if msg is not unittest.case._subtest_msg_sentinel:
                entry["$msg"] = msg
            rows[test.id()].append(entry)
            return original(test, msg, **params)

        unittest.TestCase.subTest = subTest


def run(start_dir: str) -> dict:
    recorder = Recorder()
    recorder.install()
    suite = unittest.TestLoader().discover(start_dir=start_dir)
    loaded = [t for t in iter_tests(suite)]
    failed_loads = [t.id() for t in loaded if type(t).__name__ == "_FailedTest"]
    if failed_loads:
        raise SystemExit("rows: the loader could not load: " + ", ".join(sorted(failed_loads)))
    stream = io.StringIO()
    result = unittest.TextTestRunner(stream=stream, verbosity=0).run(suite)
    if not result.wasSuccessful():
        sys.stderr.write(stream.getvalue())
        raise SystemExit("rows: the suite is not green, so row counts cannot be trusted")
    return recorder.rows


def plain(value):
    """`value` as JSON-ready data, with non-JSON values tagged."""
    if value is None or isinstance(value, (bool, int, str)):
        return value
    if isinstance(value, float):
        return value if math.isfinite(value) else {"$float": repr(value)}
    if isinstance(value, (list, tuple)):
        return [plain(v) for v in value]
    if isinstance(value, dict):
        return {(k if isinstance(k, str) else repr(k)): plain(v) for k, v in value.items()}
    if isinstance(value, (set, frozenset)):
        return {"$set": sorted((plain(v) for v in value), key=repr)}
    if isinstance(value, (bytes, bytearray)):
        return {"$bytes": bytes(value).hex()}
    if isinstance(value, PurePath):
        return str(value)
    return {"$repr": repr(value)}


def json_str(s: str, escape: bool = True) -> str:
    marks = set()
    for pattern in _SENSITIVE if escape else ():
        for m in pattern.finditer(s):
            if m.end() > m.start():
                marks.add(m.start())
    out = ['"']
    for i, ch in enumerate(s):
        if i in marks or ch in '"\\' or ord(ch) < 0x20 or ord(ch) > 0x7E:
            if ch in '"\\' and i not in marks:
                out.append("\\" + ch)
            elif ord(ch) > 0xFFFF:
                v = ord(ch) - 0x10000
                out.append("\\u%04x\\u%04x" % (0xD800 + (v >> 10), 0xDC00 + (v & 0x3FF)))
            else:
                out.append("\\u%04x" % ord(ch))
        else:
            out.append(ch)
    out.append('"')
    return "".join(out)


def dump(value) -> str:
    if isinstance(value, str):
        return json_str(value)
    if isinstance(value, list):
        return "[" + ", ".join(dump(v) for v in value) + "]"
    if isinstance(value, dict):
        return "{" + ", ".join(json_str(k) + ": " + dump(v) for k, v in value.items()) + "}"
    return json.dumps(value)


def write_counts(rows: dict, path: str) -> None:
    with open(path, "w", encoding="utf-8", newline="\n") as fh:
        for test_id in sorted(rows):
            fh.write(f"{test_id}\t{len(rows[test_id])}\n")


def write_jsonl(rows: dict, directory: str) -> None:
    os.makedirs(directory, exist_ok=True)
    by_module = collections.defaultdict(list)
    for test_id in sorted(rows):
        by_module[test_id.split(".", 1)[0]].append(test_id)
    for module, ids in sorted(by_module.items()):
        with open(os.path.join(directory, module + ".jsonl"), "w", encoding="utf-8",
                  newline="\n") as fh:
            for test_id in ids:
                for n, params in enumerate(rows[test_id]):
                    fh.write("{\"py\": " + json_str(test_id, escape=False) + f", \"row\": {n}, "
                             + "\"params\": " + dump(plain(params)) + "}\n")


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--tests", default="tests", help="the discover start directory")
    parser.add_argument("--counts", help="write the per-id row counts (TSV) here")
    parser.add_argument("--jsonl", help="write <python_module>.jsonl rows into this directory")
    args = parser.parse_args(argv)
    if not (args.counts or args.jsonl):
        parser.error("give --counts FILE, --jsonl DIR or both")
    if not os.path.isdir(args.tests):
        raise SystemExit(f"rows: no {args.tests}/ directory (run from the repository root)")
    # The run may chdir; resolve the outputs first.
    counts = os.path.abspath(args.counts) if args.counts else None
    jsonl = os.path.abspath(args.jsonl) if args.jsonl else None
    rows = run(args.tests)
    if counts:
        write_counts(rows, counts)
    if jsonl:
        write_jsonl(rows, jsonl)
    total = sum(len(v) for v in rows.values())
    sys.stderr.write(f"rows: {total} rows in {len(rows)} tests\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
