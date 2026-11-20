"""A tiny agent repo for the startup-budget tests (stdlib only)."""
from pathlib import Path

AGENT = """---
name: {name}
description: "Use when testing."
model: haiku
{skills}---

## Startup
Before any other work, Read every file below, in order:
{files}

## Role
Test agent.
"""


def write(root: Path, rel: str, text: str) -> Path:
    path = root / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")
    return path


def make_repo(root: Path) -> None:
    """Three agents: `reader` (two Startup files), `skilled` (one file, one preload), `bare`."""
    write(root, "plugins/anthology/knowledge/alpha/SKILL.md", "a" * 100)
    write(root, "plugins/anthology/knowledge/beta/SKILL.md", "b" * 200)
    write(root, "plugins/anthology-superpowers/skills/plan/SKILL.md", "p" * 400)
    line = "- `${CLAUDE_PLUGIN_ROOT}/knowledge/%s/SKILL.md`"
    agents = "plugins/anthology/agents/"
    write(root, agents + "reader.md", AGENT.format(
        name="reader", skills="", files=line % "alpha" + "\n" + line % "beta"))
    write(root, agents + "skilled.md", AGENT.format(
        name="skilled", skills="skills:\n  - anthology-superpowers:plan\n", files=line % "beta"))
    write(root, agents + "bare.md", AGENT.format(name="bare", skills="", files=""))
