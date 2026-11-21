"""RT1a oracle for the `runtime.guardrails_loc` difftest surface (guardrails_loc.py, compat.py).

F4's driver loads this module by file path; it runs as `python3 -I` (`json.py` beside this file
shadows the standard library's `json`). Library surface: `run(case: dict) -> dict`.

Case: `{"op": OP, "tree": [ENTRY, ...], "env": {K: V}, ...}`. The tree is built under a fresh
temporary folder at its real path (the root), exactly as `anthology_testkit::rt_a::materialize`
builds it (see that module for the entry kinds). `@ROOT@` in env values, `plugin_root`, `cwd`
and `path` is the root. A `guard` entry writes both `scripts/guard.py` (what Python looks for)
and `hooks/guard.sh` (what Rust looks for, F-6), so the two sides see the same plugins.

Ops and their result (a JSON object, every root prefix written as `@ROOT@`):
- `inspect` (`plugin_root`, `cwd`): `{"root": str|null, "problem": str|null}`;
- `disabled` (`cwd`): `{"disabled": bool}` (`guardrails_loc._disabled`);
- `wrappers` (`plugin_root`): `{"bad": [...]}`. F-9: `anthology` is checked first, the same
  way as the others, then `compat.wrapper_problems`;
- `agent_memory` (`cwd`): `{"bad": [...], "good": [...]}`;
- `subagent_model` (`cwd`): `{"source": str|null}`;
- `repo_top` (`cwd`): `{"top": str|null}`;
- `read_json` (`path`, `limit`: int|null): `{"value": ...}` (`compat.read_json`), or
  `{"found": bool}` with `"found_only": true` (for values too deep to print);
- `decode` (`hex`): `{"text": str}` (`compat.decode_text`).

The answer is `{"exit": 0, "stdout": json.dumps(result, sort_keys=True), "stderr": ""}`; an
exception is exit 1 with `"<type>: <message>"` on stderr (an unexplained diff: the generator
never builds a layout on which Python raises).

Python 3.9+ and the standard library only.
"""
from __future__ import annotations

import json
import os
import shutil
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
SCRIPTS = ROOT / "plugins" / "anthology" / "scripts"
if str(SCRIPTS) not in sys.path:
    sys.path.append(str(SCRIPTS))  # compat imports memory_sync lazily

import compat  # noqa: E402
import guardrails_loc  # noqa: E402

GUARD_BODY = b"#!/bin/sh\nexit 2\n"


def _expand(s, root, entry=None):
    out = s.replace("@ROOT@", root)
    entry = entry or {}
    if "@PAD@" in out:
        out = out.replace("@PAD@", "A" * max(int(entry.get("pad") or 0), 0))
    if "@DEEP@" in out:
        n = max(int(entry.get("deep") or 0), 0)
        out = out.replace("@DEEP@", "[" * n + "]" * n)
    return out


def _encode(text, enc):
    if enc == "utf8":
        return text.encode("utf-8")
    if enc == "utf8-bom":
        return b"\xef\xbb\xbf" + text.encode("utf-8")
    if enc == "utf16le-bom":
        return b"\xff\xfe" + text.encode("utf-16-le")
    if enc == "utf16be-bom":
        return b"\xfe\xff" + text.encode("utf-16-be")
    raise ValueError("unknown encoding %r" % enc)


def _parents(path):
    os.makedirs(os.path.dirname(path), exist_ok=True)


def materialize(root, spec):
    """The twin of `anthology_testkit::rt_a::materialize`."""
    for entry in spec:
        kind = entry["t"]
        if entry["p"].startswith("/"):
            raise ValueError("%r is not relative" % entry["p"])
        path = os.path.join(root, entry["p"])
        if kind == "dir":
            os.makedirs(path, exist_ok=True)
        elif kind == "file":
            _parents(path)
            if "hex" in entry:
                data = bytes.fromhex(entry["hex"])
            elif "text" in entry:
                data = _encode(_expand(entry["text"], root, entry), entry.get("enc", "utf8"))
            else:
                data = b""
            if "tail" in entry:
                data += bytes.fromhex(entry["tail"])
            with open(path, "wb") as fh:
                fh.write(data)
        elif kind == "guard":
            how = entry.get("kind", "file")
            for rel in ("scripts/guard.py", "hooks/guard.sh"):
                f = os.path.join(path, rel)
                _parents(f)
                if how == "file":
                    with open(f, "wb") as fh:
                        fh.write(GUARD_BODY)
                elif how == "dir":
                    os.makedirs(f, exist_ok=True)
                elif how == "link":
                    real = os.path.join(path, "guard-real")
                    with open(real, "wb") as fh:
                        fh.write(GUARD_BODY)
                    os.symlink(real, f)
                elif how == "dangling":
                    os.symlink(os.path.join(path, "no-such-guard"), f)
                else:
                    raise ValueError("unknown guard kind %r" % how)
        elif kind == "link":
            _parents(path)
            os.symlink(_expand(entry["to"], root), path)
        elif kind == "fifo":
            _parents(path)
            os.mkfifo(path)
        elif kind == "chmod":
            os.chmod(path, entry["mode"])
        else:
            raise ValueError("unknown entry %r" % kind)


def _unlock(root):
    for dirpath, dirnames, _ in os.walk(root):
        for d in dirnames:
            path = os.path.join(dirpath, d)
            if os.path.islink(path):
                continue  # chmod follows links: never touch a folder outside the root
            try:
                os.chmod(path, 0o755)
            except OSError:
                pass


def _norm(value, root):
    if value is None:
        return None
    return str(value).replace(root, "@ROOT@")


def _wrappers(plugin_root):
    path = Path(plugin_root) / "bin" / "anthology"  # F-9: the new wrapper, checked the same way
    first = [] if (path.is_file() and os.access(str(path), os.X_OK)) else ["anthology"]
    return first + compat.wrapper_problems(plugin_root)


def _op(case, root):
    env = {k: _expand(v, root) for k, v in (case.get("env") or {}).items()}
    op = case["op"]
    arg = lambda k: _expand(case[k], root)  # noqa: E731
    if op == "inspect":
        found, problem = guardrails_loc.inspect(arg("plugin_root"), env, arg("cwd"))
        return {"root": _norm(found, root), "problem": problem}
    if op == "disabled":
        return {"disabled": guardrails_loc._disabled(env, arg("cwd"))}
    if op == "wrappers":
        return {"bad": _wrappers(arg("plugin_root"))}
    if op == "agent_memory":
        bad, good = compat.agent_memory_problems(arg("cwd"))
        return {"bad": bad, "good": good}
    if op == "subagent_model":
        return {"source": compat.subagent_model_source(env, arg("cwd"))}
    if op == "repo_top":
        return {"top": _norm(compat.repo_top(arg("cwd")), root)}
    if op == "read_json":
        value = compat.read_json(arg("path"), case.get("limit"))
        if case.get("found_only"):  # a value too deep for json.dumps to print
            return {"found": value is not None}
        return {"value": value}
    if op == "decode":
        return {"text": compat.decode_text(bytes.fromhex(case["hex"]))}
    raise KeyError("unknown op %r" % op)


def run(case):
    """What Rust must produce for `case` (module docstring)."""
    root = os.path.realpath(tempfile.mkdtemp(prefix="rt1a-"))
    try:
        materialize(root, case.get("tree") or [])
        try:
            result = _op(case, root)
        except Exception as exc:  # the oracle reports what Python raised
            return {"exit": 1, "stdout": "", "stderr": "%s: %s" % (type(exc).__name__, exc)}
        return {"exit": 0, "stdout": json.dumps(result, sort_keys=True), "stderr": ""}
    finally:
        _unlock(root)
        shutil.rmtree(root, ignore_errors=True)
