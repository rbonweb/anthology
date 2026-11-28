---
name: 'Agent prompt edits must fit the startup budget and the 60-line body cap'
description: 'A line added to every agent prompt overran budgets and the ui-designer 60-line cap twice; fix recipe and where the budget table lives'
metadata:
  type: project
  modified: 2026-11-28
---

A shared line added to the Shell section of all 11 agent prompts pushes some agents over budget and makes ui-designer 61 body lines against the cap of 60 in roster.rs test_body_shape. 2026-11-27 (126 bytes): architect, debugger, docs-writer and scout went over; ui-designer needed two Do lines merged. 2026-11-28 (credential path rule, 393 bytes): architect, debugger and scout went over again; ui-designer was at exactly 60 lines.
Fix without touching anything outside the Shell section (another branch edits ui-designer elsewhere): merge two Shell lines in all 11 agents (Durations come only from timestamps you read, plus A report names a settings key) so the new rule adds no line, then raise only the BUDGET rows that scripts/dev.sh startup-budget reports as OVER, by the added bytes (git cat-file -s old versus wc -c new), with the reason in the row comment. That keeps the old spare bytes (debugger 1, scout 3).
The budget table is in three files: crates/anthology-dev/src/startup_budget.rs, crates/anthology-dev/tests/startup_budget.rs, crates/repo-tests/tests/roster.rs. crates/difftest/src/surfaces/dev_upstream_watch.rs pins its own table; leave it, bless instead. rustfmt drops the column alignment of trailing comments in these tables when a comment is long, so run rustfmt on the three files (container, rustup component add rustfmt first) or CI fmt goes red. Roster needles for the Shell wording are in every_agent_has_the_same_shell_section: add one for a new line. Run scripts/dev.sh startup-budget, then bless dev.startup-budget and dev.upstream-watch.
Why: headroom per agent is tiny after earlier Shell additions, so any shared wording change needs a budget decision and a line count check.
