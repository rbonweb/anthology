"""Finds the sibling anthology-guardrails plugin (shared by session start, setup and doctor).

`inspect(plugin_root, env, cwd)` returns `(folder, problem)` (F20):

- `installed_plugins.json` is the source of truth. An `anthology-guardrails@*` entry counts when
  its install path has `scripts/guard.py` and its scope is `user`, or `project`/`local` with a
  `projectPath` containing `cwd`; the newest `lastUpdated` wins.
- The folder returned is the one the PreToolUse hook really runs from (B2), which is not always
  the entry's `installPath` (an install-time snapshot in `<config>/plugins/cache/`):
  - `directory` marketplace (`known_marketplaces.json`, `source.source == "directory"`): Claude
    Code loads plugins in place, so the folder is `installLocation` joined with the plugin's
    relative `source` in `<installLocation>/.claude-plugin/marketplace.json`. The source must
    stay inside `installLocation` after realpath (no absolute path, no escaping `..`), otherwise
    the problem is `live_missing`. So is a contained folder without `scripts/guard.py`, a plugin
    that marketplace.json does not list, and a missing or invalid marketplace.json (folder None
    in every case): the hook runs the live folder, so the cache copy never stands in.
  - any other source type (`github`, `git`, `url`, ...): the plugins run from the cache, so the
    folder is `installPath`.
  - fallback (`known_marketplaces.json` missing, unreadable, or without the marketplace): when `plugin_root` is not under
    `<config>/plugins/cache/` it is loaded in place and the sibling
    `<plugin root>/../anthology-guardrails` is used if it has `scripts/guard.py` and its realpath
    stays inside `<plugin root>/..` (a symlink escaping it is refused); else `installPath`.
- Only when the registry has no `anthology@*` entry either (a `--plugin-dir` dev session) is the
  sibling folder `<plugin root>/../anthology-guardrails` used (same containment rule).
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


def _under(path, parent, strict=False):
    """Whether realpath(path) is inside realpath(parent); `strict` also refuses the parent itself."""
    try:
        path, parent = os.path.realpath(str(path)), os.path.realpath(str(parent))
    except (OSError, ValueError):
        return False
    if path == parent:
        return not strict
    return path.startswith(parent.rstrip(os.sep) + os.sep)


def _directory_root(config, marketplace, plugin):
    """The live guardrails folder of a `directory` marketplace (contained in its install location,
    whether or not it has guard.py), or None when it is not one or any piece is missing or invalid.
    Never raises."""
    try:
        known = _read_json(config / "plugins" / "known_marketplaces.json")
        item = known.get(marketplace) if isinstance(known, dict) else None
        source = item.get("source") if isinstance(item, dict) else None
        if not isinstance(source, dict) or source.get("source") != "directory":
            return None
        location = item.get("installLocation")
        if not isinstance(location, str) or not location or not os.path.isabs(location):
            return None
        manifest = _read_json(Path(location) / ".claude-plugin" / "marketplace.json")
        listed = manifest.get("plugins") if isinstance(manifest, dict) else None
        for spec in listed if isinstance(listed, list) else ():
            if not isinstance(spec, dict) or spec.get("name") != plugin:
                continue
            rel = spec.get("source")
            if not isinstance(rel, str) or not rel or os.path.isabs(rel) or "\x00" in rel:
                return None
            root = Path(os.path.realpath(os.path.join(location, rel)))
            return root if _under(root, location) else None
    except (OSError, ValueError, TypeError, RecursionError):
        return None
    return None


def _marketplace_kind(config, marketplace):
    """`"directory"`, `"other"` (a readable source of another type) or None (unreadable)."""
    known = _read_json(config / "plugins" / "known_marketplaces.json")
    item = known.get(marketplace) if isinstance(known, dict) else None
    source = item.get("source") if isinstance(item, dict) else None
    kind = source.get("source") if isinstance(source, dict) else None
    if not isinstance(kind, str):
        return None
    return "directory" if kind == "directory" else "other"


def _contained_sibling(plugin_root):
    """`<plugin root>/../anthology-guardrails` when it has guard.py and does not escape the folder
    next to the plugin (a symlink out of it, or to the folder itself, is refused), else None."""
    parent = Path(plugin_root).parent
    sibling = parent / "anthology-guardrails"
    return sibling if _has_guard(sibling) and _under(sibling, parent, strict=True) else None


def _live_root(plugin_root, env, name, install_path):
    """Map a chosen registry entry to `(folder, problem)`: the folder the hook executes (see the
    module docstring), or `(None, "live_missing")`."""
    config = _config_dir(env)
    marketplace = name.split("@", 1)[1] if "@" in name else ""
    kind = _marketplace_kind(config, marketplace)
    if kind == "other":
        return Path(install_path), None
    if kind == "directory":
        # never the cache copy: a source that is refused, unlisted or has no manifest is a problem too
        live = _directory_root(config, marketplace, "anthology-guardrails")
        if live is not None and _has_guard(live):
            return live, None
        return None, "live_missing"
    if not _under(plugin_root, config / "plugins" / "cache"):
        sibling = _contained_sibling(plugin_root)
        if sibling is not None:
            return sibling, None
    return Path(install_path), None


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
                best = (stamp, entry["installPath"], str(name))
    found, problem = None, None
    if best is not None:
        found, problem = _live_root(plugin_root, env, best[2], best[1])
    elif any(str(name).startswith("anthology@") for name in plugins):
        problem = "not_installed"
    else:
        found = _contained_sibling(plugin_root)
        if found is None:
            problem = "not_found"
    if problem in (None, "live_missing") and _disabled(env, cwd):
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
