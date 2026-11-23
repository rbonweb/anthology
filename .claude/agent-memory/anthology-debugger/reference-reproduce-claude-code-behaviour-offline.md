---
name: 'Reproducing Claude Code worktree and sandbox behaviour without API calls'
description: 'How to test claude --worktree, the Linux scrub and Bash tool results locally with no model call'
metadata:
  type: reference
  modified: 2026-11-22
---

Worktree creation happens at startup before any model call: run claude -p hi --worktree NAME in a throwaway repo with ANTHROPIC_BASE_URL pointing at a dead local port and a bogus key; the error text (or the created worktree in git worktree list) is the result. For Linux behaviour (bubblewrap, CLAUDE_CODE_SUBPROCESS_ENV_SCRUB=1) use a node:22-bookworm container with bubblewrap installed and docker run options seccomp=unconfined, apparmor=unconfined, systempaths=unconfined and cap-add SYS_ADMIN; install the pinned version with npm install --prefix into a scratch folder in the worktree and delete it afterwards.
To see what a Bash tool call returns, run a tiny local node HTTP server that speaks the Messages SSE stream: first reply a tool_use for Bash, then text; log the tool_result it receives.
**Why:** the CI eval artifacts cut the failure reasons to about 80 characters and re-running evals costs API credits.
**How to apply:** a claude -p run with a failed repo state leaves files behind (empty .env and similar mask files, .git/commondir), so use a fresh repo for every attempt. The hook that guards worktree-isolated agents blocks a command that sets HOME; put the setup in a script file instead.
