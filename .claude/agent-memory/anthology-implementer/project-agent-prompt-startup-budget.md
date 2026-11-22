---
name: 'Agent prompt edits must fit the startup budget, scout has about 4 bytes spare'
description: 'The per-agent startup budget in anthology-dev is fixed; adding a phrase to every agent prompt overran scout'
metadata:
  type: project
  modified: 2026-11-22
---

Adding the same sentence to all eleven agent prompts overran the scout budget (5940 of 5944 bytes before the edit). The budget table lives in crates/anthology-dev/src/startup_budget.rs, crates/anthology-dev/tests/startup_budget.rs and crates/repo-tests/tests/roster.rs, so raising it means three edits. Check with scripts/dev.sh startup-budget before committing a prompt change.

Why: the scout is the smallest agent and has almost no headroom, so a shared wording change has to skip it or trim it.
