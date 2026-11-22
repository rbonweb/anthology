---
name: 'After-work mod: agent.report has no worktree path'
description: 'Where the after-work check gets the agent folder, what raises a block, and how mods tests and tsc run'
metadata:
  type: project
  modified: 2026-11-22
---

plugins/anthology/hooks/mods/afterwork.ts checks writer reports on agent.report. AgentReportInput carries agentId, parentAgentId, subagentType, provider, status, text and no path, so the one git status uses $.agent.list() cwd for the agentId, skipped when its realpath ($.fs.stat resolve) equals the session cwd or root. Why: the lead cannot guess a worktree from report text, and the session folder holds the lead own changes. A block comes only from real gaps (commit, test evidence, attribution, uncommitted files); the project map and leftovers are reminders that ride in a block and never raise one, because a review found good reports got a block every time. The git pathspec is :(top) plus :(top,exclude).claude/agent-memory so a subfolder cwd still sees the whole tree and agent notes are ignored (checked by hand in a scratch repo). Tests: sh ci/jobs/mods.sh runs validate and plugin test in a node container, sh ci/jobs/mods.sh tsc runs tsc; each takes 2 to 4 min, mostly npm ci. In plugin tests a second on() for the same event does not reliably override the first, so give the world helper function fields instead of re-registering. The test of keeping core review and warning passed even against a hook returning only text, so it cannot prove the spread.
