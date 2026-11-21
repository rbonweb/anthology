"""GD oracle for the `guard.check` and `guard.hook` difftest surfaces (plan GD1, GD2; design §9.5, §13.2).

Run as `python3 -I` (no script folder on `sys.path`): `json.py` beside this file shadows the standard
library's `json`. F4's driver loads this module by file path.

Library surface: `run(case: dict) -> dict` (the case's `data`, or a dict holding it under "data").

Case data (`plugins/anthology-guardrails/scripts/` is the oracle):
- `{"op": "check", "tool": T, "input": I, "home": H}`: `guard_rules.check(T, I, H)`, rendered as the
  canonical JSON of `[rule, message]` or `null`. `check` turns any exception into `None`; this module
  calls `_check` itself, so an exception is seen: it answers `null` with `"divergence": "D-1"` (Python
  failed open; Rust may block or match).
- `{"op": "hook", "stdin_hex": X, "home": H|null}`: `guard.py` run as a child process with the bytes of
  `X` on stdin and `HOME=H` (unset when null), in plain mode. The result is its exit code, stdout and
  stderr.

Result: `{"exit": E, "stdout": S, "stderr": R}`; for "check", `E` is 0, `S` the canonical JSON
(`json.dumps(obj, sort_keys=True, separators=(",", ":"))`, Rust: `pycompat::json::dumps(.., &COMPACT_SORTED)`)
and `R` empty.

Python 3.9+ and the standard library only.
"""
from __future__ import annotations

import importlib.util
import json
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
SCRIPTS = os.path.join(ROOT, "plugins", "anthology-guardrails", "scripts")
GUARD = os.path.join(SCRIPTS, "guard.py")


def _load():
    path = os.path.join(SCRIPTS, "guard_rules.py")
    spec = importlib.util.spec_from_file_location("anthology_oracle_guard_rules_gd", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


G = _load()


def canonical(obj):
    return json.dumps(obj, sort_keys=True, separators=(",", ":"))


def _check(tool, tool_input, home):
    try:
        rule = G._check(tool, tool_input, home)
    except Exception as exc:  # check() would return None (allow): Rust may block or match (D-1)
        return {"exit": 0, "stdout": canonical(None), "stderr": "", "divergence": "D-1",
                "python": type(exc).__name__}
    result = [rule, G.MESSAGES[rule]] if rule else None
    return {"exit": 0, "stdout": canonical(result), "stderr": ""}


def _hook(stdin, home):
    env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "LANG": "C.UTF-8"}
    if home is not None:
        env["HOME"] = home
    proc = subprocess.run([sys.executable, "-I", GUARD], input=stdin, capture_output=True, env=env,
                          cwd=ROOT, timeout=120)
    return {"exit": proc.returncode, "stdout_hex": proc.stdout.hex(), "stderr_hex": proc.stderr.hex()}


def run(case):
    data = case["data"] if "data" in case and "op" not in case else case
    op = data["op"]
    if op == "check":
        return _check(data["tool"], data["input"], data["home"])
    if op == "hook":
        return _hook(bytes.fromhex(data["stdin_hex"]), data.get("home"))
    raise ValueError("guard oracle: unknown op %r" % (op,))


if __name__ == "__main__":
    print("guard oracle: loaded by port/difftest/oracle/_driver.py", file=sys.stderr)
    sys.exit(2)
