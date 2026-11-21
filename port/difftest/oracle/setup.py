"""RT4 oracle for the `setup` difftest surface (plugins/anthology/scripts/anthology_setup.py).

F4's driver loads this module by file path; it runs as `python3 -I`. Library surface:
`run(case: dict) -> dict`.

Case (every string may hold `@ROOT@`, the case's temporary root at its real path):
- `tree`: layout entries (`@ENC@` in a path is the root's realpath with every character that is
  not an ASCII letter or digit written `-`, as the old path-keyed memory folders are named; the
  report writes it `@ENC@` again), built by `guardrails_loc.materialize` (the RT1a twin of
  `anthology_testkit::rt_a::materialize`). A `guard` entry of kind `file` also gets the real
  `scripts/guard_rules.py` here (Python's `setup_ops` loads its PROFILE_ROOTS from it; Rust's
  are compiled in, F-7);
- `env`: the process environment (`PATH=@ROOT@/gitbin:/usr/bin:/bin` is added: the tree holds
  the difftest's fake `git` there, `crates/difftest/src/surfaces/fake_git.rs`, because this image
  has no git; Rust runs the same script);
- `uname`: `linux` (Linux/x86_64), `darwin` (Darwin/arm64), `riscv` (Linux/riscv64) or
  `windows` (Windows/AMD64): `platform.system()` and `platform.machine()`;
- `doctor`: `{"checks": V, "installed": V, "state": S, "duplicates": V}`: what
  `doctor.check_env`, `doctor.installed_plugins_state` and `doctor.find_duplicates` return
  (RT1b's surface tests the real doctor; the Rust default is a SWAP(RT1b) stand-in);
- memory: all of `memory_sync` is real on both sides (`repo_root`, `repo_key` and `approve` run
  the fake git; a note under `<config>/projects/@ENC@-w/memory/` is a legacy note of the repo at
  `@ROOT@/w`); only the privacy scan's host name is patched (`alice-laptop`);
- `rtk`: `null` (no `rtk_install` module) or `{"lock_exc", "exc", "warning", "installed",
  "touch"}`: a fake module whose `load_lock` raises `RuntimeError(lock_exc)` when set, whose
  `target` is the real `rtk_install.target`, and whose `reconcile` raises `RuntimeError(exc)` or
  returns the warning and `installed`, after appending a space to `<root>/<touch>` when set (an
  edit made during the download);
- `guard_binary`: the status Rust's `ensure_guard_binary` seam returns (`present`,
  `installed`, `failed: <reason>`);
- `steps`: argv lists for `main(argv)`, run in order in the same tree. `@SHA@` in an argument is
  the sha256 setup computes for the settings.json text at that moment (`0`*64 when it is not
  UTF-8). A step `["@WRITE@", P, T]` writes `T` to `<root>/P` instead (an edit between commands);
- `show`: paths (relative to the root) whose final state is printed.

The forced changes are applied to Python's answer, nothing else:
- F-6: `_settings_error` names `hooks/guard.sh`;
- F-15: `guard_binary` is added as the last key: preview `{"root": <located guardrails folder or
  null>}`; apply (ok, or refused as changed during setup) `{"status": <the case's status, or
  "skipped: guardrails not found">}`; remove `{"status"}` and remove --preview `{"dir",
  "status"}` for `<ah>/bin`: `nothing to delete` when absent, `kept (anthology-guardrails still
  uses it)` when `installed_plugins.json` has an `anthology-guardrails@*` entry, has no `plugins` object or
  does not parse, else `delete` (preview) / `deleted` (the folder, or a symlink to it, is
  removed after the rest of remove; Rust deletes it before unapproving, which no case can
  tell apart: neither step fails once the other has run);
- the argparse prog is `anthology setup` (`sys.argv[0]`); argparse's `(choose from 'a', 'b')`
  is written unquoted, as CPython 3.12.8 and later (and the Rust port) print it; a case where
  argparse exited is marked D-4.

The answer is `{"exit": <last step's exit>, "stdout": <report>, "stderr": ""}`: per step
`--- step N: exit E ---`, its stdout, `--- stderr ---`, its stderr; then per `show` path
`=== P ===` and `<absent>`, `<link>`, `<dir MODE>` or the file's text. The root is written
`@ROOT@` and each step's settings sha256 `@SHA@`.

rtk's history folder is real on both sides: `state.rtk_data_dir()` is under the case's
`CLAUDE_CONFIG_DIR` (or `HOME`), so the tree's `<config>/anthology/rtk` entries (a folder, a
symlink, a file) are what remove previews, deletes or leaves in place.

Python 3.9+ and the standard library only.
"""
from __future__ import annotations

import contextlib
import hashlib
import importlib.util
import io
import os
import re
import shutil
import stat
import sys
import tempfile
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
SCRIPTS = ROOT / "plugins" / "anthology" / "scripts"
if str(SCRIPTS) not in sys.path:
    sys.path.append(str(SCRIPTS))

import anthology_setup  # noqa: E402
import compat  # noqa: E402
import guardrails_loc  # noqa: E402
import memory_sync  # noqa: E402
import rtk_install as _real_rtk  # noqa: E402
import settings_patch  # noqa: E402

_spec = importlib.util.spec_from_file_location("difftest_oracle_rt4_loc", str(HERE / "guardrails_loc.py"))
_loc = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_loc)

RULES = (ROOT / "plugins" / "anthology-guardrails" / "scripts" / "guard_rules.py").read_text(encoding="utf-8")
UNAME = {"linux": ("Linux", "x86_64"), "darwin": ("Darwin", "arm64"),
         "riscv": ("Linux", "riscv64"), "windows": ("Windows", "AMD64")}
DURING = "settings.json changed during setup; it was left untouched. Run the preview again."
BIN_KEPT = "kept (anthology-guardrails still uses it)"
NO_SHA = "0" * 64
_CHOICES = re.compile(r"\(choose from '[^']*'(?:, '[^']*')*\)")

_ORIG_PREVIEW = anthology_setup.preview
_ORIG_APPLY = anthology_setup.apply
_ORIG_REMOVE = anthology_setup.remove
_ORIG_SETTINGS_ERROR = anthology_setup._settings_error


class FakeRtk:
    LOCK = {"version": "9.9.9", "tag": "rtk--v9.9.9", "repo": "example/x", "sha256": {}}

    def __init__(self, spec, root):
        self.spec = spec
        self.root = root

    def load_lock(self, path):
        if self.spec.get("lock_exc") is not None:
            raise RuntimeError(self.spec["lock_exc"])
        return self.LOCK

    def target(self, system, machine):
        return _real_rtk.target(system, machine)

    def reconcile(self, data_dir, lock, fetch=None, timeout=30, system=None, machine=None):
        assert timeout == 60, timeout
        if self.spec.get("touch"):
            path = os.path.join(self.root, self.spec["touch"])
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "ab") as fh:
                fh.write(b" ")
        if self.spec.get("exc") is not None:
            raise RuntimeError(self.spec["exc"])
        return {"rtk_path": None, "warning": self.spec.get("warning"),
                "installed": bool(self.spec.get("installed"))}


def _sha(env):
    try:
        text = settings_patch.read_text(settings_patch.settings_path(env))
    except ValueError:
        return NO_SHA
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def _guardrails_installed(env):
    path = Path(str(memory_sync.config_dir(env))) / "plugins" / "installed_plugins.json"
    if not os.path.lexists(str(path)):
        return False
    data = compat.read_json(str(path))
    if data is None:
        return True
    plugins = data.get("plugins") if isinstance(data, dict) else None
    if not isinstance(plugins, dict):
        return True  # an unexpected shape keeps the binary
    return any(str(k).startswith("anthology-guardrails@") for k in plugins)


def _bin_target(env):
    bin_dir = memory_sync.anthology_home(env) / "bin"
    if not os.path.lexists(str(bin_dir)):
        return None, "nothing to delete"
    if _guardrails_installed(env):
        return None, BIN_KEPT
    return bin_dir, "delete"


def _show(root, rel):
    path = os.path.join(root, rel)
    if os.path.islink(path):
        return "<link>"
    if os.path.isdir(path):
        return "<dir %o>" % stat.S_IMODE(os.stat(path).st_mode)
    try:
        with open(path, "rb") as fh:
            return fh.read().decode("utf-8", "replace")
    except OSError:
        return "<absent>"


def run(case):
    tmp = tempfile.mkdtemp(prefix="dt-rt4-", dir="/tmp")
    root = os.path.realpath(tmp)
    try:
        def x(s):
            return s.replace("@ROOT@", root)

        env = {k: x(v) for k, v in (case.get("env") or {}).items()}
        env["PATH"] = root + "/gitbin:/usr/bin:/bin"
        enc = re.sub(r"[^A-Za-z0-9]", "-", root)
        tree = [dict(e, p=e["p"].replace("@ENC@", enc)) for e in (case.get("tree") or [])]
        _loc.materialize(root, tree)
        for e in tree:
            if e.get("t") == "guard" and e.get("kind", "file") == "file":
                with open(os.path.join(root, e["p"], "scripts", "guard_rules.py"), "w", encoding="utf-8") as fh:
                    fh.write(RULES)

        doc = case.get("doctor") or {}
        guard_status = case.get("guard_binary") or "present"

        def settings_error(exc):  # F-6
            return _ORIG_SETTINGS_ERROR(exc).replace("(scripts/guard.py)", "(hooks/guard.sh)")

        def preview(plugin_root, cwd=None, env=None):  # F-15
            result = _ORIG_PREVIEW(plugin_root, cwd=cwd, env=env)
            if result.get("supported"):
                found = guardrails_loc.locate(plugin_root, dict(os.environ))
                result["guard_binary"] = {"root": str(found) if found is not None else None}
            return result

        def apply(plugin_root, expect_sha256, **kw):  # F-15
            result = _ORIG_APPLY(plugin_root, expect_sha256, **kw)
            if result.get("ok") or result.get("error") == DURING:
                found = guardrails_loc.locate(plugin_root, dict(os.environ))
                status = guard_status if found is not None else "skipped: guardrails not found"
                result["guard_binary"] = {"status": status}
            return result

        def remove(plugin_root, **kw):  # F-15
            bin_dir, status = _bin_target(dict(os.environ))
            result = _ORIG_REMOVE(plugin_root, **kw)
            if not result.get("ok"):
                return result
            if kw.get("preview"):
                result["guard_binary"] = {"dir": str(bin_dir) if bin_dir else None, "status": status}
                return result
            if bin_dir is not None:
                if os.path.isdir(str(bin_dir)) and not os.path.islink(str(bin_dir)):
                    shutil.rmtree(str(bin_dir))
                else:
                    os.unlink(str(bin_dir))
                status = "deleted"
            result["guard_binary"] = {"status": status}
            return result

        rtk = case.get("rtk")
        modules = {"rtk_install": FakeRtk(rtk, root) if rtk is not None else None}
        system, machine = UNAME[case.get("uname") or "linux"]
        report, code, marked = [], 0, False
        patches = [
            mock.patch.dict(os.environ, env, clear=True),
            mock.patch.dict(sys.modules, modules),
            mock.patch.object(sys, "argv", ["anthology setup"]),
            mock.patch("platform.system", return_value=system),
            mock.patch("platform.machine", return_value=machine),
            mock.patch.object(anthology_setup.doctor, "check_env", lambda *a, **kw: doc.get("checks", [])),
            mock.patch.object(anthology_setup.doctor, "installed_plugins_state",
                              lambda *a, **kw: (doc.get("installed"), doc.get("state", "missing"))),
            mock.patch.object(anthology_setup.doctor, "find_duplicates",
                              lambda *a, **kw: doc.get("duplicates", [])),
            mock.patch.object(memory_sync.memory_privacy.socket, "gethostname",
                              return_value="alice-laptop"),
            mock.patch.object(anthology_setup, "_settings_error", settings_error),
            mock.patch.object(anthology_setup, "preview", preview),
            mock.patch.object(anthology_setup, "apply", apply),
            mock.patch.object(anthology_setup, "remove", remove),
        ]
        with contextlib.ExitStack() as stack:
            for p in patches:
                stack.enter_context(p)
            i = 0
            for step in case.get("steps") or []:
                if step and step[0] == "@WRITE@":
                    target = os.path.join(root, step[1])
                    os.makedirs(os.path.dirname(target), exist_ok=True)
                    with open(target, "wb") as fh:
                        fh.write(x(step[2]).encode("utf-8"))
                    continue
                i += 1
                sha = _sha(dict(os.environ))
                argv = [x(a).replace("@SHA@", sha) for a in step]
                out, err = io.StringIO(), io.StringIO()
                with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                    try:
                        code = anthology_setup.main(argv)
                    except SystemExit as exc:  # argparse
                        marked = True
                        code = exc.code if isinstance(exc.code, int) else (0 if exc.code is None else 1)
                stderr = err.getvalue()
                if marked:  # D-4: CPython 3.12.8+ lists the choices unquoted
                    stderr = _CHOICES.sub(lambda m: m.group(0).replace("'", ""), stderr)
                text = "--- step %d: exit %d ---\n%s--- stderr ---\n%s" % (i, code, out.getvalue(), stderr)
                report.append(text.replace(sha, "@SHA@"))
        for rel in case.get("show") or []:
            report.append("=== %s ===\n%s\n" % (rel, _show(root, rel)))
        answer = {"exit": code, "stdout": "".join(report).replace(root, "@ROOT@").replace(enc, "@ENC@"), "stderr": ""}
        if marked:
            answer["divergence"] = "D-4"
        return answer
    finally:
        _loc._unlock(root)
        shutil.rmtree(tmp, ignore_errors=True)
