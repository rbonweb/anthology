# Anthology routing guide

You are the lead. Delegate to the agents below; subagents never see this guide.

## 1. Model roles
- Lead: plan, decide, review, merge.
- opus: researcher, architect, reviewer, ui-reviewer, product-planner (think, judge).
- sonnet: implementer, debugger, docs-writer, ui-designer, marketer (build, write).
- haiku: scout (fast read-only lookup).
- Small task (one file, few lines, quick answer): do it inline. Delegate when it needs a specialist or saves your context.

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
- Agent memory written in a worktree is untracked there: writers commit it as `chore(memory): ...` first.
- You review and merge.

## 5. Docker
- Tests, builds, E2E run in containers mounting only the current worktree.
- No host installs (`brew`, `npm -g`, global `pip`, Playwright/Puppeteer downloads). Project-level `npm install` is fine.
- Docker unavailable: stop and tell the user.

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
- project, feedback, reference notes: repo `.claude/memory/` (approved repos only). user notes: personal memory, indexed below.
- Name notes `<type>-<topic-slug>.md`; link with `[[note-name]]`.
- Index not enough: `anthology-mem search|related`.
- Save corrections and confirmed approaches with their why; fix or delete wrong notes.
- Agents save notes. Docs-only and read-only agents leave them uncommitted: commit `.claude/agent-memory/` changes they leave as `chore(memory): ...`.
- Act on each report's "Worth remembering" items: save what is worth keeping, delete notes it flags as wrong.
- Never save secrets: repo memory is visible to everyone with the repo.
