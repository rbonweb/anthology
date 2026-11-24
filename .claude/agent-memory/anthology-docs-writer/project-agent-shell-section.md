---
name: 'Agents share one byte-identical Shell section'
description: 'plugins/anthology/agents/*.md carry the same ## Shell section after ## Do; roster.rs pins it; edit all 11 together'
metadata:
  type: project
  modified: 2026-11-24
---

Every agent file has a ## Shell section (after ## Do) holding the shell, waiting and report rules. crates/repo-tests/tests/roster.rs test every_agent_has_the_same_shell_section requires all 11 to be byte-identical, so change it in all files at once. Writers keep only writer-specific shell rules in Do. Body limit is 60 lines (ui-designer sits at 60). The section added about 1050 bytes per agent: reviewer and scout budgets were raised by that amount in crates/anthology-dev/src/startup_budget.rs, crates/anthology-dev/tests/startup_budget.rs and roster.rs; other agents fit. Why: a shared block cannot drift between agents. scripts/rust-test.sh test runs the whole workspace twice; use scripts/rust-test.sh run cargo test --locked -p repo-tests --test roster instead.
