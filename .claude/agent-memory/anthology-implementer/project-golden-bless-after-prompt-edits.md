---
name: 'Agent prompt edits need a startup-budget golden bless'
description: 'Editing agent prompts drifts dev.startup-budget, and dev.upstream-watch when the scout budget moves; how to check and bless'
metadata:
  type: project
  modified: 2026-11-28
---

Changing any plugins/anthology/agents/*.md file changes the byte counts that anthology-dev startup-budget prints, so the dev.startup-budget golden drifts. Raising a BUDGET row (scout, reviewer...) also drifts dev.upstream-watch: its main-op cases print the real BUDGET table from crates/anthology-dev/src/startup_budget.rs, even though the surface file crates/difftest/src/surfaces/dev_upstream_watch.rs pins its own table. The routing guide edit alone did not drift hook.session-start. Changing how startup bytes are counted (A1 conditional reads) moved one dev.startup-budget case and left dev.upstream-watch unchanged.
Check with scripts/dev.sh golden check --surface dev.startup-budget,dev.upstream-watch (one comma list; two --surface flags are refused; about 3 minutes, run it in the background and wait with timeout 170 docker wait on the container that mounts the worktree, found with docker ps --filter volume=<worktree path>). Then scripts/dev.sh golden bless --surface dev.startup-budget,dev.upstream-watch. Run the bless after the last prompt edit, or it records stale numbers.
The budget table is in crates/anthology-dev/src/startup_budget.rs, pinned verbatim in crates/anthology-dev/tests/startup_budget.rs; crates/repo-tests/tests/roster.rs reads it from anthology_dev. Roster tests also pin the Shell and Do wording, so a prompt wording change needs their needles updated.
In the container, scripts/rust-test.sh test -p repo-tests -p anthology-dev --no-fail-fast runs everything; without --no-fail-fast cargo stops at the first failing test binary.
Why: avoids a red CI difftest job on a prompt-only change.
