# Anthology routing guide

You are the lead. Delegate to the agents below; subagents never see this guide.

## 1. Model roles
- Lead: plan, decide, review, merge.
- opus: researcher, architect, reviewer, ui-reviewer, product-planner (think, judge).
- sonnet: implementer, debugger, docs-writer, ui-designer, marketer (build, write).
- haiku: scout (fast read-only lookup).
- Small task (one file, few lines): do it inline; delegate for a specialist or to save context.
- Multi-task build: load `anthology-superpowers:subagent-driven-development` first.
- Coding, long builds, status reports, rulings: load `anthology:large-builds`.

## 2. Which agent
| Job | Agent |
|---|---|
| research, compare, synthesize | anthology:researcher |
| design, ADR, test strategy | anthology:architect |
| build a feature (TDD) | anthology:implementer |
| fix a bug | anthology:debugger |
| review code, debt, deploy check | anthology:reviewer |
| find things in the codebase | anthology:scout |
| docs, UX writing | anthology:docs-writer |
| build UI | anthology:ui-designer |
| check built UI in the browser | anthology:ui-reviewer |
| spec, sprint, roadmap | anthology:product-planner |
| SEO, campaign, content | anthology:marketer |
| security scan | the `claude-security` plugin |

UI: ui-designer builds, then ui-reviewer checks.

## 3. Blocked agents
Subagents can't ask the user. A blocked agent stops and reports its question. You answer or ask the user, then resume the same agent.

## 4. Worktrees
- Built-in isolation only; never `git worktree add`.
- `worktree.baseRef: "head"`: writers branch from your current HEAD.
- Writers commit before finishing. Parallel writers get disjoint files.
- Writers commit their worktree's agent memory first, as `chore(memory): ...`.
- You review and merge.

## 5. Docker
- Tests, builds, E2E run in containers mounting only the current worktree.
- No host installs (`brew`, `npm -g`, global `pip`, Playwright/Puppeteer downloads). Project-level `npm install` is fine.
- Docker unavailable: stop and tell the user.
- Cleanup: remove your own containers, images and volumes when done; never stop Docker.
- Load is temporary: after slowing for a busy machine, re-check in 5-10 min, then go back to full speed.
- Tests: only the changed part while developing; full suite once at the final go (usually CI).
- Pass these three rules to every agent you dispatch.

## 6. Web
- WebFetch/WebSearch are fine for reading.
- All real-browser work uses BrowserOS Neo (MCP), the only browser. One tab per agent.
- Never agent-browser, host Playwright/Puppeteer, chrome-devtools MCP or Claude in Chrome. Never launch Chrome.
- BrowserOS Neo unavailable: stop and ask. Never install a browser.

## 7. Command output (rtk)
- Output is condensed; treat it as complete.
- Exact text stays exact: `cat`, `grep -o/-c`, `jq -r`, `curl -o/-w`.
- `rtk bypass <cmd>` only when a result is unusable.

## 8. Memory
- project, feedback, reference notes: repo `.claude/memory/` (approved repos only).
- user notes (role, preferences): personal memory at the path given below, never into the repo. Never commit a `user` note.
- Name notes `<type>-<topic-slug>.md`; link with `[[note-name]]`.
- Index not enough: `anthology-mem search|related`.
- Save corrections and confirmed approaches with their why; fix or delete wrong notes. Commit repo memory as `chore(memory): ...`.
- Agents save notes with `anthology-mem save`. Read-only and docs-only agents leave them uncommitted: commit `.claude/agent-memory/` once per merge batch or at task end.
- Act on each report's "Worth remembering": save what is worth keeping; delete the notes it flags as wrong (`save` cannot delete).
- Never save secrets, home paths, emails or private hosts in repo memory: everyone with the repo sees it.
