#!/usr/bin/env python3
"""SessionStart hook: deliver the routing guide, self-test the guardrails, reconcile rtk, sync memory.

`run(env, stdin)` returns the hook's stdout JSON as a string; `main()` prints it and always
exits 0, because a problem here must never fail a session. Output (F7):

    {"hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": <guide + memory>},
     "systemMessage": <warnings and memory messages, one per line>}

`additionalContext` stays within 9,000 characters (U8: the platform cuts a field at 10,000). The
guide is never truncated; memory text is cut from the end at a line boundary and ends with a
pointer to `anthology-mem search` and the full index. Nothing is written except `state.json`
(and what `memory_sync` writes); the guide goes nowhere but the context. Every step is isolated
(F22): one failing step becomes a warning and never costs the guide. Python stdlib only (3.9+).
"""
from __future__ import annotations

import json
import os
import platform
import shutil
import subprocess
import sys
import time
from pathlib import Path

_SCRIPTS = os.path.dirname(os.path.abspath(__file__))
if _SCRIPTS not in sys.path:
    sys.path.insert(0, _SCRIPTS)

try:
    import guardrails_loc
    import memory_sync
    import settings_patch
    import state
    _IMPORT_ERROR = None
except Exception as _exc:  # a broken or half-updated install must still not fail the session
    _IMPORT_ERROR = _exc

REFUSAL = "Anthology supports macOS and Linux only; it is inactive on Windows."
NAG = "anthology: run /anthology:setup to finish installing (rtk, permissions, memory)"
MOVED = "anthology: the plugin moved; re-run /anthology:setup so agents can read their knowledge"
GUARD_TIMEOUT = 10        # seconds for the guard self-test
# rtk_install's download deadline is 4 x this timeout, overshooting by at most one socket
# timeout: worst case 5 x 8 = 40 s.
RTK_TIMEOUT = 8
DEADLINE = 50             # seconds of work before the 60 s hook timeout (M5)
RTK_MIN_REMAINING = 40    # skip the rtk reconcile when less than this is left (its worst case)
CONTEXT_BUDGET = 9000     # characters of additionalContext (U8 test budget; the platform cap is 10,000)
MESSAGE_CAP = 2000


def _plugin_root(env):
    return Path(env.get("CLAUDE_PLUGIN_ROOT") or os.path.dirname(_SCRIPTS))


def _elapsed(start):
    return time.monotonic() - start


def _guard_selftest(env, root, cwd=None):
    """Pipe the known-bad sample into guard.py; a warning string unless it blocks (exit 2 with the
    guard's own message)."""
    base, problem = guardrails_loc.inspect(root, env, cwd)
    if problem == "disabled":
        return ("anthology: anthology-guardrails is disabled in your settings, so the guardrails are "
                "OFF; enable it (/plugin)")
    if problem == "not_installed":
        return ("anthology: anthology-guardrails is not installed, so the browser and secret "
                "guardrails are NOT active; install it (/plugin)")
    if base is None:
        return ("anthology: the guardrails plugin was not found, so the browser and secret "
                "guardrails are NOT active; reinstall anthology-guardrails")
    try:
        sample = guardrails_loc.load_rules(root, env, cwd).SELF_TEST_SAMPLE
    except Exception as exc:
        return ("anthology: the guardrails self-test could not load its sample (%s); the "
                "guardrails may be OFF" % (str(exc) or type(exc).__name__)[:100])
    python = shutil.which("python3", path=env.get("PATH"))
    if not python:
        return ("anthology: python3 not found on PATH, so the guardrails hook cannot run and the "
                "guardrails are OFF; install Python 3 (3.9 or newer)")
    try:
        proc = subprocess.run([python, str(base / "scripts" / "guard.py")],
                              input=json.dumps(sample).encode("utf-8"), stdout=subprocess.PIPE,
                              stderr=subprocess.PIPE, timeout=GUARD_TIMEOUT, env=env)
    except subprocess.TimeoutExpired:
        return "anthology: the guardrails self-test timed out; the guardrails may be OFF"
    except OSError as exc:
        return "anthology: the guardrails self-test could not run (%s); the guardrails may be OFF" % exc.strerror
    if proc.returncode != 2 or not proc.stderr.startswith(b"Blocked by anthology-guardrails"):
        return ("anthology: the guardrails self-test did NOT block a known-bad command (exit %d); "
                "the guardrails may be OFF" % proc.returncode)
    return None


def _setup_warning(env, root):
    """The U1 plugin-read rule check (shared with doctor through settings_patch)."""
    status = settings_patch.plugin_rule_status(root, env)
    if status == "missing":
        return MOVED
    if status == "unreadable":
        return ("anthology: could not read settings.json to check the plugin-read permission; "
                "fix the file, then re-run /anthology:setup if agents cannot read their knowledge")
    return None


def _reconcile_rtk(root, data_dir):
    """Adapter over Task 11's `rtk_install` (the one place here that names its interface).
    Returns a warning string or None."""
    try:
        import rtk_install
    except ImportError:
        return "anthology: rtk wiring not present yet; skipped"
    except Exception as exc:  # e.g. a SyntaxError in a half-updated install
        return "anthology: rtk wiring failed to load (%s); skipped" % type(exc).__name__
    if not data_dir:
        return None
    lock_path = Path(root) / "rtk.lock"
    try:
        lock = rtk_install.load_lock(lock_path)
    except Exception as exc:
        if not lock_path.exists():
            return "anthology: rtk wiring not present yet; skipped"
        return "anthology: rtk.lock is not valid (%s); rtk skipped" % (str(exc) or type(exc).__name__)[:150]
    try:
        result = rtk_install.reconcile(Path(data_dir), lock, None, RTK_TIMEOUT)
    except Exception as exc:
        return "anthology: rtk setup failed: %s" % (str(exc) or type(exc).__name__)[:150]
    warning = result.get("warning") if isinstance(result, dict) else None
    return warning if isinstance(warning, str) and warning else None


def _read_guide(root):
    try:
        return (Path(root) / "templates" / "routing-guide.md").read_text(encoding="utf-8").rstrip("\n") + "\n"
    except (OSError, ValueError):
        return None


def _compose(guide, memory, home):
    """Guide, then memory text cut to fit the budget (the guide itself is never cut)."""
    memory = memory.rstrip("\n")
    if not memory:
        return guide.rstrip("\n")
    head = guide + "\n"
    room = CONTEXT_BUDGET - len(head)
    if len(memory) <= room:
        return head + memory
    pointer = ("(memory index truncated to fit the session budget; use `anthology-mem search <query>`; "
               "full indexes: %s, and .claude/memory/MEMORY.md in the repo)"
               % (Path(home) / "personal-memory" / "MEMORY.md"))
    allowed = room - len(pointer) - 1
    if allowed <= 0:
        return head.rstrip("\n")
    cut = memory[:allowed + 1]
    cut = cut[:cut.rfind("\n")] if "\n" in cut else ""
    return head + (cut + "\n" if cut else "") + pointer


def _step(warnings, label, fn, default=None):
    """Run one step; a failure becomes a warning naming it, never an exception."""
    try:
        return fn()
    except Exception as exc:  # includes RecursionError and UnicodeDecodeError
        warnings.append("anthology: %s failed (%s)" % (label, type(exc).__name__))
        return default


def _parse_stdin(stdin):
    try:
        hook = json.loads(stdin) if stdin and stdin.strip() else {}
    except Exception:
        return {}
    return hook if isinstance(hook, dict) else {}


def _run(env, stdin):
    if platform.system() == "Windows":
        return json.dumps({"systemMessage": REFUSAL})
    if _IMPORT_ERROR is not None:
        raise _IMPORT_ERROR
    started = time.monotonic()
    warnings = []
    hook = _step(warnings, "reading the hook input", lambda: _parse_stdin(stdin), {})
    cwd = hook.get("cwd") if isinstance(hook.get("cwd"), str) else None
    cwd = cwd or os.getcwd()
    root = _plugin_root(env)

    def add(warn):
        if warn:
            warnings.append(warn)

    add(_step(warnings, "the guardrails self-test", lambda: _guard_selftest(env, root, cwd)))

    data_dir = env.get("CLAUDE_PLUGIN_DATA") or None
    current = _step(warnings, "reading state.json", state.read, {})

    def record_data_dir():
        if data_dir and current.get("data_dir") != data_dir:
            try:
                state.write(data_dir=data_dir)
            except OSError as exc:
                warnings.append("anthology: could not write state.json (%s)" % exc.strerror)

    _step(warnings, "recording the plugin data folder", record_data_dir)
    if "setup_done" not in current:
        warnings.append(NAG)
    else:
        add(_step(warnings, "the plugin-read permission check", lambda: _setup_warning(env, root)))
        if _elapsed(started) > DEADLINE - RTK_MIN_REMAINING:
            warnings.append("anthology: skipped the rtk check this session (not enough time left)")
        else:
            add(_step(warnings, "the rtk check",
                      lambda: _reconcile_rtk(root, data_dir or current.get("data_dir"))))

    def memory_step():
        try:
            return memory_sync.run(cwd, env)
        except Exception as exc:
            return "", ["anthology memory: %s" % (str(exc) or type(exc).__name__)[:120]]

    memory, messages = _step(warnings, "memory sync", memory_step, ("", []))

    guide = _step(warnings, "reading the routing guide", lambda: _read_guide(root))
    if guide is None:
        warnings.append("anthology: the routing guide is missing from the plugin (templates/routing-guide.md)")
    context = _step(warnings, "assembling the session context",
                    lambda: _compose(guide or "", memory, memory_sync.anthology_home(env)), guide or "")

    out = {"hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": context}}
    message = "\n".join(warnings + list(messages))
    if message:
        out["systemMessage"] = message if len(message) <= MESSAGE_CAP else message[:MESSAGE_CAP - 1].rstrip() + "…"
    return json.dumps(out)


def run(env=None, stdin=""):
    """The SessionStart hook output (a JSON string). Never raises."""
    env = dict(os.environ) if env is None else dict(env)
    try:
        return _run(env, stdin)
    except Exception as exc:
        return json.dumps({"systemMessage": "anthology: session start did not complete (%s); the routing "
                                            "guide may be missing" % type(exc).__name__})


def main():
    try:
        raw = "" if sys.stdin is None or sys.stdin.isatty() else sys.stdin.read()
    except Exception:
        raw = ""
    try:
        out = run(dict(os.environ), raw)
    except Exception as exc:
        out = json.dumps({"systemMessage": "anthology: session start did not complete (%s)" % type(exc).__name__})
    try:
        sys.stdout.write(out + "\n")
        sys.stdout.flush()
    except Exception:
        pass
    return 0


if __name__ == "__main__":
    sys.exit(main())
