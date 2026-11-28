---
name: 'Facts that shaped the 2026-11-28 phase 2 safety plan'
description: 'session.append rewrites tool results; a Bash tool.call or tool.run hook stops backgrounding; privacy scan lives in anthology-memory; event owners in mods/index.ts'
metadata:
  type: project
  modified: 2026-11-28
---

Why: these decided the design of docs/superpowers/plans/2026-11-28-phase2-safety.md (secret masking, commit privacy check, parallelism nudge, ui.dummy-contact) and are not obvious from one file.
- Claude Code 2.1.332 mod API: session.append with matcher door tool-result rewrites a tool_result content before it is stored and sent to the model; origin.tool names the tool; it fires for subagent rows too (agentId). A hook that fails leaves the row as made.
- Any tool.call or tool.run hook that may match Bash turns off for Bash the move to the background, live output and Code Mode waits. Never hook Bash there for a feature that only reads or rewrites output.
- $.session.redact(text) is the engine credential masker: best effort, rules unknown, so not usable where false masks matter.
- $.agent.list() gives AgentInfo with status pending, running, waiting, idle, completed, failed or killed.
- One module may register each event once. Owners in plugins/anthology/hooks/mods: tool.check permissions.ts; tool.call (Edit, Write, NotebookEdit), turn.start and prompt.compose delegation.ts; session.start, agent.spawn, command.run and ui.render alerts.tsx; agent.report is shared in index.ts.
- The privacy scan (home paths, emails, hostnames, private hosts) is crates/anthology-memory/src/privacy.rs; the guard crate holds only the G6 secret patterns, and anthology-memory depends on anthology-guard, so the guard cannot call the scan. Its generic home-path check flags any home folder name (container users too) and its private-host suffix list flags shared container and cluster DNS names, which is too broad for a commit block; a commit check should match only the user own home, handle and machine name.
- Personal notes live in the anthology home personal-memory folder; anthology-mem save refuses type user. On 2026-11-28 no banned-names note existed.
- The SDD ledger is .superpowers/sdd/<plan>/progress.md; line 1 names the plan, done tasks have a line "Task N: complete".
