#!/usr/bin/env python3
"""`/anthology:setup` orchestration: preview, apply and remove.

    anthology_setup.py preview --plugin-root R [--cwd C] [--json]
    anthology_setup.py apply   --plugin-root R --expect-sha256 H [--cwd C] [--skip-approve]
    anthology_setup.py remove  --plugin-root R [--cwd C] [--unapprove] [--unapprove-all]

`preview` writes nothing: it reports the environment checks, duplicate plugins, the rtk step,
the settings.json diff (with the sha256 of the text it was computed from) and the memory
steps. `apply` refuses if settings.json changed since the preview. `remove` undoes the
settings edits, deletes the downloaded rtk and `state.json`, and optionally drops repo
approvals; memory notes, `.gitattributes` and the personal-memory folder are never deleted.
Every command prints one JSON object. Exit codes: 0 ok, 1 refused or failed, 2 unsupported
platform. Python stdlib only (3.9+).
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import shutil
import sys
from pathlib import Path

_SCRIPTS = os.path.dirname(os.path.abspath(__file__))
if _SCRIPTS not in sys.path:
    sys.path.insert(0, _SCRIPTS)

import doctor  # noqa: E402
import memory_sync  # noqa: E402
import settings_patch  # noqa: E402
import state  # noqa: E402

REFUSAL = "Anthology supports macOS and Linux only; it is inactive on Windows."
RTK_TIMEOUT = 120   # seconds: setup is where the first download happens


def _sha256(text):
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def _unsupported():
    return platform.system() == "Windows"


def _plugin_version(root):
    try:
        data = json.loads((Path(root) / ".claude-plugin" / "plugin.json").read_text(encoding="utf-8"))
        return str(data["version"])
    except (OSError, ValueError, KeyError, TypeError):
        return "unknown"


def _data_dir(env):
    """The plugin data folder: this process's CLAUDE_PLUGIN_DATA, else what session start recorded."""
    found = env.get("CLAUDE_PLUGIN_DATA") or state.read().get("data_dir")
    return str(found) if found else None


def _lock_path(root):
    return Path(root) / "rtk.lock"


# -- rtk adapters: the only places that name Task 11's `rtk_install` interface ------------------


def _rtk_preview(root, data_dir):
    try:
        import rtk_install
    except ImportError:
        return {"skipped": "rtk wiring not present"}
    try:
        lock = json.loads(_lock_path(root).read_text(encoding="utf-8"))
    except (OSError, ValueError):
        lock = None
    if not isinstance(lock, dict):
        return {"skipped": "rtk wiring not present (no rtk.lock)"}
    try:
        target = rtk_install.target(platform.system(), platform.machine())
    except Exception as exc:
        return {"error": str(exc) or type(exc).__name__}
    version = lock.get("version")
    dest = Path(data_dir) / "rtk" / str(version) / "rtk" if data_dir and version else None
    return {"version": version, "target": target, "dest": str(dest) if dest else None}


def _rtk_apply(root, data_dir):
    """`(status, warning)`: reconcile (download and verify when missing or stale)."""
    try:
        import rtk_install
    except ImportError:
        return "skipped: rtk wiring not present", None
    if not data_dir:
        return "skipped: plugin data folder unknown", (
            "rtk was not installed: the plugin data folder is not known yet; it is installed at the "
            "next session start")
    try:
        result = rtk_install.reconcile(Path(data_dir), _lock_path(root), None, RTK_TIMEOUT)
    except Exception as exc:
        return "failed", "rtk install failed: %s" % (str(exc) or type(exc).__name__)[:200]
    if isinstance(result, str) and result:
        return "warning", result
    return "installed", None


# -- settings -------------------------------------------------------------------------------------


def _settings_plan(root, env):
    """`(path, text, new_text, diff, ops_to_store)`; ValueError / FileNotFoundError on bad input."""
    path = settings_patch.settings_path(env)
    text = settings_patch.read_text(path)
    ops = settings_patch.setup_ops(root, env=env)
    new_text, diff, stored = settings_patch.reconcile(text, ops, state.read().get("settings_ops") or [])
    return path, text, new_text, diff, stored


def _settings_error(exc):
    if isinstance(exc, FileNotFoundError):
        return "the anthology-guardrails plugin was not found next to anthology; reinstall it"
    return str(exc)


# -- memory ---------------------------------------------------------------------------------------


def _personal_dir(env):
    return memory_sync.anthology_home(env) / "personal-memory"


def _repo_info(cwd, env):
    root = memory_sync.repo_root(cwd, env=env)
    if root is None:
        return None
    return root, memory_sync.repo_key(root, env=env)


# -- commands -------------------------------------------------------------------------------------


def preview(plugin_root, cwd=None, env=None):
    env = dict(os.environ) if env is None else dict(env)
    if _unsupported():
        return {"supported": False, "message": REFUSAL}
    cwd = cwd or os.getcwd()
    data_dir = _data_dir(env)
    text = settings_patch.read_text(settings_patch.settings_path(env))
    settings = {"path": str(settings_patch.settings_path(env)), "sha256": _sha256(text),
                "bypass_note": settings_patch.BYPASS_NOTE}
    try:
        _path, _text, new_text, diff, _stored = _settings_plan(plugin_root, env)
        settings.update(changed=new_text != text, diff=diff)
    except (ValueError, FileNotFoundError) as exc:
        settings.update(changed=False, diff="", error=_settings_error(exc))
    info = _repo_info(cwd, env)
    if info is None:
        repo = {"skipped": "not a git repo"}
    else:
        repo = {"root": str(info[0]), "key": info[1], "approved": memory_sync.is_approved(info[1], env=env)}
    personal = _personal_dir(env)
    return {
        "supported": True,
        "doctor": {"checks": doctor.check_env(env, lock=_lock_path(plugin_root))},
        "duplicates": doctor.find_duplicates(doctor.installed_plugins(env)),
        "rtk": _rtk_preview(plugin_root, data_dir),
        "settings": settings,
        "memory": {"repo": repo, "personal_dir": str(personal), "personal_exists": personal.is_dir()},
    }


def apply(plugin_root, expect_sha256, cwd=None, skip_approve=False, env=None):
    env = dict(os.environ) if env is None else dict(env)
    if _unsupported():
        return {"ok": False, "unsupported": True, "error": REFUSAL}
    cwd = cwd or os.getcwd()
    warnings = []
    path = settings_patch.settings_path(env)
    text = settings_patch.read_text(path)
    if _sha256(text) != expect_sha256:
        return {"ok": False, "error": "settings.json changed since the preview; run the preview again"}
    try:
        _path, _text, new_text, diff, stored = _settings_plan(plugin_root, env)
    except (ValueError, FileNotFoundError) as exc:
        return {"ok": False, "error": _settings_error(exc)}

    data_dir = _data_dir(env)
    rtk_status, warn = _rtk_apply(plugin_root, data_dir)
    if warn:
        warnings.append(warn)

    created = bool(state.read().get("settings_created")) or not os.path.lexists(str(path))
    changed = new_text != text
    if changed:
        settings_patch.write_text(path, new_text)
    state.write(settings_ops=stored, settings_created=created)

    memory = {"approved_key": None, "approve_error": None}
    info = None if skip_approve else _repo_info(cwd, env)
    if info is not None:
        root, key = info
        memory["approved_key"] = key
        try:
            memory["approved_key"] = memory_sync.approve(root, env=env)
        except memory_sync.UnsafeTarget as exc:  # the approval is already recorded
            memory["approve_error"] = str(exc)
            warnings.append("repo memory is approved, but: %s" % exc)
    personal = _personal_dir(env)
    fresh = not personal.is_dir()
    personal.mkdir(parents=True, exist_ok=True)
    if fresh:
        os.chmod(str(personal), 0o700)
    memory["personal_dir"] = str(personal)

    done = {"setup_done": _plugin_version(plugin_root)}
    if data_dir:
        done["data_dir"] = data_dir
    state.write(**done)
    return {"ok": True, "settings": {"path": str(path), "changed": changed, "diff": diff},
            "rtk": {"status": rtk_status}, "memory": memory, "warnings": warnings}


def _remove_rtk(env, warnings):
    data_dir = _data_dir(env)
    if not data_dir:
        return "no plugin data folder recorded"
    rtk_dir = Path(data_dir) / "rtk"
    if not os.path.lexists(str(rtk_dir)):
        return "nothing to delete"
    if rtk_dir.is_symlink():
        warnings.append("the rtk folder is a symlink; left in place")
        return "left in place"
    real = os.path.realpath(str(rtk_dir))
    config = os.path.realpath(str(memory_sync.config_dir(env)))
    if os.path.basename(real) != "rtk" or not real.startswith(config + os.sep):
        warnings.append("the rtk folder is outside the Claude config folder; left in place")
        return "left in place"
    shutil.rmtree(real)
    return "deleted"


def remove(plugin_root, cwd=None, unapprove=False, unapprove_all=False, env=None):
    env = dict(os.environ) if env is None else dict(env)
    if _unsupported():
        return {"ok": False, "unsupported": True, "error": REFUSAL}
    cwd = cwd or os.getcwd()
    warnings = []
    st = state.read()
    ops = st.get("settings_ops") or []
    path = settings_patch.settings_path(env)
    text = settings_patch.read_text(path)
    new_text = text
    if ops:
        try:
            new_text, _diff = settings_patch.revert(text, ops)
        except ValueError as exc:
            return {"ok": False, "error": "%s; left untouched. Fix the file and run remove again." % exc}
    changed = new_text != text
    if changed:
        if new_text == "" and st.get("settings_created"):
            os.unlink(str(path))
        else:
            settings_patch.write_text(path, new_text)

    rtk = _remove_rtk(env, warnings)
    unapproved = []
    if unapprove_all:
        for key in sorted(memory_sync._load_approved(env)["repos"]):
            if memory_sync.unapprove(key, env=env):
                unapproved.append(key)
    elif unapprove:
        info = _repo_info(cwd, env)
        if info is not None and memory_sync.unapprove(info[1], env=env):
            unapproved.append(info[1])
    try:
        os.unlink(str(state.state_path()))
    except FileNotFoundError:
        pass
    return {"ok": True, "settings": {"path": str(path), "changed": changed}, "rtk": {"status": rtk},
            "memory": {"unapproved": unapproved, "notes": "kept (memory notes are never deleted)"},
            "warnings": warnings}


def main(argv=None):
    parser = argparse.ArgumentParser(description="/anthology:setup: preview, apply and remove")
    sub = parser.add_subparsers(dest="command", required=True)
    for name in ("preview", "apply", "remove"):
        p = sub.add_parser(name)
        p.add_argument("--plugin-root", required=True)
        p.add_argument("--cwd")
        p.add_argument("--json", action="store_true", help="accepted for clarity; output is always JSON")
        if name == "apply":
            p.add_argument("--expect-sha256", required=True)
            p.add_argument("--skip-approve", action="store_true")
        if name == "remove":
            p.add_argument("--unapprove", action="store_true")
            p.add_argument("--unapprove-all", action="store_true")
    args = parser.parse_args(argv)
    try:
        if args.command == "preview":
            result = preview(args.plugin_root, cwd=args.cwd)
        elif args.command == "apply":
            result = apply(args.plugin_root, args.expect_sha256, cwd=args.cwd, skip_approve=args.skip_approve)
        else:
            result = remove(args.plugin_root, cwd=args.cwd, unapprove=args.unapprove,
                            unapprove_all=args.unapprove_all)
    except Exception as exc:
        result = {"ok": False, "error": "%s: %s" % (type(exc).__name__, exc)}
    print(json.dumps(result, indent=2))
    if result.get("supported") is False or result.get("unsupported"):
        return 2
    return 0 if result.get("ok", True) else 1


if __name__ == "__main__":
    sys.exit(main())
