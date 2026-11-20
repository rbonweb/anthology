"""Finds the sibling anthology-guardrails plugin (shared by session start, setup and doctor).

`inspect(plugin_root, env, cwd)` returns `(folder, problem)` (F20):

- `installed_plugins.json` is the source of truth. An `anthology-guardrails@*` entry counts when
  its install path has `scripts/guard.py` and its scope is `user`, or `project`/`local` with a
  `projectPath` containing `cwd`; the newest `lastUpdated` wins.
- Only when the registry has no `anthology@*` entry either (a `--plugin-dir` dev session) is the
  sibling folder `<plugin root>/../anthology-guardrails` used.
- An install of anthology without an applicable guardrails entry is the problem `not_installed`;
  `enabledPlugins["anthology-guardrails@..."] == false` in the user, project or local settings is
  the problem `disabled`; nothing found at all is `not_found`.

Python stdlib only (3.9+).
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


def _read_json(path):
    try:
        text = Path(path).read_text(encoding="utf-8")
        return json.loads(text[1:] if text.startswith("\ufeff") else text)
    except (OSError, ValueError, RecursionError):
        return None


def _inside(cwd, project):
    try:
        cwd = os.path.realpath(str(cwd))
        project = os.path.realpath(str(project))
    except (OSError, ValueError):
        return False
    return cwd == project or cwd.startswith(project.rstrip(os.sep) + os.sep)


def _disabled(env, cwd):
    files = [_config_dir(env) / "settings.json", Path(cwd) / ".claude" / "settings.json",
             Path(cwd) / ".claude" / "settings.local.json"]
    for path in files:
        data = _read_json(path)
        enabled = data.get("enabledPlugins") if isinstance(data, dict) else None
        if isinstance(enabled, dict):
            for name, value in enabled.items():
                if str(name).startswith("anthology-guardrails@") and value is False:
                    return True
    return False


def inspect(plugin_root, env=None, cwd=None):
    """`(folder or None, problem or None)`; see the module docstring."""
    cwd = cwd or os.getcwd()
    data = _read_json(_config_dir(env) / "plugins" / "installed_plugins.json")
    plugins = data.get("plugins") if isinstance(data, dict) else None
    plugins = plugins if isinstance(plugins, dict) else {}
    best = None
    for name, entries in plugins.items():
        if not str(name).startswith("anthology-guardrails@") or not isinstance(entries, list):
            continue
        for entry in entries:
            if not isinstance(entry, dict) or not isinstance(entry.get("installPath"), str):
                continue
            if not _has_guard(entry["installPath"]):
                continue
            scope = entry.get("scope")
            if scope in ("project", "local") and not (
                    isinstance(entry.get("projectPath"), str) and _inside(cwd, entry["projectPath"])):
                continue
            stamp = str(entry.get("lastUpdated") or "")
            if best is None or stamp > best[0]:
                best = (stamp, entry["installPath"])
    found, problem = None, None
    if best is not None:
        found = Path(best[1])
    elif any(str(name).startswith("anthology@") for name in plugins):
        problem = "not_installed"
    else:
        sibling = Path(plugin_root).parent / "anthology-guardrails"
        if _has_guard(sibling):
            found = sibling
        else:
            problem = "not_found"
    if problem is None and _disabled(env, cwd):
        problem = "disabled"
    return found, problem


def locate(plugin_root, env=None, cwd=None):
    """The guardrails plugin folder (it has `scripts/guard.py`), or None."""
    return inspect(plugin_root, env, cwd)[0]


def load_rules(plugin_root, env=None, cwd=None):
    """Import the located plugin's `guard_rules.py` (for `SELF_TEST_SAMPLE`, `PROFILE_ROOTS`).

    Raises FileNotFoundError when the plugin or the file is missing, ImportError when it will
    not load."""
    base = locate(plugin_root, env, cwd)
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
