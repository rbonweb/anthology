"""F3 oracle for the `core.state` difftest surface (plan F3, GC-3).

    python3 -I port/difftest/oracle/core_state.py --golden PATH

Run as `python3 -I` (no script folder on `sys.path`): `json.py` beside this file shadows the
standard library's `json`. F4's driver loads this module by file path.

Library surface: `run(case: dict) -> dict`.

Case: `{"op": "state_write", "prior": HEX | null, "kv": [[KEY, VALUE], ...]}`.
- `prior` is the bytes of `<config>/anthology/state.json` before the call, in hex, or null for
  no file.
- `kv` is `state.write(**kv)`'s keyword arguments, in order (JSON values).

`plugins/anthology/scripts/state.py` runs with `CLAUDE_CONFIG_DIR` set to a fresh temporary
folder. Result: `{"exit": 0, "stdout": OUT, "stderr": ""}` where `OUT` is
`"<mode> <names>\\n"` and then the exact text of state.json: `<mode>` is its permission bits
in octal (`0600`) and `<names>` the folder's entries, sorted and comma-separated (so a left-over
temp file shows). On an exception: exit 1, stdout "", stderr `"<type>: <str(exc)>"` with the
temporary folder written as `<config>`.

D-6: when the prior state holds a lone surrogate, Python writes the whole file with
`ensure_ascii=True` (its fallback), while Rust has U+FFFD there and writes UTF-8. The result is
then Python's with each lone surrogate replaced by U+FFFD at decode time, and carries
`"divergence": "D-6"` and `"python"` (Python's own result).

`--golden PATH` writes the frozen corpus, one JSON line `{"case": ..., "result": ...}` per case,
from fixed cases and fixed seeds. `crates/anthology-core/tests/golden.rs` replays it in
`cargo test`; `ci/jobs/difftest-linux.sh` regenerates it in the pinned 3.12 image and compares
it byte for byte with the committed file.

Python 3.9+ and the standard library only.
"""
from __future__ import annotations

import importlib.util
import json
import os
import random
import re
import shutil
import sys
import tempfile
import types
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
STATE_PY = ROOT / "plugins" / "anthology" / "scripts" / "state.py"

_SURROGATE = re.compile("[\ud800-\udfff]")


def _load_state():
    spec = importlib.util.spec_from_file_location("_anthology_oracle_state", str(STATE_PY))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


_STATE = _load_state()


def _has_surrogate(value):
    stack = [value]
    while stack:
        v = stack.pop()
        if isinstance(v, str):
            if _SURROGATE.search(v):
                return True
        elif isinstance(v, list):
            stack.extend(v)
        elif isinstance(v, dict):
            for k, item in v.items():
                if _SURROGATE.search(k):
                    return True
                stack.append(item)
    return False


def _fix(v):
    if isinstance(v, str):
        return _SURROGATE.sub("\ufffd", v)
    if isinstance(v, list):
        return [_fix(x) for x in v]
    return v


def _fix_pairs(pairs):
    d = {}
    for k, v in pairs:
        d[_SURROGATE.sub("\ufffd", k)] = _fix(v)
    return d


def _loads_d6(text, **kw):
    return _fix(json.loads(text, object_pairs_hook=_fix_pairs, **kw))


_D6_JSON = types.SimpleNamespace(loads=_loads_d6, dumps=json.dumps)


def _python(case, json_module):
    """Runs `state.write` in a fresh config folder with `json_module` as state.py's `json`."""
    config = tempfile.mkdtemp(prefix="core-state-")
    saved_env = os.environ.get("CLAUDE_CONFIG_DIR")
    saved_json = _STATE.json
    os.environ["CLAUDE_CONFIG_DIR"] = config
    _STATE.json = json_module
    try:
        home = Path(config) / "anthology"
        if case["prior"] is not None:
            home.mkdir()
            (home / "state.json").write_bytes(bytes.fromhex(case["prior"]))
        prior = _STATE.read()
        try:
            _STATE.write(**dict(case["kv"]))
        except Exception as exc:  # the oracle reports what Python raised
            msg = "%s: %s" % (type(exc).__name__, exc)
            return {"exit": 1, "stdout": "", "stderr": msg.replace(config, "<config>")}, prior
        path = home / "state.json"
        mode = os.stat(str(path)).st_mode & 0o7777
        names = ",".join(sorted(os.listdir(str(home))))
        text = path.read_bytes().decode("utf-8")
        return {"exit": 0, "stdout": "%04o %s\n%s" % (mode, names, text), "stderr": ""}, prior
    finally:
        _STATE.json = saved_json
        if saved_env is None:
            os.environ.pop("CLAUDE_CONFIG_DIR", None)
        else:
            os.environ["CLAUDE_CONFIG_DIR"] = saved_env
        shutil.rmtree(config, ignore_errors=True)


def run(case):
    """What Rust must produce for `case` (module docstring)."""
    if case.get("op") != "state_write":
        raise KeyError("unknown op %r" % case.get("op"))
    result, prior = _python(case, json)
    if _has_surrogate(prior):
        expect, _ = _python(case, _D6_JSON)
        return dict(expect, divergence="D-6", python=result)
    return result


# ---------------------------------------------------------------------------------------------
# The golden corpus

def _hex(text):
    return text.encode("utf-8").hex() if isinstance(text, str) else text.hex()


_PRIORS = [
    None,
    b"",
    b"{",
    b"not json",
    b"\xef\xbb\xbf{\"a\": 1}",                     # a BOM: read_text keeps it, loads refuses it
    b"{\"a\": \"\xff\"}",                           # not UTF-8
    b"{\"a\": \"\xc3\"}",                           # a truncated sequence
    b"[1, 2]",
    b"\"s\"",
    b"null",
    b"42",
    b"true",
    b"{}",
    b"  {}  \n",
    b"{\r\n  \"a\" : 1 \r\n}\r\n",
    "{\"rtk_path\": \"/old/rtk\", \"data_dir\": \"/d\"}",
    "{\"rtk_path\": \"/home/\u00e9/rtk\", \"z\u20ac\": 1, \"\U0001f600\": \"\u2028\"}",
    "{\"rtk_path\": \"/home/\\u00e9/rtk\", \"esc\": \"\\\"\\\\\\/\\b\\f\\n\\r\\t\\u0000\\u001f\"}",
    "{\"agent_memory_warned\": {\"/r/b\": true, \"/r/a\": true},"
    " \"settings_ops\": [{\"op\": \"add\", \"path\": [\"permissions\", \"allow\"],"
    " \"value\": \"Bash(rtk *)\"}], \"n\": {\"deep\": [[[{\"x\": null}]]], \"e\": {}, \"l\": []}}",
    "{\"rtk_path\": \"/x/\\udcff/rtk\", \"k\": 1}",     # a lone surrogate (D-6)
    "{\"\\ud800k\": 1, \"m\": \"\u00e9\"}",             # a lone surrogate in a key (D-6)
    "{\"a\": \"\\ud83d\\ude00 pair\"}",                 # a valid pair: no divergence
    "{\"a\": 1, \"a\": 2, \"b\": {\"c\": 1, \"c\": [3]}}",
    "{\"f\": 1e16, \"g\": -0.0, \"h\": 5e-324, \"i\": NaN, \"j\": Infinity, \"k\": -Infinity,"
    " \"l\": 0.1, \"m\": 1E400}",
    "{\"big\": 123456789012345678901234567890, \"neg\": -9223372036854775809}",
    "{\"Z\": 1, \"a\": 2, \"\u00e9\": 3, \"\uffff\": 4, \"\U00010000\": 5, \"\": 6, \"10\": 7,"
    " \"9\": 8}",
    "{\"a\": " + "[" * 50 + "1" + "]" * 50 + "}",
]

_KVS = [
    [],
    [["rtk_path", "/p/rtk"]],
    [["data_dir", "/data"], ["rtk_path", "/home/\u00e9/bin/rtk"]],
    [["setup_done", "0.2.0"], ["settings_ops_version", "1-0123456789abcdef"],
     ["settings_created", True]],
    [["agent_memory_warned", {"/repo": True}]],
    [["settings_ops", [{"op": "add", "value": ["x", 1, None]}]], ["setup_done", "unknown"]],
    [["a", None], ["n", 5], ["f", 1.5], ["g", 1e16], ["h", -0.0]],
    [["rtk_path", "/tab\tand\"quote\\ \u0001 \u007f \u2028 \U0001f600"]],
    [["\u00e9", 1], ["Z", 2], ["a", 3], ["\U0001f600", 4], ["\uffff", 5]],
    [["b", {"c": {"d": []}}], ["a", {}]],
]


def _fixed_cases():
    for prior in _PRIORS:
        for kv in _KVS:
            yield {"op": "state_write", "prior": None if prior is None else _hex(prior),
                   "kv": kv}


def _rand_str(r, n_max=6):
    out = []
    for _ in range(r.randrange(n_max + 1)):
        k = r.random()
        if k < 0.6:
            out.append(chr(r.randrange(0x20, 0x7F)))
        elif k < 0.7:
            out.append(chr(r.randrange(0, 0x20)))
        elif k < 0.85:
            c = r.randrange(0x80, 0x10000)
            out.append(chr(c) if not 0xD800 <= c <= 0xDFFF else "\ufffd")
        else:
            out.append(chr(r.randrange(0x10000, 0x110000)))
    return "".join(out)


def _rand_value(r, depth=0):
    k = r.random()
    if depth < 3 and k < 0.15:
        return [_rand_value(r, depth + 1) for _ in range(r.randrange(4))]
    if depth < 3 and k < 0.3:
        return {_rand_str(r): _rand_value(r, depth + 1) for _ in range(r.randrange(4))}
    if k < 0.4:
        return r.choice([None, True, False])
    if k < 0.55:
        return r.randrange(-10 ** r.randrange(1, 25), 10 ** r.randrange(1, 25))
    if k < 0.65:
        return r.choice([0.5, -0.0, 1e16, 1e-7, 123.456, 2.5e-308, 1e300, r.random()])
    return _rand_str(r, 12)


def _random_cases(r, n):
    keys = ["data_dir", "rtk_path", "setup_done", "settings_ops_version", "settings_ops",
            "settings_created", "agent_memory_warned"]
    for _ in range(n):
        prior = None
        if r.random() < 0.85:
            obj = {}
            for _ in range(r.randrange(6)):
                obj[r.choice(keys) if r.random() < 0.5 else _rand_str(r)] = _rand_value(r)
            text = json.dumps(obj, indent=r.choice([None, 2]), ensure_ascii=r.random() < 0.5,
                              sort_keys=r.random() < 0.3)
            prior = _hex(text)
        kv = [[r.choice(keys) if r.random() < 0.6 else _rand_str(r), _rand_value(r)]
              for _ in range(r.randrange(4))]
        kv = list(dict(kv).items())  # **kv has unique names
        yield {"op": "state_write", "prior": prior, "kv": [list(p) for p in kv]}


def _cases():
    yield from _fixed_cases()
    yield from _random_cases(random.Random(20261121), 300)


def golden(path):
    lines = []
    for case in _cases():
        lines.append(json.dumps({"case": case, "result": run(case)}) + "\n")
    Path(path).write_text("".join(lines), encoding="ascii", newline="\n")


def main(argv):
    if len(argv) == 2 and argv[0] == "--golden":
        golden(argv[1])
        return 0
    sys.stderr.write("usage: python3 -I port/difftest/oracle/core_state.py --golden PATH\n")
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
