"""SH oracle for the `shell.segments` and `shell.analyze` difftest surfaces (plan SH1, SH2; design §7.1, §13.2).

    python3 -I port/difftest/oracle/shell.py --batch < cases.jsonl > results.jsonl

Run as `python3 -I` (no script folder on `sys.path`): `json.py` beside this file shadows the standard
library's `json`. F4's driver loads this module by file path.

Library surface: `run(case: dict) -> dict` (the case's `data`, or a dict holding it under "data").

Case data (`plugins/anthology-guardrails/scripts/guard_rules.py` is the oracle):
- `{"op": "segments", "text": T, "depth": D}`: `_segments(T, D)`, reported with `lenient` (the top-level
  `_Lexer` raised `_Unbalanced`, so `_lenient_segments` produced the segments) and `incomplete` (always
  false: Python has no backtrack limit).
- `{"op": "shell_parse", "prog": P, "args": [..], "i": I}`: `_shell_parse(P, args, I)`.
- `{"op": "script_starts", "words": [..]}`: `_script_starts(words)`.
- `{"op": "analyze", "command": C, "home": H}`: `_analyze(C, H)`, every `_Entry` field (`Entry` minus
  `opaque`).

Groups are identified by object identity in Python; a result numbers them by first appearance (segments in
order, entries in order) and lists each group's texts in that order.

Result: `{"exit": 0, "stdout": <canonical JSON of the result>, "stderr": ""}`, canonical meaning
`json.dumps(obj, sort_keys=True, separators=(",", ":"))` (Rust: `pycompat::json::dumps(.., &COMPACT_SORTED)`).
When Python raises (RecursionError on hostile nesting), `{"exit": 1, "stdout": "", "stderr": "<type>",
"divergence": "D-1"}`: guard.py turns any exception into "allow", so Rust may block or match (GC-16 D-1).

`--batch`: JSON lines `{"id": ID, "data": DATA}` on stdin, JSON lines `{"id": ID, "out": run(DATA)}` on stdout
(canonical form), in input order.

Python 3.9+ and the standard library only.
"""
from __future__ import annotations

import importlib.util
import json
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
GUARD_RULES = os.path.join(ROOT, "plugins", "anthology-guardrails", "scripts", "guard_rules.py")


def _load():
    spec = importlib.util.spec_from_file_location("anthology_oracle_guard_rules", GUARD_RULES)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


G = _load()


def canonical(obj):
    return json.dumps(obj, sort_keys=True, separators=(",", ":"))


class _Groups:
    """Ordinal of each group object, by first appearance; keeps the objects alive while numbering."""

    def __init__(self):
        self.ids = {}
        self.objects = []

    def ordinal(self, group):
        key = id(group)
        if key not in self.ids:
            self.ids[key] = len(self.objects)
            self.objects.append(group)
        return self.ids[key]

    def texts(self):
        return [list(group.texts) for group in self.objects]


def _segments(text, depth):
    try:
        segs, lenient = G._Lexer(text, depth).run(), False
    except G._Unbalanced:
        segs, lenient = G._lenient_segments(text), True
    groups = _Groups()
    out = [{"tokens": list(seg.tokens), "redirs": [list(r) for r in seg.redirs],
            "group": groups.ordinal(seg.group)} for seg in segs]
    return {"lenient": lenient, "incomplete": False, "segments": out, "groups": groups.texts()}


def _shell_parse(prog, args, i):
    mode, start, scripts = G._shell_parse(prog, list(args), i)
    return {"mode": mode, "start": start, "scripts": [list(s) for s in scripts]}


def _script_starts(words):
    return {"starts": [list(s) for s in G._script_starts(list(words))]}


def _analyze(command, home):
    entries = G._analyze(command, home)
    groups = _Groups()
    out = []
    for e in entries:
        out.append({
            "raw": list(e.raw), "redirs": [list(r) for r in e.redirs], "prog_path": e.prog_path,
            "prog": e.prog, "args": list(e.args), "cwd": e.cwd, "exempt": e.exempt,
            "group": groups.ordinal(e.group), "code": list(e.code),
        })
    return {"incomplete": False, "entries": out, "groups": groups.texts()}


def _call(data):
    """The function and its arguments for a case (a malformed case is a harness bug: it raises here)."""
    op = data["op"]
    if op == "segments":
        return _segments, (data["text"], data.get("depth", 0))
    if op == "shell_parse":
        return _shell_parse, (data["prog"], data["args"], data.get("i", 0))
    if op == "script_starts":
        return _script_starts, (data["words"],)
    if op == "analyze":
        return _analyze, (data["command"], data["home"])
    raise ValueError("shell oracle: unknown op %r" % (op,))


def run(case):
    data = case["data"] if "data" in case and "op" not in case else case
    function, args = _call(data)
    try:
        result = function(*args)
    except Exception as exc:  # RecursionError and anything else guard.py would turn into "allow"
        return {"exit": 1, "stdout": "", "stderr": type(exc).__name__, "divergence": "D-1"}
    return {"exit": 0, "stdout": canonical(result), "stderr": ""}


def main(argv):
    if argv != ["--batch"]:
        print("usage: python3 -I port/difftest/oracle/shell.py --batch < cases.jsonl", file=sys.stderr)
        return 2
    out = sys.stdout
    for line in sys.stdin:
        if not line.strip():
            continue
        case = json.loads(line)
        out.write(canonical({"id": case["id"], "out": run(case["data"])}) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
