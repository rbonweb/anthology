#!/usr/bin/env python3
"""SessionStart hook: deliver the routing guide, self-test the guardrails, reconcile rtk, sync memory.

`run(env, stdin)` returns the hook's stdout JSON as a string; `main()` prints it and always
exits 0, because a problem here must never fail a session. Output (F7):

    {"hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": <guide + memory>},
     "systemMessage": <warnings and memory messages, one per line>}

`additionalContext` stays within 9,000 characters (U8: the platform cuts a field at 10,000). The
guide is never truncated; memory text is cut from the end at a line boundary and ends with a
pointer to `anthology-mem search` and the full index. Nothing is written except `state.json`
(and what `memory_sync` writes); the guide goes nowhere but the context. Python stdlib only (3.9+).
"""
from __future__ import annotations

import json
import os
import platform
import shutil
import subprocess
import sys
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
except ImportError as _exc:  # a broken install must still not fail the session
    _IMPORT_ERROR = _exc

REFUSAL = "Anthology supports macOS and Linux only; it is inactive on Windows."
NAG = "anthology: run /anthology:setup to finish installing (rtk, permissions, memory)"
MOVED = "anthology: the plugin moved; re-run /anthology:setup so agents can read their knowledge"
GUARD_TIMEOUT = 10        # seconds for the guard self-test
RTK_TIMEOUT = 20          # seconds for the rtk reconcile (download included)
CONTEXT_BUDGET = 9000     # characters of additionalContext (U8 test budget; the platform cap is 10,000)
MESSAGE_CAP = 2000


def _plugin_root(env):
    return Path(env.get("CLAUDE_PLUGIN_ROOT") or os.path.dirname(_SCRIPTS))


def _guard_selftest(env, root):
    """Pipe the known-bad sample into guard.py; a warning string unless it blocks (exit 2)."""
    base = guardrails_loc.locate(root, env)
    if base is None:
        return ("anthology: the guardrails plugin was not found, so the browser and secret "
                "guardrails are NOT active; reinstall anthology-guardrails")
    try:
        sample = guardrails_loc.load_rules(root, env).SELF_TEST_SAMPLE
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
    if proc.returncode != 2:
        return ("anthology: the guardrails self-test did NOT block a known-bad command (exit %d); "
                "the guardrails may be OFF" % proc.returncode)
    return None


def _u1_rule_present(env, root):
    try:
        data = json.loads(settings_patch.read_text(settings_patch.settings_path(env)))
        return settings_patch.plugin_read_rule(root) in data["permissions"]["allow"]
    except (OSError, ValueError, KeyError, TypeError):
        return False


def _reconcile_rtk(root, data_dir):
    """Adapter over Task 11's `rtk_install` (the one place here that names its interface).
    Returns a warning string or None."""
    try:
        import rtk_install
    except ImportError:
        return "anthology: rtk wiring not present yet; skipped"
    if not data_dir:
        return None
    try:
        result = rtk_install.reconcile(Path(data_dir), Path(root) / "rtk.lock", None, RTK_TIMEOUT)
    except Exception as exc:
        return "anthology: rtk setup failed: %s" % (str(exc) or type(exc).__name__)[:150]
    return result if isinstance(result, str) and result else None


def _read_guide(root):
    try:
        return (Path(root) / "templates" / "routing-guide.md").read_text(encoding="utf-8").rstrip("\n") + "\n"
    except OSError:
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


def _run(env, stdin):
    if platform.system() == "Windows":
        return json.dumps({"systemMessage": REFUSAL})
    if _IMPORT_ERROR is not None:
        raise _IMPORT_ERROR
    try:
        hook = json.loads(stdin) if stdin and stdin.strip() else {}
    except ValueError:
        hook = {}
    cwd = hook.get("cwd") if isinstance(hook, dict) and isinstance(hook.get("cwd"), str) else None
    cwd = cwd or os.getcwd()
    root = _plugin_root(env)
    warnings = []

    warn = _guard_selftest(env, root)
    if warn:
        warnings.append(warn)

    data_dir = env.get("CLAUDE_PLUGIN_DATA") or None
    current = state.read()
    if data_dir and current.get("data_dir") != data_dir:
        try:
            state.write(data_dir=data_dir)
        except OSError as exc:
            warnings.append("anthology: could not write state.json (%s)" % exc.strerror)
    if "setup_done" not in current:
        warnings.append(NAG)
    else:
        if not _u1_rule_present(env, root):
            warnings.append(MOVED)
        warn = _reconcile_rtk(root, data_dir or current.get("data_dir"))
        if warn:
            warnings.append(warn)

    memory, messages = "", []
    try:
        memory, messages = memory_sync.run(cwd, env)
    except Exception as exc:
        messages = ["anthology memory: %s" % (str(exc) or type(exc).__name__)[:120]]

    guide = _read_guide(root)
    if guide is None:
        warnings.append("anthology: the routing guide is missing from the plugin (templates/routing-guide.md)")
    context = _compose(guide or "", memory, memory_sync.anthology_home(env))

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
        return json.dumps({"systemMessage": "anthology: session start failed (%s); the routing guide "
                                            "and the guardrails self-test did not run" % type(exc).__name__})


def main():
    try:
        raw = "" if sys.stdin is None or sys.stdin.isatty() else sys.stdin.read()
    except Exception:
        raw = ""
    try:
        out = run(dict(os.environ), raw)
    except Exception as exc:
        out = json.dumps({"systemMessage": "anthology: session start failed (%s)" % type(exc).__name__})
    try:
        sys.stdout.write(out + "\n")
        sys.stdout.flush()
    except Exception:
        pass
    return 0


if __name__ == "__main__":
    sys.exit(main())
