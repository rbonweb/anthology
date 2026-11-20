#!/usr/bin/env python3
"""PreToolUse hook entry point: read the hook input from stdin, block with exit 2 or allow.

Exit 2 prints the reason on stderr (Claude Code feeds it back to the agent). Anything else,
including malformed input and internal errors, exits 0: a guard bug must never brick a session.
"""
from __future__ import annotations

import json
import os
import sys


def decide(raw):
    """Return (rule_id, message) when the hook input must be blocked, else None."""
    data = json.loads(raw.decode("utf-8", errors="replace"))
    if not isinstance(data, dict) or not isinstance(data.get("tool_name"), str):
        return None
    sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
    import guard_rules
    home = os.environ.get("HOME") or os.path.expanduser("~")
    return guard_rules.check(data["tool_name"], data.get("tool_input"), home)


def main():
    try:
        hit = decide(sys.stdin.buffer.read())
    except Exception:
        return 0
    if hit is None:
        return 0
    rule, message = hit
    sys.stderr.write(f"Blocked by anthology-guardrails ({rule}): {message}\n")
    return 2


if __name__ == "__main__":
    sys.exit(main())
