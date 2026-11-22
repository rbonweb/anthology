---
name: 'Merging several PRs into one release branch'
description: 'Conflict spots when PRs that touch every agent prompt, goldens and versions land together, and how each resolves'
metadata:
  type: project
  modified: 2026-11-22
---

Agent prompt Memory sections conflict in every agent when one PR rewords the save line and another adds the open-it-with-Read phrase: keep the Read phrase on the first line and the reworded second line. Binary goldens (dev.startup-budget, hook.session-start): take --ours, then scripts/dev.sh golden bless --surface <name> on the merged tree. Version-only conflicts in Cargo.toml, Cargo.lock and fuzz/Cargo.lock: check both sides only changed version lines, then take --theirs for all three. Agent MEMORY.md indexes auto-merge as plain added lines; compare entry counts with the note files, and hand-write the file only from lines the generator already produced. Why: one conflict pass costs far less than re-running CI per PR. To name containers per worktree without editing scripts/rust-test.sh, put a docker shim that adds --name after run first on PATH under target/.
