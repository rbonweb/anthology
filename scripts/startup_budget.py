#!/usr/bin/env python3
"""Per-agent startup budget: how many bytes each agent reads before it starts work.

Usage: startup_budget.py [--root PATH] [AGENT ...]

An agent's startup size is its body, plus every `${CLAUDE_PLUGIN_ROOT}/knowledge/.../SKILL.md`
listed under its `## Startup` heading, plus every preloaded `skills:` SKILL.md from the
anthology-superpowers plugin. Each agent has a byte budget (the size measured when the roster
landed, times 1.15, rounded up), so upstream growth shows up in a sync PR instead of silently
raising every call's cost.

This module is the single home of that calculation. `tests/test_roster.py` enforces the budgets
and `scripts/upstream_watch.py` uses it to put the startup-budget delta into upstream issues. The
CLI prints each agent's size against its budget and exits 1 when one is over; after a
`vendor.py sync` it is the command that gives the size after the sync.

To raise a budget: run the CLI (or `python -m unittest test_roster -k budget` from tests/), take
ceil(measured * 1.15) from the output, set it in BUDGET below and explain the growth in the same
commit.

Standard library only; runs on Python 3.9+.
"""
from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path
from typing import List, Optional, Sequence

ANTHOLOGY = "plugins/anthology"
SUPERPOWERS = "plugins/anthology-superpowers"
STARTUP_PATH = re.compile(r"\$\{CLAUDE_PLUGIN_ROOT\}/(knowledge/[A-Za-z0-9_./-]+/SKILL\.md)")
SAFE_AGENT_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*$")

# Startup budget in bytes: body + every Startup file + every preloaded SKILL.md, per agent.
# Each value is the size measured when the roster landed, times 1.15, rounded up. Growth (an
# upstream sync that makes a knowledge file bigger, a longer body) fails the test on purpose.
BUDGET = {
    "researcher": 89267,  # measured 77623
    "architect": 23612,  # measured 20532
    "implementer": 21607,  # measured 18788
    "debugger": 17388,  # measured 15120
    "reviewer": 12553,  # measured 10915
    "scout": 5944,  # measured 5168
    "docs-writer": 31376,  # measured 27283
    "ui-designer": 128097,  # measured 111388
    "ui-reviewer": 65986,  # measured 57379
    "product-planner": 102779,  # measured 89373
    "marketer": 54384,  # measured 47290
}


def suggested_budget(measured: int) -> int:
    """ceil(measured * 1.15), in integer arithmetic."""
    return -(-measured * 115 // 100)


def sections(body: str):
    """Ordered list of `## ` heading names, and a dict name -> text."""
    parts = re.split(r"^## (.+)$", body, flags=re.MULTILINE)
    names = [p.strip() for p in parts[1::2]]
    return names, dict(zip(names, parts[2::2]))


def split_agent(path) -> tuple:
    """(preloaded skill names, body) of an agent file; stdlib only, so no YAML parser.

    Only the `skills:` list of the frontmatter is read; entries look like
    `anthology-superpowers:<skill>`.
    """
    text = Path(path).read_text(encoding="utf-8")
    if not text.startswith("---\n"):
        raise ValueError(f"{path}: missing frontmatter")
    head, sep, body = text[4:].partition("\n---")
    if not sep:
        raise ValueError(f"{path}: unterminated frontmatter")
    skills: List[str] = []
    in_list = False
    for line in head.splitlines():
        if re.match(r"^skills:\s*$", line):
            in_list = True
        elif in_list:
            m = re.match(r"^\s+-\s+(\S+)\s*$", line)
            if m:
                skills.append(m.group(1))
            else:
                in_list = False
    return skills, body.lstrip("\n")


def agent_names(root) -> List[str]:
    """Agent names (file stems), sorted; names that are not plain identifiers are skipped."""
    folder = Path(root) / ANTHOLOGY / "agents"
    return sorted(p.stem for p in folder.glob("*.md") if SAFE_AGENT_RE.fullmatch(p.stem))


def _parts(root, name):
    skills, body = split_agent(Path(root) / ANTHOLOGY / "agents" / f"{name}.md")
    startup = sections(body)[1].get("Startup", "")
    return skills, body, STARTUP_PATH.findall(startup)


def startup_files(root, name: str) -> List[str]:
    """The knowledge files in the agent's Startup section, relative to the anthology plugin."""
    return _parts(root, name)[2]


def startup_paths(root, name: str) -> List[str]:
    """Every file counted for the agent, relative to the repo root (Startup files, then preloads)."""
    skills, _, files = _parts(root, name)
    paths = [f"{ANTHOLOGY}/{rel}" for rel in files]
    for skill in skills:
        paths.append(f"{SUPERPOWERS}/skills/{skill.split(':', 1)[-1]}/SKILL.md")
    return paths


def startup_bytes(root, name: str) -> int:
    """Body + every Startup file + every preloaded SKILL.md, in bytes."""
    _, body, _ = _parts(root, name)
    total = len(body.encode("utf-8"))
    for rel in startup_paths(root, name):
        total += (Path(root) / rel).stat().st_size
    return total


def agents_reading(root, target: str) -> List[str]:
    """Agents with a startup file or preload at or under `target` (a repo-relative folder)."""
    target = target.strip("/")
    if not target:
        return []
    found = []
    for name in agent_names(root):
        if any(p == target or p.startswith(target + "/") for p in startup_paths(root, name)):
            found.append(name)
    return found


def report(root, names: Optional[Sequence[str]] = None) -> List[dict]:
    """One record per agent: name, measured bytes, budget (None when unset)."""
    return [{"agent": n, "measured": startup_bytes(root, n), "budget": BUDGET.get(n)}
            for n in (names if names is not None else agent_names(root))]


def main(argv: Optional[Sequence[str]] = None, stdout=None) -> int:
    parser = argparse.ArgumentParser(description="Print each agent's startup size against its budget.")
    parser.add_argument("agents", nargs="*", help="agent names (default: all)")
    parser.add_argument("--root", default=str(Path(__file__).resolve().parent.parent),
                        help="repository root (default: this checkout)")
    args = parser.parse_args(argv)
    out = stdout or sys.stdout
    known = agent_names(args.root)
    unknown = [a for a in args.agents if a not in known]
    if unknown:
        print("unknown agent(s): " + ", ".join(unknown) + "; known: " + ", ".join(known), file=out)
        return 2
    bad = 0
    for rec in report(args.root, args.agents or None):
        measured, budget = rec["measured"], rec["budget"]
        if budget is None:
            status = "no budget set"
            bad += 1
        elif measured > budget:
            status = f"OVER; if intended set BUDGET to {suggested_budget(measured)}"
            bad += 1
        else:
            status = "ok"
        print(f"{rec['agent']}: {measured} bytes of {budget} budget ({status})", file=out)
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
