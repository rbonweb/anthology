"""Seed cases for the `shell.segments` and `shell.analyze` difftest surfaces (design §13.2: seeds come from
every Python test's inputs).

Usage (repository root, the pinned Python 3.12 image of ci/images/python.Dockerfile):

    python -I port/extract/shell_seeds.py [--out FILE] [--check]

It runs the guard's Python tests in-process with `guard_rules._segments`, `_shell_parse`, `_script_starts`
and `_analyze` wrapped, and records every call: `(text, depth)`, `(prog, args, i)`, `(words)` and
`(command, home)` (string homes only; the guard passes "" for any other). Nested calls are recorded too.
Texts over MAX_CHARS code points are dropped (the lexer's own tests cover the big timing inputs), and so is
any case that holds a secret-like fixture (the tests assemble them at run time so none is committed).

The output is sorted, one canonical JSON case (`{"op": ..., ...}`) per line: the committed
`port/difftest/seeds/shell.jsonl`. `--check` exits 1 when the committed file is stale.
"""
from __future__ import annotations

import argparse
import io
import json
import os
import re
import sys
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
OUT = os.path.join("port", "difftest", "seeds", "shell.jsonl")
MAX_CHARS = 20000
TEST_MODULES = ("test_guard_rules",)
# Secret-like fixtures (built by concatenation in the tests) must not land in a committed file.
_SECRET_LIKE = re.compile(
    r"AKIA|gh[pousr]_|github_pat_|sk-[A-Za-z0-9]|xox[a-z]-|PRIVATE KEY|hunter2|pass" + "word",
    re.IGNORECASE)


def collect():
    sys.path.insert(0, os.path.join(ROOT, "tests"))
    sys.path.insert(0, os.path.join(ROOT, "plugins", "anthology-guardrails", "scripts"))
    import guard_rules  # the module the tests import, so the wrappers see every internal call

    cases = set()

    def keep(*texts):
        return all(len(t) <= MAX_CHARS and not _SECRET_LIKE.search(t) for t in texts)

    def add(obj):
        cases.add(json.dumps(obj, sort_keys=True, separators=(",", ":")))

    real_segments, real_parse = guard_rules._segments, guard_rules._shell_parse
    real_starts, real_analyze = guard_rules._script_starts, guard_rules._analyze

    def segments(text, depth=0):
        if keep(text):
            add({"op": "segments", "text": text, "depth": depth})
        return real_segments(text, depth)

    def shell_parse(prog, args, i=0):
        if keep(prog, *args):
            add({"op": "shell_parse", "prog": prog, "args": list(args), "i": i})
        return real_parse(prog, args, i)

    def script_starts(words):
        if keep(*words):
            add({"op": "script_starts", "words": list(words)})
        return real_starts(words)

    def analyze(command, home):
        if isinstance(home, str) and keep(command, home):
            add({"op": "analyze", "command": command, "home": home})
        return real_analyze(command, home)

    guard_rules._segments, guard_rules._shell_parse = segments, shell_parse
    guard_rules._script_starts, guard_rules._analyze = script_starts, analyze
    try:
        suite = unittest.TestSuite(
            unittest.defaultTestLoader.loadTestsFromName(name) for name in TEST_MODULES)
        result = unittest.TextTestRunner(stream=io.StringIO(), verbosity=0).run(suite)
    finally:
        guard_rules._segments, guard_rules._shell_parse = real_segments, real_parse
        guard_rules._script_starts, guard_rules._analyze = real_starts, real_analyze
    # Recording slows the parser down, so only the wall-clock bounds may fail here; anything else means
    # the wrappers changed behaviour, and the seeds would not be the tests' inputs.
    broken = [test.id() for test, _trace in result.failures + result.errors
              if "cheap" not in test.id() and "linearly" not in test.id()]
    if broken:
        raise SystemExit("shell_seeds: guard tests failed under recording: " + ", ".join(broken))
    return sorted(cases)


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", default=OUT)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args(argv)
    text = "".join(line + "\n" for line in collect())
    path = os.path.join(ROOT, args.out)
    if args.check:
        with open(path, encoding="utf-8") as f:
            if f.read() != text:
                print("shell_seeds: %s is stale; regenerate it in the 3.12 image" % args.out,
                      file=sys.stderr)
                return 1
        return 0
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        f.write(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
