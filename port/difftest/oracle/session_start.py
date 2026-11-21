"""RT3 oracle for the `hook.session-start` difftest surface (plugins/anthology/scripts/session_start.py).

F4's driver loads this module by file path; it runs as `python3 -I`. Library surface:
`run(case: dict) -> dict`.

Case (every string may hold `@ROOT@`, the case's temporary root at its real path):
- `tree`: layout entries (`@ENC@` in a path is the root's realpath with every character that is
  not an ASCII letter or digit written `-`, as the old path-keyed memory folders are named; the
  answer writes it `@ENC@` again), built by `guardrails_loc.materialize` (the RT1a twin of
  `anthology_testkit::rt_a::materialize`), plus `{"t": "sguard", "p": P, "b": B, "reason": R}`:
  a guardrails plugin folder whose guard behaves as `B` (see `GUARDS`), written as BOTH
  `scripts/guard.py` (+ the real `scripts/guard_rules.py`) and `hooks/guard.sh` (F-6), so
  both sides run the same guard. File texts may hold `@RULE@` (settings_patch.plugin_read_rule of
  the plugin root) and `@OPSV@` (setup's ops_version, or `1-ffffffffffffffff` when setup_ops
  refuses the root); entries holding them are built after the others;
- `env`: the hook's environment (`PATH` is added here: `@ROOT@/gitbin`, where the tree holds the
  difftest's fake `git` (`crates/difftest/src/surfaces/fake_git.rs`; this image has no git, and
  Rust runs the same script), then python3's folder, so `python3` runs the guard);
- `stdin`, `cwd` (what `os.getcwd()` gives), `elapsed` (`_elapsed`), `windows` (bool);
- memory: `memory_sync.run` is real, over the tree's notes and approvals (no patch);
- `rtk`: `null` (no `rtk_install` module) or `{"warning", "lock_exc", "exc"}` (a fake rtk_install:
  `load_lock` raises `RuntimeError(lock_exc)` when set, `reconcile` raises `RuntimeError(exc)` or
  returns the warning).

The forced changes are applied to Python's answer, nothing else:
- F-4 and m-R14: the self-test passes only on exit 2 with stderr starting
  `Blocked by anthology-guardrails (G1)`; exit 2 with `Blocked by anthology-guardrails (setup): `
  reports `anthology: the guardrails are failing closed: <reason>` (first line, cut to 300 with …);
- F-6: the live-missing line names `hooks/guard.sh`;
- F-9: `bin/anthology` is a wrapper too, checked first;
- D-1: `memory_sync._load_settings` raising `RecursionError` (json.loads on a settings file nested
  deeper than CPython's C stack, e.g. an unterminated `[[[[...`) aborts Python's whole memory
  step; Rust's parser has no such limit, finds the document invalid and reads it as unusable,
  as Python does for any other invalid JSON. The oracle makes the call return None (unusable)
  and marks the case D-1.

The answer is `{"exit": 0, "stdout": <hook JSON> + "\\n" + <state.json text or "<none>">, "stderr": ""}`
with the root written as `@ROOT@` and its encoded form `@ENC@`.

Python 3.9+ and the standard library only.
"""
from __future__ import annotations

import importlib.util
import os
import re
import shutil
import subprocess
import sys
import tempfile
import types
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
SCRIPTS = ROOT / "plugins" / "anthology" / "scripts"
if str(SCRIPTS) not in sys.path:
    sys.path.append(str(SCRIPTS))

import compat  # noqa: E402
import session_start  # noqa: E402
import settings_patch  # noqa: E402

_spec = importlib.util.spec_from_file_location("difftest_oracle_rt3_loc", str(HERE / "guardrails_loc.py"))
_loc = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_loc)

PASS = b"Blocked by anthology-guardrails (G1)"
SETUP = b"Blocked by anthology-guardrails (setup): "
# The real guard_rules.py: its SELF_TEST_SAMPLE, PROFILE_ROOTS and _PREFIX_VARIANTS equal the
# compiled-in anthology_guard::data (F-7), so setup_ops gives the same op set on both sides.
RULES = (ROOT / "plugins" / "anthology-guardrails" / "scripts" / "guard_rules.py").read_text(encoding="utf-8")

# behaviour: (exit code, stderr line or None)
GUARDS = {
    "g1": (2, "Blocked by anthology-guardrails (G1): run it inside Docker (OrbStack); for browser work use BrowserOS Neo"),
    "g2": (2, "Blocked by anthology-guardrails (G2): never launch Chrome on the host; use BrowserOS Neo"),
    "plain": (2, "Blocked by anthology-guardrails: x"),
    "internal": (2, "Blocked by anthology-guardrails (internal): the guard failed on this input"),
    "setup": (2, None),
    "exit0": (0, None),
    "exit1": (1, None),
    "exit2": (2, None),
    "g1exit0": (0, "Blocked by anthology-guardrails (G1): x"),
    "g1exit3": (3, "Blocked by anthology-guardrails (G1): x"),
    "other": (2, "something else went wrong"),
}

_ORIG_SELFTEST = session_start._guard_selftest
_ORIG_LOAD_SETTINGS = session_start.memory_sync._load_settings
_ORIG_WRAPPERS = compat.wrapper_problems


def guard_texts(b, reason):
    """`(guard.py, guard.sh)` for behaviour `b`."""
    code, line = GUARDS[b]
    if b == "setup":
        line = "Blocked by anthology-guardrails (setup): " + reason
    py = "import sys\nsys.stdin.buffer.read()\n"
    sh = "#!/bin/sh\ncat >/dev/null\n"
    if line is not None:
        py += "sys.stderr.buffer.write(%r)\nsys.stderr.flush()\n" % (line + "\n").encode("utf-8")
        sh += "printf '%%s\\n' '%s' >&2\n" % line
    py += "sys.exit(%d)\n" % code
    sh += "exit %d\n" % code
    return py, sh


def _write(path, text):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(text.encode("utf-8"))


def _has_late(entry):
    text = entry.get("text")
    return isinstance(text, str) and ("@RULE@" in text or "@OPSV@" in text)


def build(root, case, env, cwd):
    """Builds the tree: the plain entries, the guards, then the entries with @RULE@/@OPSV@."""
    early, late, guards = [], [], []
    for entry in case.get("tree") or []:
        if entry.get("t") == "sguard":
            guards.append(entry)
        elif _has_late(entry):
            late.append(entry)
        else:
            early.append(entry)
    _loc.materialize(root, early)
    for g in guards:
        base = Path(root) / g["p"]
        py, sh = guard_texts(g["b"], g.get("reason") or "")
        _write(base / "scripts" / "guard.py", py)
        _write(base / "scripts" / "guard_rules.py", RULES)
        _write(base / "hooks" / "guard.sh", sh)
        os.chmod(str(base / "hooks" / "guard.sh"), 0o755)
    plugin = env.get("CLAUDE_PLUGIN_ROOT") or ""
    rule = settings_patch.plugin_read_rule(plugin) if plugin else "Read(//none/**)"
    try:
        with mock.patch.object(os, "getcwd", return_value=cwd):
            opsv = settings_patch.ops_version(settings_patch.setup_ops(plugin, env=env))
    except Exception:
        opsv = "1-ffffffffffffffff"
    for entry in late:
        e = dict(entry)
        e["text"] = e["text"].replace("@RULE@", rule).replace("@OPSV@", opsv)
        _loc.materialize(root, [e])


class FakeRtk:
    LOCK = {"version": "9.9.9", "sha256": {}}

    def __init__(self, spec):
        self.spec = spec

    def load_lock(self, path):
        if self.spec.get("lock_exc") is not None:
            raise RuntimeError(self.spec["lock_exc"])
        return self.LOCK

    def reconcile(self, data_dir, lock, fetch=None, timeout=30, system=None, machine=None):
        if self.spec.get("exc") is not None:
            raise RuntimeError(self.spec["exc"])
        return {"rtk_path": None, "warning": self.spec.get("warning"), "installed": False}


def _cut(line, n):
    return line[:n - 1].rstrip() + "…" if len(line) > n else line


def run(case):
    tmp = tempfile.mkdtemp(prefix="dt-rt3-", dir="/tmp")
    root = os.path.realpath(tmp)
    try:
        def x(s):
            return s.replace("@ROOT@", root)

        env = {k: x(v) for k, v in (case.get("env") or {}).items()}
        env["PATH"] = root + "/gitbin:" + os.path.dirname(sys.executable) + ":/usr/bin:/bin"
        cwd = x(case.get("cwd") or "@ROOT@")
        enc = re.sub(r"[^A-Za-z0-9]", "-", root)
        case = dict(case, tree=[dict(e, p=e["p"].replace("@ENC@", enc))
                                for e in (case.get("tree") or [])])
        build(root, case, env, cwd)
        captured = {}

        def capture_run(*a, **kw):
            proc = subprocess.run(*a, **kw)
            captured["proc"] = proc
            return proc

        fake_subprocess = types.SimpleNamespace(
            run=capture_run, PIPE=subprocess.PIPE, TimeoutExpired=subprocess.TimeoutExpired)

        def selftest(env_, root_, cwd_=None):
            captured.clear()
            w = _ORIG_SELFTEST(env_, root_, cwd_)
            if w and "has no guard.py" in w:  # F-6
                w = w.replace("has no guard.py", "has no hooks/guard.sh")
            proc = captured.get("proc")
            if proc is not None and (w is None or "did NOT block" in w):  # F-4, m-R14
                err = proc.stderr
                if proc.returncode == 2 and err.startswith(PASS):
                    w = None
                elif proc.returncode == 2 and err.startswith(SETUP):
                    line = err[len(SETUP):].decode("utf-8", "replace").split("\n")[0].rstrip("\r")
                    w = "anthology: the guardrails are failing closed: " + _cut(line, 300)
                else:
                    w = ("anthology: the guardrails self-test did NOT block a known-bad command "
                         "(exit %d); the guardrails may be OFF" % proc.returncode)
            return w

        def wrappers(plugin_root):  # F-9
            p = Path(plugin_root) / "bin" / "anthology"
            first = [] if (p.is_file() and os.access(str(p), os.X_OK)) else ["anthology"]
            return first + _ORIG_WRAPPERS(plugin_root)

        deep = []

        def load_settings(path):  # D-1
            try:
                return _ORIG_LOAD_SETTINGS(path)
            except RecursionError:
                deep.append(path)
                return None

        rtk = case.get("rtk")
        modules = {"rtk_install": FakeRtk(rtk) if rtk is not None else None}
        elapsed = float(case.get("elapsed") or 0)
        system = "Windows" if case.get("windows") else "Linux"
        with mock.patch.dict(os.environ, env, clear=True), \
                mock.patch.dict(sys.modules, modules), \
                mock.patch.object(session_start, "subprocess", fake_subprocess), \
                mock.patch.object(session_start, "_guard_selftest", selftest), \
                mock.patch.object(compat, "wrapper_problems", wrappers), \
                mock.patch.object(session_start.memory_sync, "_load_settings", load_settings), \
                mock.patch.object(session_start, "_elapsed", lambda start: elapsed), \
                mock.patch("platform.system", return_value=system), \
                mock.patch.object(os, "getcwd", return_value=cwd):
            out = session_start.run(env, x(case.get("stdin") or ""))
            config = env.get("CLAUDE_CONFIG_DIR") or os.path.join(env.get("HOME", ""), ".claude")
        state_path = Path(config) / "anthology" / "state.json"
        try:
            state_text = state_path.read_bytes().decode("utf-8", "replace")
        except OSError:
            state_text = "<none>"
        stdout = (out + "\n" + state_text).replace(root, "@ROOT@").replace(enc, "@ENC@")
        answer = {"exit": 0, "stdout": stdout, "stderr": ""}
        if deep:
            answer["divergence"] = "D-1"
        return answer
    finally:
        _loc._unlock(root)
        shutil.rmtree(tmp, ignore_errors=True)
