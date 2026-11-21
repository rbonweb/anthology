"""RP1 oracle for `anthology-dev replay run` (plan RP1, design §13.7): the Python side of the
`check`, `analyze` and `privacy` comparisons on the owner's replay corpus.

Loaded by path through F4's `_driver.py` (module name `replay`), always as `python3 -I`:
`json.py` beside this file shadows the standard library's `json`.

Library surface: `run(case: dict) -> dict`. Cases:
- `{"op": "check", "envelope_hex": H, "home": S}`: the envelope bytes decoded exactly as
  `guard.py`'s `decide` does (lossy UTF-8, then `json.loads`), then `guard_rules.check(...)`.
  `{"verdict": "allow" | "block:<rule>" | "malformed"}` (`malformed`: not JSON, not an object,
  or `tool_name` not a string, which `guard.py` allows), or `{"raised": "<type>"}` when the
  decoding or `check` raised any other exception, such as `RecursionError` on deeply nested JSON
  (`guard.py` fails open: the D-1 case).
- `{"op": "analyze", "envelope_hex": H, "home": S}`: `guard_rules._analyze(command, home)` on
  the envelope's Bash `command`, as SH's oracle reports it (`Entry` minus `opaque`, groups
  numbered by first appearance), canonical JSON in `"analysis"`; `null` when the envelope has
  no Bash command; or `{"raised": "<type>"}`.
- `{"op": "privacy", "text": S, "home": S, "hostname": S}`:
  `memory_privacy.privacy_findings(text, home=, hostname=)` as `{"findings": [[line, kind], ...]}`,
  or `{"raised": "<type>"}`.

A lone surrogate in a result is replaced with U+FFFD and the answer carries `"d6": true` (D-6):
the driver forbids them, and Rust's decoder would make an unmarked one compare equal.

Python 3.9+ and the standard library only.
"""
from __future__ import annotations

import importlib.util
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
GUARD_RULES = ROOT / "plugins" / "anthology-guardrails" / "scripts" / "guard_rules.py"
PRIVACY = ROOT / "plugins" / "anthology" / "scripts" / "memory_privacy.py"


def _load(name, path):
    spec = importlib.util.spec_from_file_location(name, str(path))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


G = _load("anthology_oracle_replay_guard_rules", GUARD_RULES)
P = _load("anthology_oracle_replay_privacy", PRIVACY)

_SURROGATE = re.compile("[\ud800-\udfff]")


def _clean(obj, flag):
    """`obj` with lone surrogates replaced by U+FFFD; `flag[0]` set when any was."""
    if isinstance(obj, str):
        if _SURROGATE.search(obj):
            flag[0] = True
            return _SURROGATE.sub("\ufffd", obj)
        return obj
    if isinstance(obj, list):
        return [_clean(x, flag) for x in obj]
    if isinstance(obj, dict):
        return {_clean(k, flag): _clean(v, flag) for k, v in obj.items()}
    return obj


def _decode(envelope_hex):
    """`guard.py`'s `decide` up to the check: the payload dict, or None when it is malformed.
    Any other exception (RecursionError on deep nesting) propagates: the caller reports it as
    raised, since `guard.py` fails open on it just the same."""
    raw = bytes.fromhex(envelope_hex)
    try:
        data = json.loads(raw.decode("utf-8", errors="replace"))
    except ValueError:  # JSONDecodeError
        return None
    if not isinstance(data, dict) or not isinstance(data.get("tool_name"), str):
        return None
    return data


class _Groups:
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


def _analysis(command, home):
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


def _check(case):
    try:
        data = _decode(case["envelope_hex"])
        if data is None:
            return {"verdict": "malformed"}
        hit = G.check(data["tool_name"], data.get("tool_input"), case["home"])
    except Exception as exc:  # guard.py turns any exception into "allow"
        return {"raised": type(exc).__name__}
    return {"verdict": "allow" if hit is None else "block:%s" % hit[0]}


def _analyze(case):
    try:
        data = _decode(case["envelope_hex"])
    except Exception as exc:  # RecursionError on deep nesting: guard.py allows
        return {"raised": type(exc).__name__}
    tool_input = data.get("tool_input") if data is not None else None
    if data is None or data["tool_name"] != "Bash" or not isinstance(tool_input, dict) \
            or not isinstance(tool_input.get("command"), str):
        return {"analysis": None}
    try:
        result = _analysis(tool_input["command"], case["home"])
    except Exception as exc:  # RecursionError on hostile nesting: guard.py allows
        return {"raised": type(exc).__name__}
    flag = [False]
    result = _clean(result, flag)
    answer = {"analysis": json.dumps(result, sort_keys=True, separators=(",", ":"))}
    if flag[0]:
        answer["d6"] = True
    return answer


def _privacy(case):
    try:
        found = P.privacy_findings(case["text"], home=case["home"], hostname=case["hostname"])
    except Exception as exc:
        return {"raised": type(exc).__name__}
    return {"findings": [[f["line"], f["kind"]] for f in found]}


_OPS = {"check": _check, "analyze": _analyze, "privacy": _privacy}


def run(case):
    return _OPS[case["op"]](case)
