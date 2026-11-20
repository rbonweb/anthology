"""Anthology's small per-user state file (Task 11 interface).

`state.json` lives at `${CLAUDE_CONFIG_DIR:-~/.claude}/anthology/state.json` and holds
values other scripts look up at run time (for example `data_dir` and `rtk_path`).
"""
from __future__ import annotations

import json
import os
import tempfile
from pathlib import Path


def state_path() -> Path:
    base = os.environ.get("CLAUDE_CONFIG_DIR") or os.path.join(os.path.expanduser("~"), ".claude")
    return Path(base) / "anthology" / "state.json"


def read() -> dict:
    """Return the state as a dict; a missing or unreadable file reads as empty."""
    try:
        data = json.loads(state_path().read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    return data if isinstance(data, dict) else {}


def write(**kv) -> dict:
    """Merge `kv` into the state and replace the file atomically. Returns the new state."""
    path = state_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    data = read()
    data.update(kv)
    # ensure_ascii=False keeps non-ASCII paths literal so bin/rtk (sed, no python) can read them.
    # A lone surrogate (a non-UTF-8 byte in a path) cannot be encoded that way: escape instead.
    text = json.dumps(data, indent=2, sort_keys=True, ensure_ascii=False) + "\n"
    try:
        text.encode("utf-8")
    except UnicodeEncodeError:
        text = json.dumps(data, indent=2, sort_keys=True, ensure_ascii=True) + "\n"
    fd, tmp = tempfile.mkstemp(dir=str(path.parent), prefix=".state.", suffix=".tmp")
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            fh.write(text)
        os.replace(tmp, str(path))
    except BaseException:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise
    return data
