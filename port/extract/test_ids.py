"""Every Python test run id, as `python -m unittest discover -s tests -v` would run it.

Usage (repository root, the pinned Python 3.12 image with tests/requirements.txt installed):

    python port/extract/test_ids.py [--out FILE]

The ids come from the unittest loader itself, so inherited test methods (one base class, several
subclasses) give one id per subclass, exactly as a run does. Nothing is run. A module that fails
to import is an error, never a silently shorter list. The output is sorted, one id per line:
the committed form of `port/python-test-ids.txt` (design §13.3).
"""
from __future__ import annotations

import argparse
import os
import sys
import unittest


def iter_tests(suite):
    for item in suite:
        if isinstance(item, unittest.TestSuite):
            yield from iter_tests(item)
        else:
            yield item


def collect(start_dir: str) -> list[str]:
    suite = unittest.TestLoader().discover(start_dir=start_dir)
    ids = []
    failed = []
    for test in iter_tests(suite):
        if type(test).__module__ == "unittest.loader" or type(test).__name__ == "_FailedTest":
            failed.append(test.id())
            continue
        ids.append(test.id())
    if failed:
        raise SystemExit("test_ids: the loader could not load: " + ", ".join(sorted(failed)))
    duplicates = sorted({i for i in ids if ids.count(i) > 1})
    if duplicates:
        raise SystemExit("test_ids: duplicate run ids: " + ", ".join(duplicates))
    return sorted(ids)


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--tests", default="tests", help="the discover start directory")
    parser.add_argument("--out", help="write here instead of stdout")
    args = parser.parse_args(argv)
    if not os.path.isdir(args.tests):
        raise SystemExit(f"test_ids: no {args.tests}/ directory (run from the repository root)")
    text = "".join(i + "\n" for i in collect(args.tests))
    if args.out:
        with open(args.out, "w", encoding="utf-8", newline="\n") as fh:
            fh.write(text)
    else:
        sys.stdout.write(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
