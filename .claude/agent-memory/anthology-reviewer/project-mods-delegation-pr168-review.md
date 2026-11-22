---
name: 'Strict-delegation mod (PR 168) review findings'
description: 'Gaps found in hooks/mods/delegation.ts at head 1251f5cd and how the mods tests were run in Docker'
metadata:
  type: project
  modified: 2026-11-22
---

The lead-note exemption in plugins/anthology/hooks/mods/delegation.ts covers only ~/.claude/projects/*/memory, ~/.claude/plans, root/.superpowers and root/docs/superpowers/plans. In anthology-approved repos the lead memory is root/.claude/memory/ (autoMemoryDirectory, auto memory off) and personal memory is ~/.claude/anthology/personal-memory/, so both are denied (probed). Relative and ~ paths are denied even into an exempt folder. A terminal-pane teammate is a main loop with no agentId and gets denied, while compose skips the teammate trait. The deny never checks that the Agent tool is offered ($.tool.list), so a lead without it is told to dispatch and cannot. Removing all three .catch lines left 47 of 47 mods tests green: fail-open is untested.
Why: these are the gaps a later mod (afterwork, alerts) copying the same exemption or fail-open pattern would inherit.
How to apply: when reviewing any mod that denies on tool.call, check the exemption list against the repo memory layout in README Memory and the routing guide section 8, and mutate the .catch lines.
Run the mods tests: start a node:22-bookworm-slim container with the worktree mounted read-only, npm ci the pinned claude-code from ci/claude-code under /tmp, copy plugins/anthology to /tmp/p, then run claude plugin test /tmp/p (probe tests can be added to the copy, not the worktree). Install takes about 3 minutes.
