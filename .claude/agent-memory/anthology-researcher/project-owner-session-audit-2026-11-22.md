---
name: 'Owner session audit findings (2026-11-22)'
description: 'Kinds of Anthology gaps seen in a 2-day multi-agent build session in another project, with counts'
metadata:
  type: project
  modified: 2026-11-22
---

Audit of the owner's build session in another project: about 200 agent dispatches over 2 days, on plugin 0.1.0 and then 0.2.1.
Top gaps found, for checking whether later releases fixed them:
- Worktree-isolated agents had about 195 Bash calls refused as too complex to verify (heredoc python/cat edits, compound git). 76 agents hit it, and agents that saved a note about it kept hitting it. Agent prompts need a plain-commands rule.
- docs-writer forces isolation: worktree, so it fails when the session cwd is not a git repo (a parent folder holding several repos); the lead fell back to general-purpose agents without Anthology rules.
- SessionStart:compact fires inside subagents and injects the lead routing guide plus a false guardrails-not-installed warning (0.1.0 check, cwd in a worktree).
- Lead did maker work itself: about 300 inline heredoc/sed edits, contract files, 50 conflict resolutions, foreground test runs hitting the 600 s timeout.
- The owner asked three times for more parallel agents and five times for status.
- Lead memory went to Claude auto-memory of the non-git parent folder, never to repo .claude/memory.
Why: lets the next audit compare against a baseline instead of starting over.
