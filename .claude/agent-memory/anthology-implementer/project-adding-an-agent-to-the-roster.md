---
name: 'Adding an agent to the roster: every list and count it touches'
description: 'ROSTER, repo.rs expected_roster, smoke and session_start lists, three budget tables, evals, guide row and cap, goldens; traps found adding blog-writer'
metadata:
  type: project
  modified: 2026-11-28
---

Adding blog-writer (copy plan Task C, 2026-11-28) touched: anthology-core ROSTER (sorted; binary release), crates/repo-tests/src/repo.rs expected_roster and expected_knowledge (plus FIRST_PARTY_KNOWLEDGE and SUPPORT_KNOWLEDGE), crates/anthology-dev/tests/smoke.rs EXPECTED_ROSTER (row count, always reads only) and src/smoke/helpers.rs EXPECTED_AGENTS, crates/anthology-runtime/tests/session_start.rs AGENTS, the BUDGET table (src/startup_budget.rs and the_budget_table_is_gc13_verbatim; repo-tests roster.rs has no copy since A1), evals route-<agent> and knowledge-<agent> cases (a read, order and inorder grader per always file, in Startup order), the routing guide row, and goldens memory.sync and dev.startup-budget. Count tests to bump: knowledge.rs, licenses.rs, repo_api_test.rs, sources_json_test.rs names, evals_layout.rs startup.len().
Traps: a new always line goes in the agent Startup in the same order as the knowledge list in repo.rs and smoke.rs. A roster test that forbids "$(" must read the Do section only, because every Memory section names "$(" as a thing to avoid. Adding a conditional line to ui-designer can push it to 61 body lines: join the worktree sentence onto the one-git-command line. A budget row is raised only when startup-budget says OVER; a row that still fits (marketer) stays.
A one-shot or session golden check needs umask 022: run sh -c with umask 022 and cargo run -p anthology-dev -- golden bless --surface <name>.
Why: the roster count appears in about ten files and only the failing tests name them.
