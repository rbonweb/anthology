"""F4's oracle driver: one long-lived process per difftest run (design §13.2).

    python3 -I port/difftest/oracle/_driver.py              # the JSONL loop
    python3 -I port/difftest/oracle/_driver.py --self-test  # checks this file's own rules

Speaks JSONL. Each request line is `{"module": "<file stem>", "case": {...}}`; each answer is
one line, the module's `run(case)` dict, or `{"error": "<traceback>"}` when the request is bad,
the module cannot load, `run` raises (any exception, `SystemExit` included), or the result
breaks a rule below. An error is an unexplained diff, never a pass. Answers are
`json.dumps(answer, ensure_ascii=True)`, one flushed line each.

Rules for a result:
- It is a dict.
- No lone surrogate anywhere except under the `"python"` key (Python's own result, kept for the
  report). Rust's JSON decoder reads an escaped lone surrogate as U+FFFD (F-19), so a surrogate
  would compare equal to Rust's U+FFFD and an undeclared D-6 would pass. A module replaces
  lone surrogates itself and marks `"divergence": "D-6"`, or sends bytes as `stdout_hex`.

Modules are `port/difftest/oracle/<module>.py`, loaded once by path under the name
`difftest_oracle_<module>`, never by putting this folder on `sys.path` (`json.py` here shadows
the standard library's `json`, which is also why every oracle script runs as `python3 -I`). A
module name is lowercase letters, digits and `_`, and does not start with `_`.

Isolation while a module runs: file descriptor 1 points at stderr and fd 0 at /dev/null, so a
stray `print`, a read of `sys.stdin` or a child process cannot touch the protocol, which uses
private copies of the original descriptors. Bytecode is not written (the repository may be
mounted read-only).

Python 3.9+ and the standard library only.
"""
from __future__ import annotations

import sys

sys.dont_write_bytecode = True

import importlib.util  # noqa: E402
import io  # noqa: E402
import json  # noqa: E402
import os  # noqa: E402
import re  # noqa: E402
import tempfile  # noqa: E402
import traceback  # noqa: E402
from pathlib import Path  # noqa: E402

HERE = Path(__file__).resolve().parent
_NAME = re.compile(r"[a-z0-9][a-z0-9_]*")
_MODULES = {}


def _load(name, folder):
    key = (str(folder), name)
    mod = _MODULES.get(key)
    if mod is None:
        if not isinstance(name, str) or not _NAME.fullmatch(name):
            raise ValueError("bad oracle module name %r" % (name,))
        path = Path(folder) / (name + ".py")
        if not path.is_file():
            raise FileNotFoundError("no oracle module %s" % path)
        spec = importlib.util.spec_from_file_location("difftest_oracle_" + name, str(path))
        mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(mod)
        _MODULES[key] = mod
    return mod


def _check(result):
    if not isinstance(result, dict):
        raise TypeError("run() returned %s, not a dict" % type(result).__name__)
    rest = {k: v for k, v in result.items() if k != "python"}
    try:
        json.dumps(rest, ensure_ascii=False).encode("utf-8")
    except UnicodeEncodeError:
        raise ValueError(
            "the result holds a lone surrogate outside \"python\": replace it with U+FFFD and "
            "mark \"divergence\": \"D-6\", or send the bytes as stdout_hex/stderr_hex") from None


def _answer(line, folder=HERE):
    try:
        req = json.loads(line)
        if not isinstance(req, dict) or set(req) != {"module", "case"}:
            raise ValueError("a request is {\"module\": ..., \"case\": ...}")
        result = _load(req["module"], folder).run(req["case"])
        _check(result)
        return json.dumps(result, ensure_ascii=True)
    except KeyboardInterrupt:
        raise
    except BaseException:  # every failure is an answer, never a dead driver
        return json.dumps({"error": traceback.format_exc()}, ensure_ascii=True)


def main(folder=HERE):
    proto_in = os.fdopen(os.dup(0), "rb")
    proto_out = os.fdopen(os.dup(1), "w", encoding="ascii", newline="\n")
    null = os.open(os.devnull, os.O_RDONLY)
    os.dup2(null, 0)
    os.close(null)
    os.dup2(2, 1)
    sys.stdin = io.StringIO("")
    sys.stdout = sys.stderr
    for raw in proto_in:
        line = raw.decode("utf-8", errors="replace").strip()
        if not line:
            continue
        proto_out.write(_answer(line, folder) + "\n")
        proto_out.flush()
    return 0


_PROBES = {
    "lone": 'def run(case):\n    return {"exit": 0, "stdout": "\\ud800", "stderr": ""}\n',
    "lonekey": 'def run(case):\n    return {"exit": 0, "stdout": "", "stderr": "", "files": '
               '{"\\udcff": {"type": "dir", "mode": "0755"}}}\n',
    "marked": 'def run(case):\n    return {"exit": 0, "stdout": "\\ufffd", "stderr": "", '
              '"divergence": "D-6", "python": {"exit": 0, "stdout": "\\ud800"}}\n',
    "exits": 'import sys\ndef run(case):\n    sys.exit(3)\n',
    "reads": 'import sys\ndef run(case):\n    return {"exit": 0, "stdout": sys.stdin.read(), '
             '"stderr": ""}\n',
    "notdict": 'def run(case):\n    return [1]\n',
}


def self_test():
    """The driver's rules, on probe modules in a temporary folder."""
    with tempfile.TemporaryDirectory() as folder:
        for name, text in _PROBES.items():
            Path(folder, name + ".py").write_text(text, encoding="utf-8")

        def ask(module):
            return json.loads(_answer(json.dumps({"module": module, "case": {}}), folder))

        for module in ("lone", "lonekey"):
            got = ask(module)
            assert "lone surrogate outside" in got.get("error", ""), (module, got)
        got = ask("marked")
        assert got == {"exit": 0, "stdout": "\ufffd", "stderr": "", "divergence": "D-6",
                       "python": {"exit": 0, "stdout": "\ud800"}}, got
        assert "SystemExit" in ask("exits").get("error", ""), ask("exits")
        assert "not a dict" in ask("notdict").get("error", "")
        for bad in ("_driver", "../json", "x-y"):
            assert "bad oracle module name" in ask(bad).get("error", ""), bad
        assert "no oracle module" in ask("missing").get("error", "")
        assert "a request is" in json.loads(_answer('{"module": "lone"}', folder))["error"]
        assert "error" in json.loads(_answer("not json", folder))
    # fd 0 isolation needs the real loop: run it on a pipe.
    import subprocess
    with tempfile.TemporaryDirectory() as folder:
        Path(folder, "reads.py").write_text(_PROBES["reads"], encoding="utf-8")
        code = ("import runpy, sys; g = runpy.run_path(%r); sys.exit(g['main'](%r))"
                % (str(Path(__file__)), folder))
        req = json.dumps({"module": "reads", "case": {}}) + "\n"
        out = subprocess.run([sys.executable, "-I", "-c", code], input=req.encode() * 2,
                             stdout=subprocess.PIPE, timeout=60).stdout.decode()
        lines = out.splitlines()
        assert len(lines) == 2 and all(json.loads(x)["stdout"] == "" for x in lines), out
    print("_driver.py self-test: ok")
    return 0


if __name__ == "__main__":
    if sys.argv[1:] == ["--self-test"]:
        sys.exit(self_test())
    sys.exit(main())
