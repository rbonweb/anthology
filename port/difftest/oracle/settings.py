"""ST1 oracle for the `settings` difftest surface (plan ST1, GC-8): `settings_patch.py`.

Run as `python3 -I` (no script folder on `sys.path`): `json.py` beside this file shadows the
standard library's `json`. F4's driver loads this module by file path.

Library surface: `run(case: dict) -> dict`. `case["op"]` picks the call:

| op | call | stdout (json.dumps, ensure_ascii) |
|---|---|---|
| `plan` | `plan(text, ops)` | the planned records |
| `apply` | `apply(text, ops)` | `{"text", "diff"}` |
| `revert` | `revert(text, stored)` | `{"text", "diff"}` |
| `reconcile` | `reconcile(text, ops, stored)` | `{"text", "diff", "stored"}` |
| `redact` | `redact(text)` | `{"text"}` |
| `diff` | `_diff(text, text2)` | `{"diff"}` |
| `ops_version` | `ops_version(ops)` with `OPS_VERSION = case["ops_version"]` | `{"version"}` |
| `read_text` | `read_text(<ROOT>/f)` after the `fs` fixture | `{"text"}` |
| `settings_path` | `settings_path(env)` | `{"path"}` |
| `plugin_rule_status` | `plugin_rule_status(root, env)` after the `fs` fixture | `{"status"}` |
| `setup_ops` | `setup_ops(root, profile_roots, env)` after the `fs` fixture | `{"ops", "rule"}` |
| `write_text` | `write_text(path, text)` after the `fs` fixture | `{"tree"}` |

Filesystem ops build `case["fs"]` (entries `{"path", "type": "dir"|"file"|"link", "text" or
"hex", "mode", "target"}`; an entry that cannot be made is skipped) in a fresh temporary folder.
`<ROOT>` in any string of the case (and in a file's `text`) stands for that folder and `<REAL>`
for its realpath; both read `<ROOT>` again in the result. `env` is the `env=` argument;
`os.environ["HOME"]` follows `env["HOME"]` during the call (deleted when the case has none),
because `expanduser("~")` reads the process environment (the driver gives oracles no HOME).

Errors: a `ValueError` (with `JSONDecodeError` and `UnicodeDecodeError`) is exit 1 and
`str(exc)` on stderr: Rust must give the same text. An `OSError` is exit 1 and
`"<class> errno=<n>"` (the errno text is P1's surface). Any other exception is exit 1 with only
its class name, marked `D-1` (Python failed where it never meant to; Rust must fail with the
same class, its text is free). A `RecursionError` (a document deeper than CPython's C stack
allows) is marked `D-1` with the result Rust gives without that limit, which the case states
in `case["deep"]`: `"valid"` (Rust parses it and the scanner then refuses it as nesting too
deeply; `plugin_rule_status` answers `missing`), `"valid-ok"` (a settings.json that also holds
the plugin-read rule: `plugin_rule_status` answers `ok`) or `"invalid:<message>"`.

Python 3.9+ and the standard library only.
"""
from __future__ import annotations

import importlib.util
import json
import os
import shutil
import stat
import sys
import tempfile
import types
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
SCRIPTS = ROOT / "plugins" / "anthology" / "scripts"
GUARD_RULES = ROOT / "plugins" / "anthology-guardrails" / "scripts" / "guard_rules.py"


def _load(name, path):
    spec = importlib.util.spec_from_file_location(name, str(path))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


SP = _load("_anthology_oracle_settings_patch", SCRIPTS / "settings_patch.py")
_RULES = _load("_anthology_oracle_guard_rules", GUARD_RULES)
# setup_ops(profile_roots=None) imports guardrails_loc and asks it for the guardrails plugin's
# guard_rules; this stand-in answers with the repository's own guard_rules.py, so the default
# branch (roots plus `.config/` channel variants) runs as written.
_GUARDRAILS_LOC_STUB = types.SimpleNamespace(load_rules=lambda root, env=None: _RULES)


def _setup_ops(*args, **kwargs):
    """SP.setup_ops with the guardrails_loc stand-in installed only for the call: the driver runs
    every oracle module in one process, and a global stub would replace the real module that the
    runtime.guardrails_loc oracle loads."""
    saved = sys.modules.get("guardrails_loc")
    sys.modules["guardrails_loc"] = _GUARDRAILS_LOC_STUB
    try:
        return SP.setup_ops(*args, **kwargs)
    finally:
        if saved is None:
            sys.modules.pop("guardrails_loc", None)
        else:
            sys.modules["guardrails_loc"] = saved

PLACE = "<ROOT>"
REAL = "<REAL>"


def _subs(text, root):
    return text.replace(REAL, os.path.realpath(root)).replace(PLACE, root)


def _sub(v, root):
    """`v` with `<REAL>` and `<ROOT>` in every string replaced by the folder's realpath and the
    folder."""
    if isinstance(v, str):
        return _subs(v, root)
    if isinstance(v, list):
        return [_sub(x, root) for x in v]
    if isinstance(v, dict):
        return {k: _sub(x, root) for k, x in v.items()}
    return v


def _unsub(text, roots):
    for r in roots:
        text = text.replace(r, PLACE)
    return text


def _build(fs, root):
    """The fixture; like the Rust side, an entry that cannot be made is skipped."""
    for e in fs or []:
        p = os.path.join(root, e["path"])
        kind = e["type"]
        try:
            if kind == "dir":
                os.makedirs(p, exist_ok=True)
            elif kind == "file":
                os.makedirs(os.path.dirname(p), exist_ok=True)
                data = (_subs(e["text"], root).encode("utf-8") if "text" in e
                        else bytes.fromhex(e.get("hex", "")))
                with open(p, "wb") as fh:
                    fh.write(data)
            elif kind == "link":
                os.makedirs(os.path.dirname(p), exist_ok=True)
                os.symlink(_subs(e["target"], root), p)
            if "mode" in e:
                os.chmod(p, e["mode"])
        except OSError:
            pass


def _tree(root, roots):
    """Every entry under `root`, sorted: `[path, kind, mode, content]` (content: file hex or
    link target)."""
    out = []
    for dirpath, dirnames, filenames in os.walk(root):
        for name in sorted(dirnames + filenames):
            p = os.path.join(dirpath, name)
            rel = os.path.relpath(p, root)
            st = os.lstat(p)
            mode = "%04o" % stat.S_IMODE(st.st_mode)
            if stat.S_ISLNK(st.st_mode):
                out.append([rel, "link", "", _unsub(os.readlink(p), roots)])
            elif stat.S_ISDIR(st.st_mode):
                out.append([rel, "dir", mode, ""])
            else:
                with open(p, "rb") as fh:
                    out.append([rel, "file", mode, fh.read().hex()])
    out.sort()
    return out


def _ok(payload, roots):
    return {"exit": 0, "stdout": _unsub(json.dumps(payload), roots), "stderr": ""}


def _error(exc, case, roots):
    if isinstance(exc, RecursionError):
        deep = case.get("deep", "")
        if case["op"] == "plugin_rule_status" and deep in ("valid", "valid-ok"):
            status = "ok" if deep == "valid-ok" else "missing"
            return dict(_ok({"status": status}, roots), divergence="D-1", python=repr(exc))
        if deep == "valid":
            msg = "settings.json nests too deeply"
        elif deep.startswith("invalid:"):
            msg = deep[len("invalid:"):]
        else:
            msg = "RecursionError"
        return {"exit": 1, "stdout": "", "stderr": msg, "divergence": "D-1", "python": repr(exc)}
    if isinstance(exc, ValueError):
        return {"exit": 1, "stdout": "", "stderr": _unsub(str(exc), roots)}
    if isinstance(exc, OSError):
        return {"exit": 1, "stdout": "", "stderr": "%s errno=%s" % (type(exc).__name__, exc.errno)}
    return {"exit": 1, "stdout": "", "stderr": type(exc).__name__, "divergence": "D-1",
            "python": _unsub(repr(exc), roots)[:400]}


def _call(case, root):
    op = case["op"]
    c = _sub(case, root) if root else case
    text = c.get("text", "")
    if op == "plan":
        return SP.plan(text, c["ops"])
    if op == "apply":
        new, diff = SP.apply(text, c["ops"])
        return {"text": new, "diff": diff}
    if op == "revert":
        new, diff = SP.revert(text, c["stored"])
        return {"text": new, "diff": diff}
    if op == "reconcile":
        new, diff, stored = SP.reconcile(text, c["ops"], c["stored"])
        return {"text": new, "diff": diff, "stored": stored}
    if op == "redact":
        return {"text": SP.redact(text)}
    if op == "diff":
        return {"diff": SP._diff(text, c["text2"])}
    if op == "ops_version":
        saved = SP.OPS_VERSION
        SP.OPS_VERSION = c["ops_version"]
        try:
            return {"version": SP.ops_version(c["ops"])}
        finally:
            SP.OPS_VERSION = saved
    if op == "settings_path":
        return {"path": str(SP.settings_path(c["env"]))}
    if op == "read_text":
        return {"text": SP.read_text(Path(c["path"]))}
    if op == "plugin_rule_status":
        if case.get("deep") in ("valid", "valid-ok"):
            # plugin_rule_status swallows a RecursionError as "unreadable"; see it here.
            t = SP.read_text(SP.settings_path(c["env"]))
            json.loads(t[1:] if t.startswith("\ufeff") else t)
        return {"status": SP.plugin_rule_status(c["root"], c["env"])}
    if op == "setup_ops":
        ops = _setup_ops(c["root"], profile_roots=c.get("profile_roots"), env=c["env"])
        return {"ops": ops, "rule": SP.plugin_read_rule(c["root"])}
    if op == "write_text":
        SP.write_text(c["path"], text)
        return None
    raise ValueError("unknown case op %r" % (op,))


def run(case):
    root = None
    roots = []
    saved_home = os.environ.get("HOME")
    if "fs" in case:
        root = tempfile.mkdtemp(prefix="st1-oracle-")
        roots = [os.path.realpath(root), root]
        _build(case["fs"], root)
    env = case.get("env")
    if isinstance(env, dict):
        env_home = env.get("HOME")
        if env_home is None:
            os.environ.pop("HOME", None)
        else:
            os.environ["HOME"] = _subs(env_home, root) if root else env_home
    try:
        try:
            payload = _call(case, root)
        except Exception as exc:  # noqa: BLE001 - every exception is a result here
            return _error(exc, case, roots)
        if case["op"] == "write_text":
            payload = {"tree": _tree(root, roots)}
        return _ok(payload, roots)
    finally:
        if saved_home is None:
            os.environ.pop("HOME", None)
        else:
            os.environ["HOME"] = saved_home
        if root:
            shutil.rmtree(root, ignore_errors=True)
