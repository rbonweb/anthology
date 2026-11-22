---
name: 'Evals on Linux runners need bubblewrap and socat'
description: 'Why the first CI eval run failed all cases and which smokes depend on the Claude Code sandbox'
metadata:
  type: project
  modified: 2026-11-22
---

The evals workflow runs on ubuntu-latest (ubuntu-26.04 image in Nov 2026). Claude Code on Linux needs bubblewrap and socat for its Bash sandbox; they are absent on the runner, so plugin eval refuses every case with a shell grant and every smoke that runs Bash without bypassPermissions fails (missing-interpreter, rtk-rewrite, agent-memory-path, live-guard). Smokes that use bypassPermissions pass.
**Why:** the Python-era evals were only run on macOS (seatbelt, no extra packages), so the first Linux run exposed it.
**How to apply:** if an eval smoke fails with "Sandbox is required but failed to initialize", check the workflow installs bubblewrap and socat and that unprivileged user namespaces work (AppArmor restrict_unprivileged_userns). The smoke.json reasons are cut to 80-160 characters, so the full error of agent-start failures (model-check, knowledge-read-writers, worktree-base) is not in the artifact.
