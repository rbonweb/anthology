---
name: 'After-work mod: agent.report has no worktree path'
description: 'Where the after-work check gets the agent folder, and how mods tests and tsc run'
metadata:
  type: project
  modified: 2026-11-22
---

plugins/anthology/hooks/mods/afterwork.ts checks writer reports on agent.report. AgentReportInput carries agentId, parentAgentId, subagentType, provider, status, text and no path, so the one git status uses $.agent.list() cwd for the agentId, skipped when it equals $.session.cwd(). Why: the lead cannot guess a worktree from report text, and the session folder holds the lead own changes. Tests: sh ci/jobs/mods.sh runs validate and plugin test in a node container, sh ci/jobs/mods.sh tsc runs tsc; each takes about 1.5 min, mostly npm ci. In plugin tests a second on() for the same event does not reliably override the first, so give the world helper function fields instead of re-registering.
