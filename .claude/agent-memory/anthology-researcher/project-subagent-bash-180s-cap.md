---
name: 'Async subagent Bash 180 s foreground cap'
description: 'Why subagent foreground Bash moves to background at 180 s, what lifts it, and why worktree agents call repo memory missing'
metadata:
  type: project
  modified: 2026-11-24
---

Measured 2026-11-24 from the binaries of Claude Code 2.1.323, 2.1.325 and 2.1.330 (same code in all three) and a live run in a 2.1.331 session.
An async subagent (Agent calls run async) has its foreground Bash and PowerShell timeout clamped to 180000 ms; the tool result then reads "moved to the background (ID ...) after running for 180s". The cap is skipped when the subagent prompt cache TTL is 1h or off, for the main session, for hook or inner calls, and in single-shot -p runs.
The subagent TTL comes from, in order: FORCE_PROMPT_CACHING_5M, the CLAUDE_CODE_SUBAGENT_PROMPT_CACHE_TTL env var, the subagentPromptCacheTtl setting, agent frontmatter experimental.cacheTtl, then ENABLE_PROMPT_CACHING_1H. With none set it is 5m, so the cap applies even on a subscription. The main-session equivalent is CLAUDE_CODE_AUTO_BACKGROUND_TIMEOUT_MS (floor 2000 ms).
A command whose first word is sleep is never auto-backgrounded: "sleep 1; <wait>" ran 200 s in the foreground. Standalone sleep of 25 s or more is refused.
Worktree "missing memory" reports: the Read tool reads .claude/memory in worktrees fine. The agents never read the map; one read its MEMORY.md under the home .claude folder. The afterwork mod accepts "map missing" as dealing with the map.
Why: the waiting and project-map rules in the agent prompts depend on this. How to apply: re-check the 180000 constant and the cIn TTL order when the CI pin moves.
