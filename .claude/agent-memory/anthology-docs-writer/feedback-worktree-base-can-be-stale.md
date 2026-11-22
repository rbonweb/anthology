---
name: 'Fetch origin build first'
description: 'The agent worktree can branch from an old build commit; fetch and fast-forward before editing'
metadata:
  type: feedback
  modified: 2026-11-22
---

At the start of a task, run git fetch origin build and git merge --ff-only FETCH_HEAD when the task refers to a recent PR.
**Why:** the worktree started 20+ commits behind build, so the routing guide text the task quoted was not there yet.
**How to apply:** check with grep that the quoted text exists before editing; if not, fetch.
