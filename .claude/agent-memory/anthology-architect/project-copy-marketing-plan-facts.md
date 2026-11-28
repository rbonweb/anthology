---
name: 'Facts that shaped the copy and marketing plan (2026-11-28)'
description: 'Startup budget counts conditional reads; a new agent needs a binary release; brand-review and claude-blog do not vendor cleanly'
metadata:
  type: project
  modified: 2026-11-28
---

Why: each fact changed the 2026-11-28 copy and marketing plan (docs/superpowers/plans/2026-11-28-copy-and-marketing.md) and none shows from one file.
The startup budget (crates/anthology-dev/src/startup_budget.rs) counted every knowledge path in the Startup section, conditional "only when" lines included, so conditional reads did not keep a budget small; the eval knowledge-reviewer scored 0.625 because its graders demanded the conditional reads. The plan makes a line ending in ", only when|for|to ..." conditional and counts only the largest conditional file.
The dev.startup-budget golden replays the real checkout, so any agent prompt or knowledge size change needs a bless of that surface; memory.sync goldens quote the roster list in the unknown-agent error.
A new agent touches anthology-core ROSTER (binary graph, so a bin release and lock write before the plugin can release), repo-tests repo.rs expected_roster, anthology-dev smoke.rs EXPECTED_ROSTER and helpers.rs EXPECTED_AGENTS, session_start.rs AGENTS, BUDGET, evals route and knowledge cases, evals_layout count, and a guide row; anthology-mem save refuses an agent not in ROSTER.
Agent body cap is 60 lines; ui-designer sat at exactly 60, marketer 48, reviewer 53, docs-writer 52 before the plan.
Vendor tooling had no first-party knowledge and no way to leave upstream files out; vendor check ignores folders that no source targets. KWP brand-review has a tilde placeholder that strip-connectors rejects, so it needs a patch instead. claude-blog sub-skills link into the shared skills/blog folder and run repo-root scripts plus a Playwright render for their delivery gates, so they need heavy patches.
Agent files, the guide and docs may not mention the word python (cutover_docs.rs allowlist); non-browser agents may not mention BrowserOS; ledger rows pin test function names, so counts change but names stay.
