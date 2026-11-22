---
name: 'Guide budget and targeted test runs'
description: 'Routing guide is capped at 3800 chars; how to run the roster, budget and guide tests in Docker'
metadata:
  type: project
  modified: 2026-11-22
---

The routing guide (plugins/anthology/templates/routing-guide.md) is tested at 3800 characters in crates/anthology-runtime/tests/session_start.rs (guide_test). Added lines must be paid for by trimming glosses elsewhere; rule text pinned by tests (Lead line, Inline only line, browser lines, memory commit line) must stay verbatim.
Writer rules live in each of 4 agent prompts under ## Do and are asserted in crates/repo-tests/tests/roster.rs with each_agent(|s| s.edits == "yes", ...). Agent bodies are capped at 60 lines and startup budgets are pinned in two places (roster.rs and crates/anthology-dev/src/startup_budget.rs).
Targeted run: one container, cargo test -p repo-tests -p anthology-dev -p anthology-runtime --test roster --test startup_budget --test session_start, with the worktree mounted and CARGO_TARGET_DIR=/w/target (ignored by git). The rust CI image has no rustfmt; run rustup component add rustfmt inside the container before cargo fmt --check.
Why: saves a full-suite run and avoids a surprise over-budget failure in CI.
