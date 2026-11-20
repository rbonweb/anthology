#!/usr/bin/env python3
"""Cheap compatibility checks shared by `doctor.py` and `session_start.py` (PLAN section 14).

Each helper reads a few small files and never runs a subprocess, so session start can call them.
Nothing here returns a value from the user's settings or MCP configuration: only names, scopes
and paths of folders Anthology owns. Python stdlib only (3.9+).
"""
from __future__ import annotations

import json
import os
import stat
from pathlib import Path

WRAPPERS = ("anthology-mem", "rtk", "rtk-hook")
SUBAGENT_MODEL = "CLAUDE_CODE_SUBAGENT_MODEL"
REPO_READ_LIMIT = 2 * 1024 * 1024  # files a cloned repo controls: Claude Code's own settings limit


def decode_text(raw):
    """Text of a JSON file the way Claude Code reads settings: UTF-16LE when it starts with that BOM,
    else UTF-8 with invalid bytes replaced; one leading BOM dropped. UTF-16BE with a BOM is decoded
    too, which Claude Code does not do (it reads that as UTF-8 and ignores the file): a harmless
    superset."""
    if raw.startswith(b"\xff\xfe"):
        text = raw[2:].decode("utf-16-le", errors="replace")
    elif raw.startswith(b"\xfe\xff"):
        text = raw[2:].decode("utf-16-be", errors="replace")
    else:
        text = raw.decode("utf-8", errors="replace")
    return text[1:] if text.startswith("\ufeff") else text


def read_json(path, limit=None):
    """The parsed JSON file, or None when it is missing or malformed. Decoded like Claude Code
    does (`decode_text`: a UTF-8 or UTF-16 BOM is fine, bad bytes do not matter).

    With `limit` (bytes) the file must also be a regular file (a symlink to one is fine; a device
    or FIFO is not) of at most `limit` bytes, else it is skipped like an absent file. Use it for
    files a cloned repo controls (`REPO_READ_LIMIT`): they are read at every session start."""
    if limit is None:
        try:
            return json.loads(decode_text(Path(path).read_bytes()))
        except (OSError, ValueError, RecursionError):
            return None
    try:
        fd = os.open(str(path), os.O_RDONLY | getattr(os, "O_NONBLOCK", 0))  # a FIFO must not block
    except (OSError, ValueError):
        return None
    try:
        if not stat.S_ISREG(os.fstat(fd).st_mode):
            return None
        with os.fdopen(fd, "rb", closefd=False) as fh:
            raw = fh.read(limit + 1)
        if len(raw) > limit:
            return None
        return json.loads(decode_text(raw))
    except (OSError, ValueError, RecursionError):
        return None
    finally:
        os.close(fd)


def config_dir(env):
    base = env.get("CLAUDE_CONFIG_DIR")
    if base:
        return Path(base)
    return Path(env.get("HOME") or os.path.expanduser("~")) / ".claude"


def repo_top(cwd):
    """The nearest folder at or above `cwd` holding a `.git` (folder or file), or None. No git."""
    try:
        here = Path(os.path.realpath(str(cwd)))
    except (OSError, ValueError, TypeError):
        return None
    try:
        for folder in (here, *here.parents):
            if (folder / ".git").exists():
                return folder
    except OSError:
        return None
    return None


def wrapper_problems(plugin_root):
    """Names of the PATH wrappers (`bin/anthology-mem`, `bin/rtk`, `bin/rtk-hook`) that are missing
    or not executable under the plugin root."""
    bad = []
    for name in WRAPPERS:
        path = Path(plugin_root) / "bin" / name
        if not (path.is_file() and os.access(str(path), os.X_OK)):
            bad.append(name)
    return bad


def agent_memory_problems(cwd):
    """Folders under `<repo>/.claude/agent-memory/` that are named like a roster agent without the
    `anthology-` prefix (`implementer`, `anthology:implementer`, `anthology_implementer`): Claude
    Code may not be mapping `anthology:<agent>` to `anthology-<agent>/`. Roster names are generic,
    so a user's own agent can own such a folder; `bad` is therefore only filled when no
    `anthology-<agent>/` folder exists for any roster agent. Returns (bad, good) as name lists;
    both are empty when there is no such folder or no repo."""
    import memory_sync
    top = repo_top(cwd)
    if top is None:
        return [], []
    try:
        names = sorted(p.name for p in (top / ".claude" / "agent-memory").iterdir() if p.is_dir())
    except OSError:
        return [], []
    bad, good = [], []
    for name in names:
        for agent in memory_sync.ROSTER:
            if name in (agent, "anthology:" + agent, "anthology_" + agent):
                bad.append(name)
            elif name == "anthology-" + agent:
                good.append(name)
    if good:
        return [], good
    return bad, good


def agent_memory_hint(name):
    """The soft warning for a bare roster-name folder (shared by doctor and session start)."""
    return ("`.claude/agent-memory/%s/` exists but no `anthology-%s/` does: either Claude Code changed "
            "the agent-memory path or this is an agent of your own named %s; if anthology agents have "
            "run here, report it" % (name, name, name))


def _env_has_model(data):
    block = data.get("env") if isinstance(data, dict) else None
    value = block.get(SUBAGENT_MODEL) if isinstance(block, dict) else None
    return isinstance(value, str) and bool(value.strip())


def subagent_model_source(env, cwd):
    """Where `CLAUDE_CODE_SUBAGENT_MODEL` is set (never its value), or None: "the process
    environment", "user settings.json", "the project's .claude/settings.json" or
    ".claude/settings.local.json"."""
    if str(env.get(SUBAGENT_MODEL) or "").strip():
        return "the process environment"
    candidates = [("user settings.json", config_dir(env) / "settings.json")]
    top = repo_top(cwd) or Path(str(cwd))
    candidates.append(("the project's .claude/settings.json", top / ".claude" / "settings.json"))
    candidates.append(("the project's .claude/settings.local.json", top / ".claude" / "settings.local.json"))
    for label, path in candidates:
        # user settings are the owner's; the project's two files come with the clone, so they are bounded
        if _env_has_model(read_json(path, None if label == "user settings.json" else REPO_READ_LIMIT)):
            return label
    return None
