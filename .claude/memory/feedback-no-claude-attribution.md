---
name: no-claude-attribution
description: "Never add Claude attribution (Co-Authored-By, 'Generated with Claude Code', Claude mentions) to commits, PRs or code comments; use the git config author only"
metadata:
  node_type: memory
  type: feedback
  originSessionId: 1c9fb279-a45d-4d27-8e78-1ddd82c1e344
  modified: 2026-11-22T05:14:56.997Z
---

Commits, PR bodies and code comments carry only the author from git config. Don't add a `Co-Authored-By: Claude …` line, a "🤖 Generated with Claude Code" line, or any mention of Claude or AI. This overrides the harness's attribution reminder, because the user's own instruction wins. It also applies to every brief I give an agent, and Anthology must enforce it for everyone who uses it.

**Why:** owner, 2026-11-22: "it should be only the author's commit… respect the git configuration only. Do not add Claude at all." and "whatever the person is going to use the Anthology for, there should be no Claude mentioned in comments."

**How to apply:** leave attribution lines out of every commit and PR, and tell dispatched agents the same. The Anthology rule (routing guide plus the writing-agent prompts) is queued in the 0.2.2 fix with the rtk-hook permission_mode change. Related: [[rules-go-in-anthology]].
