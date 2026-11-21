"""RT1b oracle for the `doctor` difftest surface (doctor.py).

F4's driver loads this module by file path; it runs as `python3 -I` (`json.py` beside this file
shadows the standard library's `json`). Surface: `run(case: dict) -> dict`.

Case: `{"op": OP, "tree": [ENTRY, ...], "env": {K: V}, "plugin_root": P, "cwd": C, ...}`. The
tree is built under a fresh temporary folder at its real path (the root) by the twin of
`anthology_testkit::rt_a::materialize` (`guardrails_loc.py` beside this file, loaded by path).
`@ROOT@` in env values and paths is the root.

Fixed for every case (the GC-19 seams, as the Python tests patch them):
- `doctor.SYSTEM_APPLICATIONS` is `<root>/SysApps`; `doctor.MANAGED_MCP_PATHS` is
  `(<root>/managed-mcp.json,)`; `compat.MANAGED_SETTINGS_PATHS` is
  `(<root>/managed-settings.json,)`;
- `shutil.which` and `subprocess.run` are the real ones, on the case's `PATH` (fake `node`,
  `docker` scripts and the like are in the tree);
- F-9: `compat.wrapper_problems` also checks `bin/anthology` first, as Rust does;
- F-5: Rust's first check is `anthology_binary` where Python's is `python3`. Both sides mask the
  first check: in a list as `{"name": "<F-5>", "ok": null, "detail": ""}`, in text output as the
  line `<F-5>`.

Ops and their stdout:
- `check_env` (`lock`: a path or null, `plugin_root`, `cwd`): the masked checks,
  `json.dumps(checks, indent=2)`;
- `find_mcp_servers` (`cwd`): `json.dumps(servers, indent=2)`;
- `duplicates` (`installed`: any JSON value): `json.dumps(find_duplicates(installed), indent=2)`;
- `main` (`json`: bool): `doctor.main(["--json"] or [])` with `os.environ` = env,
  `os.getcwd()` = `cwd`, the plugin root `env["CLAUDE_PLUGIN_ROOT"]` and its own `rtk.lock`
  (`DEFAULT_LOCK`, the plugin's lock): exit = main's return, stdout masked (JSON: checked to be
  exactly `json.dumps(…, indent=2)`, then re-dumped that way after masking).

Every root prefix is written `@ROOT@`. An exception is exit 1 with `"<type>: <message>"` on
stderr (the generator never builds a layout on which Python raises).

Python 3.9+ and the standard library only.
"""
from __future__ import annotations

import contextlib
import importlib.util
import io
import json
import os
import shutil
import sys
import tempfile
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
SCRIPTS = ROOT / "plugins" / "anthology" / "scripts"
if str(SCRIPTS) not in sys.path:
    sys.path.append(str(SCRIPTS))

import compat  # noqa: E402
import doctor  # noqa: E402

_spec = importlib.util.spec_from_file_location("difftest_oracle_rt1b_layout", str(HERE / "guardrails_loc.py"))
_layout = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_layout)

import settings_patch  # noqa: E402

MASK = {"name": "<F-5>", "ok": None, "detail": ""}
_WRAPPER_PROBLEMS = compat.wrapper_problems
_RULE_STATUS = settings_patch.plugin_rule_status
_MARKS = []


def _rule_status(plugin_root, env=None):
    """D-1 (ST1's ruling): on a settings.json too deep for CPython's json.loads, Python's
    plugin_rule_status catches the RecursionError and says "unreadable"; Rust reads the document.
    The generator's deep documents are bare lists (no `permissions`), so Rust says "missing"."""
    status = _RULE_STATUS(plugin_root, env)
    if status == "unreadable":
        try:
            text = settings_patch.read_text(settings_patch.settings_path(env))
            json.loads(text[1:] if text.startswith("\ufeff") else text)
        except RecursionError:
            _MARKS.append("D-1")
            return "missing"
        except Exception:  # noqa: BLE001 (really unreadable)
            pass
    return status


def _wrappers(plugin_root):
    path = Path(plugin_root) / "bin" / "anthology"  # F-9: the new wrapper, checked the same way
    first = [] if (path.is_file() and os.access(str(path), os.X_OK)) else ["anthology"]
    return first + _WRAPPER_PROBLEMS(plugin_root)


def _mask_checks(checks):
    return [MASK] + list(checks[1:])


def _mask_text(out):
    lines = out.split("\n")
    return "\n".join(["<F-5>"] + lines[1:]) if lines and lines[0] else out


def _dump(value):
    return json.dumps(value, indent=2)


def _op(case, root):
    expand = lambda s: _layout._expand(s, root)  # noqa: E731
    env = {k: expand(v) for k, v in (case.get("env") or {}).items()}
    op = case["op"]
    arg = lambda k: None if case.get(k) is None else expand(case[k])  # noqa: E731
    if op == "check_env":
        checks = doctor.check_env(env, lock=arg("lock"), plugin_root=arg("plugin_root"), cwd=arg("cwd"))
        return {"exit": 0, "stdout": _dump(_mask_checks(checks))}
    if op == "find_mcp_servers":
        return {"exit": 0, "stdout": _dump(doctor.find_mcp_servers(env, arg("cwd")))}
    if op == "duplicates":
        return {"exit": 0, "stdout": _dump(doctor.find_duplicates(case.get("installed")))}
    if op == "main":
        buf = io.StringIO()
        plugin = env.get("CLAUDE_PLUGIN_ROOT")
        lock = Path(plugin) / "rtk.lock" if plugin else doctor.DEFAULT_LOCK
        with mock.patch.dict(os.environ, env, clear=True), \
                mock.patch.object(doctor.os, "getcwd", return_value=arg("cwd")), \
                mock.patch.object(doctor, "DEFAULT_LOCK", lock), \
                contextlib.redirect_stdout(buf):
            code = doctor.main(["--json"] if case.get("json") else [])
        out = buf.getvalue()
        if case.get("json"):
            data = json.loads(out)
            if out != _dump(data) + "\n":  # never: main prints exactly this dump
                raise ValueError("main's JSON is not json.dumps(indent=2)")
            data["checks"] = _mask_checks(data["checks"])
            return {"exit": code, "stdout": _dump(data) + "\n"}
        return {"exit": code, "stdout": _mask_text(out)}
    raise KeyError("unknown op %r" % op)


def run(case):
    """What Rust must produce for `case` (module docstring)."""
    root = os.path.realpath(tempfile.mkdtemp(prefix="rt1b-"))
    del _MARKS[:]
    try:
        _layout.materialize(root, case.get("tree") or [])
        with mock.patch.object(doctor, "SYSTEM_APPLICATIONS", Path(root) / "SysApps"), \
                mock.patch.object(doctor, "MANAGED_MCP_PATHS", (Path(root) / "managed-mcp.json",)), \
                mock.patch.object(compat, "MANAGED_SETTINGS_PATHS", (Path(root) / "managed-settings.json",)), \
                mock.patch.object(compat, "wrapper_problems", _wrappers), \
                mock.patch.object(settings_patch, "plugin_rule_status", _rule_status):
            try:
                result = _op(case, root)
            except Exception as exc:  # the oracle reports what Python raised
                return {"exit": 1, "stdout": "", "stderr": "%s: %s" % (type(exc).__name__, exc)}
        result["stdout"] = result["stdout"].replace(root, "@ROOT@")
        result.setdefault("stderr", "")
        if _MARKS:
            result["divergence"] = "D-1"
            result["python"] = "plugin_rule_status: unreadable (RecursionError)"
        return result
    finally:
        _layout._unlock(root)
        shutil.rmtree(root, ignore_errors=True)
