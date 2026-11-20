"""Finds the sibling anthology-guardrails plugin (shared by session start, setup and doctor).

Order: (1) `<plugin root>/../anthology-guardrails` (a local marketplace or `--plugin-dir`
layout), else (2) the newest `anthology-guardrails@*` entry of `installed_plugins.json` whose
install path exists. Python stdlib only (3.9+).
"""
from __future__ import annotations

import importlib.util
import json
import os
import sys
from pathlib import Path


def _config_dir(env):
    env = os.environ if env is None else env
    base = env.get("CLAUDE_CONFIG_DIR")
    if base:
        return Path(base)
    return Path(env.get("HOME") or os.path.expanduser("~")) / ".claude"


def _has_guard(path):
    return (Path(path) / "scripts" / "guard.py").is_file()


def locate(plugin_root, env=None):
    """The guardrails plugin folder (it has `scripts/guard.py`), or None."""
    sibling = Path(plugin_root).parent / "anthology-guardrails"
    if _has_guard(sibling):
        return sibling
    try:
        data = json.loads((_config_dir(env) / "plugins" / "installed_plugins.json").read_text(encoding="utf-8"))
        plugins = data.get("plugins") if isinstance(data, dict) else None
        best = None
        for name, entries in (plugins.items() if isinstance(plugins, dict) else ()):
            if not str(name).startswith("anthology-guardrails@") or not isinstance(entries, list):
                continue
            for entry in entries:
                if not isinstance(entry, dict) or not isinstance(entry.get("installPath"), str):
                    continue
                if not _has_guard(entry["installPath"]):
                    continue
                stamp = str(entry.get("lastUpdated") or "")
                if best is None or stamp > best[0]:
                    best = (stamp, entry["installPath"])
        return Path(best[1]) if best else None
    except (OSError, ValueError, AttributeError):
        return None


def load_rules(plugin_root, env=None):
    """Import the located plugin's `guard_rules.py` (for `SELF_TEST_SAMPLE`, `PROFILE_ROOTS`).

    Raises FileNotFoundError when the plugin or the file is missing, ImportError when it will
    not load."""
    base = locate(plugin_root, env)
    if base is None:
        raise FileNotFoundError("anthology-guardrails plugin not found")
    path = base / "scripts" / "guard_rules.py"
    if not path.is_file():
        raise FileNotFoundError("anthology-guardrails has no scripts/guard_rules.py")
    name = "_anthology_guard_rules"
    spec = importlib.util.spec_from_file_location(name, str(path))
    if spec is None or spec.loader is None:
        raise ImportError("cannot load %s" % path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    try:
        spec.loader.exec_module(module)
    except Exception as exc:
        sys.modules.pop(name, None)
        raise ImportError("cannot load %s: %s" % (path, exc))
    sys.modules.pop(name, None)
    return module
