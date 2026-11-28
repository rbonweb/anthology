---
name: 'Agent prompt edits must fit the startup budget and the 60-line body cap'
description: 'A line added to every agent prompt overran four budgets and the ui-designer line cap; where the budget table lives'
metadata:
  type: project
  modified: 2026-11-28
---

A shared line added to the Shell section of all 11 agent prompts (126 bytes) pushed architect, debugger, docs-writer and scout over budget (debugger had 1 byte spare, scout 3) and made ui-designer 61 body lines against the cap of 60 in roster.rs test_body_shape. Fix: raise those four BUDGET rows by the added bytes (precedent: a note such as +126 atl in the row comment) and merge two lines in ui-designer (the build-and-check line and the fix-and-hand-back line).
The budget table lives in crates/anthology-dev/src/startup_budget.rs and is pinned verbatim in crates/anthology-dev/tests/startup_budget.rs (the_budget_table_is_gc13_verbatim). Since copy-and-marketing A1, crates/repo-tests/tests/roster.rs has no copy: its budget module wraps anthology_dev::startup_budget. A conditional Startup line (ending in ", only when|for|to ...") adds only the largest of its files, so a new conditional read costs nothing until it is the largest. crates/difftest/src/surfaces/dev_upstream_watch.rs pins its own table; leave it, bless instead. Run scripts/dev.sh startup-budget before committing a prompt change, then bless dev.startup-budget and dev.upstream-watch.
Why: headroom per agent is tiny after earlier Shell additions, so any shared wording change needs a budget decision and a line count check.
