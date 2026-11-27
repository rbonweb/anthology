---
name: 'no-cli-owned-open-items'
description: 'Don''t list Claude Code CLI matters (its version, its cache cost) as open owner decisions; the owner keeps the CLI current'
metadata:
  type: feedback
  modified: 2026-11-26
---

The owner, 2026-11-26: the terminal claude is up to date, and the 1-hour cache cost belongs to the CLI, not Anthology. Don't carry these as open items.

**Why:** they are not Anthology decisions, and repeating them in every status wastes the owner's attention.

**How to apply:** status reports list only Anthology work and decisions. If a CLI version matters for a release step, use whichever binary works (release.sh needs CLAUDE_BIN set to a current VS Code extension binary) without asking the owner. The field check (the other-project scan) runs only when the owner asks.
