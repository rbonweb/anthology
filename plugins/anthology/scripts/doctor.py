#!/usr/bin/env python3
"""Environment checks for Anthology (`/anthology:setup` runs this first).

`check_env(env)` returns an ordered list of `{"name", "ok", "detail"}` (ok is True, False, or
None for "cannot tell yet"). `find_duplicates(installed)` lists installs of the official
`superpowers` and `design-skills` plugins, which Anthology already bundles. Nothing here
prints a URL, a token or a path out of the user's MCP configuration. Python stdlib only (3.9+).

Limitation: the BrowserOS tool-name check (`browseros_tools`) reads configuration files only. It
never connects to the server, so it cannot list the tools the server really offers.

    python3 doctor.py [--json]
"""
from __future__ import annotations

import argparse
import json
import os
import platform
import re
import shutil
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import compat  # noqa: E402
import settings_patch  # noqa: E402

SYSTEM_APPLICATIONS = Path("/Applications")
MANAGED_MCP_PATHS = (Path("/Library/Application Support/ClaudeCode/managed-mcp.json"),
                     Path("/etc/claude-code/managed-mcp.json"))
BROWSEROS_BINARIES = ("browseros", "browseros-neo", "BrowserOS")
EXPECTED_TOOL_SERVER = "browseros-neo"
DOCKER_TIMEOUT = 10
SUPPORTED_SYSTEMS = ("Darwin", "Linux")
DUPLICATE_NAMES = ("superpowers", "design-skills")
UNINSTALL_SCOPES = ("user", "project", "local")  # `claude plugin uninstall -s` accepts these
SAFE_PLUGIN_KEY = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]*(@[A-Za-z0-9][A-Za-z0-9._-]*)?")
SHOWN_NAME_LIMIT = 80
SHOWN_SERVERS = 5
DEFAULT_LOCK = Path(__file__).resolve().parent.parent / "rtk.lock"
CHECKS = ("python3", "node", "docker", "browseros", "rtk", "platform_supported", "plugin_read_rule",
          "browseros_tools", "plugin_data", "path_wrappers", "agent_memory_path", "subagent_model")


def _config_dir(env):
    base = env.get("CLAUDE_CONFIG_DIR")
    if base:
        return Path(base)
    return Path(env.get("HOME") or os.path.expanduser("~")) / ".claude"


def _which(name, env):
    return shutil.which(name, path=env.get("PATH"))


def _read_json(path):
    return compat.read_json(path)  # None when missing or malformed, including absurdly nested JSON


def _safe(value, limit=SHOWN_NAME_LIMIT):
    """`value` escaped for a terminal line: control and non-printable characters (ANSI escapes,
    newlines, bidi marks) become backslash escapes, as in `repr`; long values are cut."""
    text = repr(str(value))[1:-1]
    return text if len(text) <= limit else text[:limit] + "..."


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


def _home(env):
    return Path(env.get("HOME") or os.path.expanduser("~"))


def _servers_in(data):
    """Server names in an `mcpServers` object (names only; nothing else is ever read out)."""
    return [str(n) for n in data] if isinstance(data, dict) else []


def find_mcp_servers(env, cwd):
    """Every MCP server Claude Code would see from `cwd`, as `{"name", "scope", "tool_prefix"}`.

    Scopes: user and local (`.claude.json`), project (`.mcp.json`), plugin (installed plugins'
    `.mcp.json` and `plugin.json`), managed. Reads names only, never a URL, command, argument,
    header or token. Missing or malformed files are skipped. Never connects to a server.

    A project server also carries `"approved"`: Claude Code loads a repo's `.mcp.json` servers only
    after the user approves them (see `_project_approval`); the other scopes always load."""
    found = []

    def add(names, scope, head="mcp__"):
        for name in names:
            found.append({"name": name, "scope": scope, "tool_prefix": head + name + "__"})

    base = Path(env["CLAUDE_CONFIG_DIR"]) if env.get("CLAUDE_CONFIG_DIR") else _home(env)
    data = compat.read_json(base / ".claude.json")
    data = data if isinstance(data, dict) else {}
    add(_servers_in(data.get("mcpServers")), "user")

    here = Path(os.path.realpath(str(cwd)))
    projects = data.get("projects")
    if isinstance(projects, dict):
        for folder in (here, *here.parents):
            entry = projects.get(str(folder))
            if isinstance(entry, dict):
                add(_servers_in(entry.get("mcpServers")), "local")

    top = compat.repo_top(cwd)
    for folder in (here, *here.parents):
        mcp = compat.read_json(folder / ".mcp.json", compat.REPO_READ_LIMIT)  # the clone controls it
        if mcp is not None:
            names = _servers_in(mcp.get("mcpServers") if isinstance(mcp, dict) else None)
            approved = _project_approval(env, data, here)
            for name in names:
                found.append({"name": name, "scope": "project", "tool_prefix": "mcp__%s__" % name,
                              "approved": approved(name)})
            break
        if folder == top:
            break

    installed = compat.read_json(compat.config_dir(env) / "plugins" / "installed_plugins.json")
    plugins = installed.get("plugins") if isinstance(installed, dict) else None
    for key, entries in sorted(plugins.items()) if isinstance(plugins, dict) else ():
        plugin = str(key).split("@", 1)[0]
        for entry in entries if isinstance(entries, list) else ():
            path = entry.get("installPath") if isinstance(entry, dict) else None
            if not isinstance(path, str) or not path:
                continue
            names = []
            mcp = compat.read_json(Path(path) / ".mcp.json", compat.REPO_READ_LIMIT)
            if isinstance(mcp, dict):
                names += _servers_in(mcp["mcpServers"] if isinstance(mcp.get("mcpServers"), dict) else mcp)
            for manifest in (Path(path) / ".claude-plugin" / "plugin.json", Path(path) / "plugin.json"):
                meta = compat.read_json(manifest, compat.REPO_READ_LIMIT)
                if isinstance(meta, dict):
                    names += _servers_in(meta.get("mcpServers"))
            add(sorted(set(names)), "plugin", "mcp__plugin_" + plugin + "_")

    for path in MANAGED_MCP_PATHS:
        managed = compat.read_json(path)
        if isinstance(managed, dict):
            add(_servers_in(managed.get("mcpServers")), "managed")
    return found


def _names(value):
    return {n for n in value if isinstance(n, str)} if isinstance(value, list) else set()


def _project_approval(env, claude_json, here):
    """`approved(name)` for servers in a project `.mcp.json`: Claude Code 2.1.329 loads one only when
    it is approved and not disabled. Doctor is never more permissive than that, and deliberately
    stricter in one place (below). Ruling: m14.

    - `disabledMcpjsonServers` from ANY source (user settings.json, `.claude.json`, and the
      working folder's `.claude/settings.json` and `settings.local.json`) rejects the server.
    - `enableAllProjectMcpServers: true` or the name in `enabledMcpjsonServers` approves it when it
      comes from the user's own settings.json or the project's entry in `.claude.json`.
    - The same keys in `<cwd>/.claude/settings.json` and `<cwd>/.claude/settings.local.json` (where
      Claude Code reads project settings from) approve it only when the workspace is trusted:
      `.claude.json` `projects[<key>].hasTrustDialogAccepted` is `true`, `<key>` being the git
      root of the working folder, else the folder itself. Not determinable means untrusted.
      Stricter than Claude Code: it also honours an untracked settings.local.json before trust;
      doctor does not, by design (it cannot tell whether git tracks the file without running git).
    - Repo files are read only up to `REPO_READ_LIMIT`; anything unreadable or oddly shaped approves
      nothing and disables nothing."""
    projects = claude_json.get("projects") if isinstance(claude_json, dict) else None
    projects = projects if isinstance(projects, dict) else {}
    key = compat.repo_top(here) or here
    entry = projects.get(str(key))
    trusted = isinstance(entry, dict) and entry.get("hasTrustDialogAccepted") is True
    own = [_read_json(compat.config_dir(env) / "settings.json")]
    own += [projects.get(str(folder)) for folder in (here, *here.parents)]
    repo = [compat.read_json(here / ".claude" / name, compat.REPO_READ_LIMIT)
            for name in ("settings.json", "settings.local.json")]
    own = [d for d in own if isinstance(d, dict)]
    repo = [d for d in repo if isinstance(d, dict)]
    approving = own + (repo if trusted else [])
    everything = any(d.get("enableAllProjectMcpServers") is True for d in approving)
    enabled = set().union(*[_names(d.get("enabledMcpjsonServers")) for d in approving])
    disabled = set().union(*[_names(d.get("disabledMcpjsonServers")) for d in own + repo])
    return lambda name: name not in disabled and (everything or name in enabled)


def _browseros_servers(env, cwd, loaded=True):
    """BrowserOS servers Claude Code loads (`loaded`), or those in a project file nobody approved."""
    return [s for s in find_mcp_servers(env, cwd)
            if "browseros" in s["name"].lower() and s.get("approved", True) == loaded]


def _unapproved_note(env, cwd):
    names = [_safe(s["name"]) for s in _browseros_servers(env, cwd, loaded=False)]
    if not names:
        return ""
    shown = ", ".join(names[:SHOWN_SERVERS]) + (" and %d more" % (len(names) - SHOWN_SERVERS)
                                                 if len(names) > SHOWN_SERVERS else "")
    return ("found in an unapproved project .mcp.json (%s), so it is not counted; approve it in Claude "
            "Code or add the server at user scope" % shown)


def _browseros_installed(env):
    """A BrowserOS app (macOS), binary on PATH or AppImage (Linux), through injectable roots."""
    home = _home(env)
    for root in (SYSTEM_APPLICATIONS, home / "Applications"):
        try:
            if any(p.is_dir() for p in root.glob("BrowserOS*.app")):
                return True
        except OSError:
            pass
    for root in (home / "Applications", home / ".local" / "bin"):
        try:
            if any(p.is_file() for p in root.glob("BrowserOS*.AppImage")):
                return True
        except OSError:
            pass
    return any(_which(name, env) for name in BROWSEROS_BINARIES)


def _check_browseros(env, cwd):
    if _browseros_servers(env, cwd):
        return True, "a BrowserOS MCP server is configured"
    note = _unapproved_note(env, cwd)
    if _browseros_installed(env):
        return True, "BrowserOS is installed" + ("; a BrowserOS MCP server was " + note if note else "")
    return False, ("BrowserOS Neo not found. It is the only browser Anthology uses; install it "
                   "yourself (agents never install a browser) and add its MCP server to Claude Code"
                   + ("; a BrowserOS MCP server was " + note if note else ""))


def _check_browseros_tools(env, cwd):
    """Agents and the guide call `mcp__browseros-neo__*`. Config files only; no live tool listing."""
    servers = _browseros_servers(env, cwd)
    if not servers:
        note = _unapproved_note(env, cwd)
        if note:
            return None, "no BrowserOS MCP server is loaded: one was " + note
        return None, "no BrowserOS MCP server is configured (see the browseros check)"
    if any(s["name"] == EXPECTED_TOOL_SERVER and s["scope"] != "plugin" for s in servers):
        return True, "the BrowserOS MCP server is named browseros-neo (tools: mcp__browseros-neo__*)"
    found = "; ".join("%s (%s scope, tools %s*)" % (_safe(s["name"]), s["scope"], _safe(s["tool_prefix"]))
                      for s in servers[:SHOWN_SERVERS])
    if len(servers) > SHOWN_SERVERS:
        found += "; and %d more" % (len(servers) - SHOWN_SERVERS)
    return False, ("agents call mcp__browseros-neo__* tools, but the BrowserOS server is configured as: %s. "
                   "Name the server browseros-neo (user, local, project or managed scope)" % found)


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


def _plugin_root(env, plugin_root):
    return Path(plugin_root or env.get("CLAUDE_PLUGIN_ROOT") or Path(__file__).resolve().parent.parent)


def _check_plugin_data(env):
    """`${CLAUDE_PLUGIN_DATA}` only exists in hook environments, so the folder session start
    recorded in state.json stands in for it."""
    state = _read_json(_config_dir(env) / "anthology" / "state.json")
    recorded = env.get("CLAUDE_PLUGIN_DATA") or (state.get("data_dir") if isinstance(state, dict) else None)
    if not recorded:
        return None, "session start has not run since install, so no plugin data folder is recorded yet"
    if Path(str(recorded)).is_dir():
        return True, "the plugin data folder exists"
    return False, ("session start recorded a data folder that is gone; start a new session, and if it "
                   "stays gone Claude Code changed where it keeps plugin data (report it)")


def _check_path_wrappers(env, plugin_root):
    root = _plugin_root(env, plugin_root)
    bad = compat.wrapper_problems(root)
    if bad:
        return False, "the plugin's bin/ is missing or not executable: %s (reinstall the plugin)" % ", ".join(
            "bin/" + n for n in bad)
    if env.get("CLAUDECODE"):
        found = _which("anthology-mem", env)
        try:
            ours = bool(found) and os.path.realpath(os.path.dirname(found)) == os.path.realpath(str(root / "bin"))
        except OSError:
            ours = False
        if not ours:
            return False, "the plugin's bin/ is not on PATH; agents cannot run anthology-mem"
        return True, "the plugin's bin/ wrappers exist and bin/ is on PATH"
    return True, "the plugin's bin/ wrappers exist and are executable"


def _check_agent_memory(env, cwd):
    bad, good = compat.agent_memory_problems(cwd)
    if bad:
        return None, compat.agent_memory_hint(bad[0])
    if good:
        return True, "agent memory is under .claude/agent-memory/anthology-<agent>/"
    return None, "no agent memory folders yet"


def _check_subagent_model(env, cwd):
    where = compat.subagent_model_source(env, cwd)
    if where is None:
        return True, "CLAUDE_CODE_SUBAGENT_MODEL is not set"
    return False, ("CLAUDE_CODE_SUBAGENT_MODEL is set (%s): it overrides every agent's model, so the "
                   "roster's opus/sonnet/haiku tiers do not apply. Anthology never changes it; remove it "
                   "yourself if you want the tiers." % where)


def check_env(env=None, lock=None, plugin_root=None, cwd=None):
    env = dict(os.environ) if env is None else env
    cwd = cwd or os.getcwd()
    runners = (
        ("python3", _check_python), ("node", _check_node), ("docker", _check_docker),
        ("browseros", lambda e: _check_browseros(e, cwd)), ("rtk", lambda e: _check_rtk(e, lock)),
        ("platform_supported", _check_platform),
        ("plugin_read_rule", lambda e: _check_plugin_rule(e, plugin_root)),
        ("browseros_tools", lambda e: _check_browseros_tools(e, cwd)),
        ("plugin_data", _check_plugin_data),
        ("path_wrappers", lambda e: _check_path_wrappers(e, plugin_root)),
        ("agent_memory_path", lambda e: _check_agent_memory(e, cwd)),
        ("subagent_model", lambda e: _check_subagent_model(e, cwd)))
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
    commands. `installed` is the parsed `installed_plugins.json` (version 2).

    Each item is `{"plugin", "scope", "command"}` with `plugin` and `scope` escaped for printing.
    `command` is only built from a scope in `UNINSTALL_SCOPES` (a missing scope means user) and a
    plugin key of plain name characters; otherwise it is None and no command is offered."""
    plugins = installed.get("plugins") if isinstance(installed, dict) else None
    found = []
    for name, entries in sorted(plugins.items()) if isinstance(plugins, dict) else ():
        if str(name).split("@", 1)[0] not in DUPLICATE_NAMES or not isinstance(entries, list):
            continue
        for entry in entries:
            if not isinstance(entry, dict):
                continue
            scope = entry.get("scope")
            scope = "user" if scope is None else scope
            command = None
            if isinstance(name, str) and scope in UNINSTALL_SCOPES and SAFE_PLUGIN_KEY.fullmatch(name):
                command = "claude plugin uninstall %s -s %s" % (name, scope)
            found.append({"plugin": _safe(name), "scope": _safe(scope), "command": command})
    return found


def installed_plugins_state(env=None):
    """`(parsed installed_plugins.json or None, "ok" | "missing" | "unreadable")`."""
    env = dict(os.environ) if env is None else env
    path = _config_dir(env) / "plugins" / "installed_plugins.json"
    try:
        text = path.read_text(encoding="utf-8")
    except FileNotFoundError:
        return None, "missing"
    except (OSError, ValueError):
        return None, "unreadable"
    try:
        return json.loads(text), "ok"
    except (ValueError, RecursionError):  # RecursionError: absurdly nested JSON
        return None, "unreadable"


def installed_plugins(env=None):
    return installed_plugins_state(env)[0]


def main(argv=None):
    parser = argparse.ArgumentParser(description="Anthology environment checks")
    parser.add_argument("--json", action="store_true", help="print the result as JSON")
    args = parser.parse_args(argv)
    env = dict(os.environ)
    installed, state = installed_plugins_state(env)
    result = {"checks": check_env(env), "duplicates": find_duplicates(installed), "installed_plugins": state}
    if args.json:
        print(json.dumps(result, indent=2))
    else:
        for c in result["checks"]:
            mark = {True: "ok", False: "FAIL", None: "n/a"}[c["ok"]]
            print("%-18s %-4s %s" % (c["name"], mark, c["detail"]))
        if state == "unreadable":
            print("duplicates: installed_plugins.json is unreadable, so installs of the bundled plugins "
                  "were not checked")
        for d in result["duplicates"]:
            if d["command"]:
                print("duplicate: %s (%s): %s" % (d["plugin"], d["scope"], d["command"]))
            else:
                print("duplicate: %s (scope '%s' is not one of %s): no uninstall command offered"
                      % (d["plugin"], d["scope"], ", ".join(UNINSTALL_SCOPES)))
    supported = [c for c in result["checks"] if c["name"] == "platform_supported"][0]["ok"]
    return 0 if supported else 2


if __name__ == "__main__":
    sys.exit(main())
