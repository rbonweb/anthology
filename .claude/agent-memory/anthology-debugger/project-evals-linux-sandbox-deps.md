---
name: 'Evals on Linux: bubblewrap, and the subprocess scrub breaks worktrees'
description: 'Why Linux eval smokes failed: missing bubblewrap first, then CLAUDE_CODE_SUBPROCESS_ENV_SCRUB=1 isolating Claude Code in bwrap'
metadata:
  type: project
  modified: 2026-11-22
---

The evals workflow runs on ubuntu-latest (ubuntu-26.04 in Nov 2026). Claude Code on Linux needs bubblewrap and socat; the workflow now installs them and lifts the AppArmor userns restriction. Smokes that use bypassPermissions pass either way.
Second, separate cause: the smoke harness sets CLAUDE_CODE_SUBPROCESS_ENV_SCRUB=1 by default (build_env). On macOS that only strips env; on Linux Claude Code 2.1.329 treats it as subprocess isolation (bwrap). Then (a) an isolation: worktree agent cannot start: "Failed to resolve base branch ..." (repo .git "could not be confirmed"), with or without worktree.baseRef=head, and (b) Bash sees the repo with .git and a missing .claude read-only, so a memory note is never written (agent-memory-path). Fix: those smokes pass extra_env no_scrub() like writer-memory-commit and auto-memory-dir.
**Why:** the earlier evals only ran on macOS, so the first Linux runs exposed both. The smoke.json reasons are cut to 80-160 characters, so the full agent-start error is not in the artifact; reproduce it instead.
**How to apply:** a new smoke that makes an agent create a worktree or writes under .claude/ from Bash must set extra_env no_scrub(). worktree.baseRef=head itself is fine in clones, shallow clones and detached HEADs; only a repo with no commits fails, in both baseRef modes.
