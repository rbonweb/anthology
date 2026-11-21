"""RT2 oracle for the `rtk_install.reconcile` difftest surface (plan RT2, GC-9).

Run by F4's driver (`python3 -I port/difftest/oracle/_driver.py`), which loads this module by
file path; `json.py` beside this file shadows the standard library's `json`, so this folder is
never on `sys.path`.

Library surface: `run(case: dict) -> dict`. The case layout is documented in
`crates/difftest/src/surfaces/rtk_install_reconcile.rs`; in short: a lock JSON value, a
platform, a tarball (hex) the fake fetch serves at the lock's release URL (or a URL miss, or an
error with an exact message), files to create under `<tmp>/data` first, a prior state.json
`rtk_path`, and optional `MAX_BINARY_BYTES` / `MAX_TAR_MEMBERS` patches.

`plugins/anthology/scripts/rtk_install.py` runs with `CLAUDE_CONFIG_DIR` in a fresh temporary
folder. An invalid lock is `{"exit": 2, "stderr": "rtk_install: <msg>\\n"}` (what the CLI
prints); otherwise exit 0 with a text report of `reconcile(...)`: the result fields, each fetch
call, every entry under `data/` and state.json's `rtk_path`, with the temporary folder written
as `<tmp>`.

Python 3.9+ and the standard library only.
"""
from __future__ import annotations

import hashlib
import importlib.util
import os
import shutil
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
SCRIPTS = ROOT / "plugins" / "anthology" / "scripts"


def _load(name):
    spec = importlib.util.spec_from_file_location(name, str(SCRIPTS / (name + ".py")))
    mod = importlib.util.module_from_spec(spec)
    sys.modules[name] = mod  # rtk_install does `import state`
    spec.loader.exec_module(mod)
    return mod


_STATE = _load("state")
_RTK = _load("rtk_install")


class CaseError(Exception):
    """An error whose str() is exactly the case's message."""


class FakeFetch:
    def __init__(self, case, url):
        self.serve = case.get("serve") or "lock"
        self.blob = bytes.fromhex(case.get("blob") or "")
        self.error = case.get("error")
        self.url = url
        self.calls = []

    def __call__(self, url, dest, timeout):
        self.calls.append((url, timeout))
        if self.serve == "error":
            raise CaseError(self.error[1])
        if self.serve == "lock" and url == self.url:
            Path(dest).write_bytes(self.blob)
            return
        raise KeyError(url)


def _build_pre(tmp, data, pre):
    ages = []
    for item in pre or []:
        path = data / item["path"]
        path.parent.mkdir(parents=True, exist_ok=True)
        kind = item["kind"]
        if kind == "file":
            path.write_bytes(bytes.fromhex(item.get("data") or ""))
        elif kind == "dir":
            path.mkdir(parents=True, exist_ok=True)
        elif kind == "symlink":
            os.symlink((item.get("target") or "").replace("<tmp>", str(tmp)), str(path))
            continue
        else:
            raise KeyError(kind)
        if item.get("mode") is not None:
            os.chmod(str(path), item["mode"])
        if item.get("age") is not None:
            ages.append((path, item["age"]))
    for path, days in reversed(ages):
        when = time.time() - days * 86400
        os.utime(str(path), (when, when))


def _listing(data):
    out = []
    for dirpath, dirnames, filenames in os.walk(str(data)):
        for name in dirnames + filenames:
            p = os.path.join(dirpath, name)
            rel = os.path.relpath(p, str(data))
            st = os.lstat(p)
            mode = st.st_mode & 0o7777
            if os.path.islink(p):
                out.append("%s link -> %s" % (rel, os.readlink(p)))
            elif os.path.isdir(p):
                out.append("%s dir %04o" % (rel, mode))
            else:
                with open(p, "rb") as fh:
                    digest = hashlib.sha256(fh.read()).hexdigest()
                out.append("%s file %04o %s" % (rel, mode, digest[:16]))
        # os.walk does not descend into symlinked folders (followlinks=False).
    return sorted(out)


def run(case):
    """What Rust must produce for `case` (module docstring)."""
    tmp = Path(os.path.realpath(tempfile.mkdtemp(prefix="rtk-reconcile-")))
    saved_env = {k: os.environ.get(k) for k in ("CLAUDE_CONFIG_DIR", "HOME")}
    saved_limits = (_RTK.MAX_BINARY_BYTES, _RTK.MAX_TAR_MEMBERS)
    os.environ["CLAUDE_CONFIG_DIR"] = str(tmp / "config")
    os.environ["HOME"] = str(tmp / "home")
    try:
        data = tmp / "data"
        data.mkdir()
        _build_pre(tmp, data, case.get("pre"))
        shown = lambda s: s.replace(str(tmp), "<tmp>")  # noqa: E731
        if case.get("state") is not None:
            _STATE.write(rtk_path=case["state"].replace("<tmp>", str(tmp)))
        try:
            lock = _RTK.validate_lock(case["lock"])
        except _RTK.InstallError as exc:
            return {"exit": 2, "stdout": "", "stderr": "rtk_install: %s\n" % exc}
        system, machine = case["uname"]
        try:
            url = _RTK.release_url(lock, _RTK.target(system, machine))
        except _RTK.InstallError:
            url = ""
        if case.get("max_binary_bytes") is not None:
            _RTK.MAX_BINARY_BYTES = case["max_binary_bytes"]
        if case.get("max_tar_members") is not None:
            _RTK.MAX_TAR_MEMBERS = case["max_tar_members"]
        fetch = FakeFetch(case, url)
        res = _RTK.reconcile(data, lock, fetch, 5, system=system, machine=machine)
        lines = [
            "rtk_path: %s" % (shown(res["rtk_path"]) if res["rtk_path"] is not None else "None"),
            "installed: %s" % res["installed"],
            "warning: %s" % (shown(res["warning"]) if res["warning"] is not None else "None"),
            "calls: %d" % len(fetch.calls),
        ]
        for u, t in fetch.calls:
            lines.append("call: %s %g" % (u, t))
        lines.append("tree:")
        lines.extend(shown(line) for line in _listing(data))
        rtk_path = _STATE.read().get("rtk_path")
        lines.append("state: %s" % (shown(rtk_path) if isinstance(rtk_path, str) else "None"))
        return {"exit": 0, "stdout": "\n".join(lines) + "\n", "stderr": ""}
    finally:
        _RTK.MAX_BINARY_BYTES, _RTK.MAX_TAR_MEMBERS = saved_limits
        for k, v in saved_env.items():
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v
        shutil.rmtree(str(tmp), ignore_errors=True)
