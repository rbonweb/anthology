"""F4's oracle driver: one long-lived process per difftest run (design §13.2).

    python3 -I port/difftest/oracle/_driver.py

Speaks JSONL. Each stdin line is a request `{"module": "<file stem>", "case": {...}}`; each
answer is one stdout line, the module's `run(case)` dict, or `{"error": "<traceback>"}` when the
request is bad, the module cannot load, or `run` raises (a crash is an unexplained diff, never a
pass). Answers are `json.dumps(answer, ensure_ascii=True)`, so a lone surrogate in a result
travels as an escape; each line is flushed.

Modules are `port/difftest/oracle/<module>.py`, loaded once by path under the name
`difftest_oracle_<module>` (never by putting this folder on `sys.path`: `json.py` here shadows
the standard library's `json`, which is also why every oracle script runs as `python3 -I`). A
module name is lowercase letters, digits and `_`, and does not start with `_`.

While `run` runs, file descriptor 1 points at stderr, so a stray `print` (or a child process)
cannot corrupt the protocol. Bytecode is not written (the repository may be mounted read-only).

Python 3.9+ and the standard library only.
"""
from __future__ import annotations

import sys

sys.dont_write_bytecode = True

import importlib.util  # noqa: E402
import json  # noqa: E402
import os  # noqa: E402
import re  # noqa: E402
import traceback  # noqa: E402
from pathlib import Path  # noqa: E402

HERE = Path(__file__).resolve().parent
_NAME = re.compile(r"[a-z0-9][a-z0-9_]*")
_MODULES = {}


def _load(name):
    mod = _MODULES.get(name)
    if mod is None:
        if not isinstance(name, str) or not _NAME.fullmatch(name):
            raise ValueError("bad oracle module name %r" % (name,))
        path = HERE / (name + ".py")
        if not path.is_file():
            raise FileNotFoundError("no oracle module %s" % path)
        spec = importlib.util.spec_from_file_location("difftest_oracle_" + name, str(path))
        mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(mod)
        _MODULES[name] = mod
    return mod


def _answer(line):
    try:
        req = json.loads(line)
        if not isinstance(req, dict) or set(req) != {"module", "case"}:
            raise ValueError("a request is {\"module\": ..., \"case\": ...}")
        result = _load(req["module"]).run(req["case"])
        if not isinstance(result, dict):
            raise TypeError("run() returned %s, not a dict" % type(result).__name__)
        return json.dumps(result, ensure_ascii=True)
    except Exception:  # every failure is an answer, never a dead driver
        return json.dumps({"error": traceback.format_exc()}, ensure_ascii=True)


def main():
    out = os.fdopen(os.dup(1), "w", encoding="ascii", newline="\n")
    os.dup2(2, 1)
    sys.stdout = sys.stderr
    for raw in sys.stdin.buffer:
        line = raw.decode("utf-8", errors="replace").strip()
        if not line:
            continue
        out.write(_answer(line) + "\n")
        out.flush()
    return 0


if __name__ == "__main__":
    sys.exit(main())
