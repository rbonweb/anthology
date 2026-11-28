---
name: 'Guide budget and targeted test runs'
description: 'Routing guide cap (see the routing-guide note for the number); how to run the roster, budget and guide tests in Docker'
metadata:
  type: project
  modified: 2026-11-28
---

The routing guide (plugins/anthology/templates/routing-guide.md) is size-tested in crates/anthology-runtime/tests/session_start.rs (guide_test); the cap and current size are in the routing-guide size budget note. Added lines must be paid for by trimming glosses elsewhere; rule text pinned by tests (Lead line, Inline only line, browser lines, memory commit line) must stay verbatim.
Writer rules live in each of 4 agent prompts under ## Do and are asserted in crates/repo-tests/tests/roster.rs with each_agent(|s| s.edits == "yes", ...). Agent bodies are capped at 60 lines. The startup BUDGET table lives only in crates/anthology-dev/src/startup_budget.rs (pinned verbatim in its tests/startup_budget.rs; roster.rs reads it from anthology_dev).
Targeted run: one container, cargo test -p repo-tests -p anthology-dev -p anthology-runtime --test roster --test startup_budget --test session_start, with the worktree mounted and CARGO_TARGET_DIR=/w/target (ignored by git). The rust CI image has no rustfmt; run rustup component add rustfmt inside the container before cargo fmt --check. scripts/rust-test.sh run has rustfmt and clippy already.
Why: saves a full-suite run and avoids a surprise over-budget failure in CI.
