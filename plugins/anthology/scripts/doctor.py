#!/usr/bin/env python3
"""Environment checks for Anthology (`/anthology:setup` runs this first).

`check_env(env)` returns an ordered list of `{"name", "ok", "detail"}` (ok is True, False, or
None for "cannot tell yet"). `find_duplicates(installed)` lists installs of the official
`superpowers` and `design-skills` plugins, which Anthology already bundles. Nothing here
prints a URL, a token or a path out of the user's MCP configuration. Python stdlib only (3.9+).

    python3 doctor.py [--json]
"""
from __future__ import annotations

import argparse
import json
import os
import platform
import shutil
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import settings_patch  # noqa: E402

BROWSEROS_APP = Path("/Applications/BrowserOS.app")
DOCKER_TIMEOUT = 10
SUPPORTED_SYSTEMS = ("Darwin", "Linux")
DUPLICATE_NAMES = ("superpowers", "design-skills")
DEFAULT_LOCK = Path(__file__).resolve().parent.parent / "rtk.lock"
CHECKS = ("python3", "node", "docker", "browseros", "rtk", "platform_supported", "plugin_read_rule")


def _config_dir(env):
    base = env.get("CLAUDE_CONFIG_DIR")
    if base:
        return Path(base)
    return Path(env.get("HOME") or os.path.expanduser("~")) / ".claude"


def _which(name, env):
    return shutil.which(name, path=env.get("PATH"))


def _read_json(path):
    try:
        return json.loads(Path(path).read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None


def _check_python(env):
    if _which("python3", env):
        return True, "python3 is on PATH"
    return False, "python3 not found on PATH; install Python 3.9 or newer"


def _check_node(env):
    if _which("node", env):
        return True, "Node is on PATH"
    return False, "Node not found on PATH; install Node.js (project tooling and some tests need it)"


def _check_docker(env):
    docker = _which("docker", env)
    if not docker:
        return False, "Docker not found on PATH; install Docker (OrbStack or Docker Desktop)"
    try:
        proc = subprocess.run([docker, "info"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                              timeout=DOCKER_TIMEOUT, env=env)
    except (OSError, subprocess.TimeoutExpired):
        return False, "Docker is on PATH but did not answer; start Docker and try again"
    if proc.returncode == 0:
        return True, "Docker is running"
    return False, "Docker is on PATH but not running; start Docker and try again"


def _check_browseros(env):
    claude_json = (Path(env["CLAUDE_CONFIG_DIR"]) / ".claude.json" if env.get("CLAUDE_CONFIG_DIR")
                   else Path(env.get("HOME") or os.path.expanduser("~")) / ".claude.json")
    data = _read_json(claude_json)
    servers = data.get("mcpServers") if isinstance(data, dict) else None
    if isinstance(servers, dict) and any("browseros" in str(name).lower() for name in servers):
        return True, "a BrowserOS MCP server is configured"
    if BROWSEROS_APP.exists() or _which("browseros", env):
        return True, "BrowserOS is installed"
    return False, ("BrowserOS Neo not found. It is the only browser Anthology uses; install it "
                   "yourself (agents never install a browser) and add its MCP server to Claude Code")


def _check_rtk(env, lock):
    """Adapter over Task 11's `rtk_install` (the one place that names its interface).

    The lock pins each release tarball, not the binary, so the check is: state.json's `rtk_path`
    is `<data>/rtk/<lock version>/rtk`, and rtk_install says that version dir holds an executable
    rtk whose recorded tarball hash equals the lock's hash for this platform."""
    try:
        import rtk_install
    except ImportError:
        return None, "rtk is not wired yet"
    lock_path = Path(lock) if lock else DEFAULT_LOCK
    try:
        data = rtk_install.load_lock(lock_path)
    except Exception:
        return None, "rtk is not wired yet (no rtk.lock)"
    try:
        target = rtk_install.target(platform.system(), platform.machine())
    except Exception as exc:
        return False, str(exc) or "this platform is not supported"
    state = _read_json(_config_dir(env) / "anthology" / "state.json")
    state = state if isinstance(state, dict) else {}
    path = state.get("rtk_path")
    if not path or not os.path.exists(str(path)):
        return False, "rtk is not installed; run /anthology:setup"
    # session start records data_dir; failing that, the folder three levels above the binary
    parents = Path(str(path)).parents
    data_dir = env.get("CLAUDE_PLUGIN_DATA") or state.get("data_dir") or (parents[2] if len(parents) > 2 else "")
    expected = Path(str(data_dir)) / "rtk" / data["version"] / "rtk"
    if Path(str(path)) == expected and rtk_install.check_installed(data_dir, data, target):
        return True, "rtk matches rtk.lock"
    return False, "rtk does not match rtk.lock; run /anthology:setup"


def _check_platform(env):
    system = platform.system()
    if system in SUPPORTED_SYSTEMS:
        return True, "%s is supported" % system
    return False, "Anthology supports macOS and Linux only"


def _check_plugin_rule(env, plugin_root):
    """The U1 plugin-read allow rule (same helper as session start). None until setup has run."""
    state = _read_json(_config_dir(env) / "anthology" / "state.json")
    if not isinstance(state, dict) or "setup_done" not in state:
        return None, "setup has not run yet"
    root = plugin_root or env.get("CLAUDE_PLUGIN_ROOT") or Path(__file__).resolve().parent.parent
    status = settings_patch.plugin_rule_status(root, env)
    if status == "ok":
        return True, "the plugin-read permission matches this install"
    if status == "unreadable":
        return False, "settings.json cannot be read; fix it, then re-run /anthology:setup"
    return False, "the plugin moved or the rule is gone; re-run /anthology:setup"


def check_env(env=None, lock=None, plugin_root=None):
    env = dict(os.environ) if env is None else env
    runners = (
        ("python3", _check_python), ("node", _check_node), ("docker", _check_docker),
        ("browseros", _check_browseros), ("rtk", lambda e: _check_rtk(e, lock)),
        ("platform_supported", _check_platform),
        ("plugin_read_rule", lambda e: _check_plugin_rule(e, plugin_root)))
    out = []
    for name, fn in runners:
        try:
            ok, detail = fn(env)
        except Exception as exc:
            ok, detail = False, "check failed: %s" % (str(exc) or type(exc).__name__)[:120]
        out.append({"name": name, "ok": ok, "detail": detail})
    return out


def find_duplicates(installed):
    """Installs of the official `superpowers` / `design-skills` plugins, with U4 uninstall
    commands. `installed` is the parsed `installed_plugins.json` (version 2)."""
    plugins = installed.get("plugins") if isinstance(installed, dict) else None
    found = []
    for name, entries in sorted(plugins.items()) if isinstance(plugins, dict) else ():
        if str(name).split("@", 1)[0] not in DUPLICATE_NAMES or not isinstance(entries, list):
            continue
        for entry in entries:
            if not isinstance(entry, dict):
                continue
            scope = entry.get("scope") if isinstance(entry.get("scope"), str) else "user"
            found.append({"plugin": name, "scope": scope,
                          "command": "claude plugin uninstall %s -s %s" % (name, scope)})
    return found


def installed_plugins(env=None):
    env = dict(os.environ) if env is None else env
    return _read_json(_config_dir(env) / "plugins" / "installed_plugins.json")


def main(argv=None):
    parser = argparse.ArgumentParser(description="Anthology environment checks")
    parser.add_argument("--json", action="store_true", help="print the result as JSON")
    args = parser.parse_args(argv)
    env = dict(os.environ)
    result = {"checks": check_env(env), "duplicates": find_duplicates(installed_plugins(env))}
    if args.json:
        print(json.dumps(result, indent=2))
    else:
        for c in result["checks"]:
            mark = {True: "ok", False: "FAIL", None: "n/a"}[c["ok"]]
            print("%-18s %-4s %s" % (c["name"], mark, c["detail"]))
        for d in result["duplicates"]:
            print("duplicate: %s (%s): %s" % (d["plugin"], d["scope"], d["command"]))
    supported = [c for c in result["checks"] if c["name"] == "platform_supported"][0]["ok"]
    return 0 if supported else 2


if __name__ == "__main__":
    sys.exit(main())
