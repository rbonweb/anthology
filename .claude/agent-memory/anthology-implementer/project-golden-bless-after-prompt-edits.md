---
name: 'Agent prompt edits need a startup-budget golden bless'
description: 'Editing agent prompts drifts only the dev.startup-budget golden; how to check and bless it'
metadata:
  type: project
  modified: 2026-11-22
---

Changing any plugins/anthology/agents/*.md file changes the byte counts that anthology-dev startup-budget prints, so the dev.startup-budget golden drifts. The routing guide edit alone did not drift hook.session-start.
Check with scripts/dev.sh golden check --all --show 3 (it takes several minutes; run it in the background), then scripts/dev.sh golden bless --surface dev.startup-budget. Run the bless after the last prompt edit, or it records stale numbers.
In the container, scripts/rust-test.sh run cargo test --locked -p repo-tests -p anthology-dev -p anthology-runtime --test roster --test startup_budget --test session_start works; cargo accepts only one positional test-name filter, so filter by --test instead.
Why: avoids a red CI difftest job on a prompt-only change.
