---
name: anthology-status
description: "Anthology project status — PLAN.md rev. 6 approved, building on branch `build` via SDD; ledger at .superpowers/sdd/PLAN/progress.md; repo public"
metadata:
  node_type: memory
  type: project
  originSessionId: 1c9fb279-a45d-4d27-8e78-1ddd82c1e344
  modified: 2026-11-20T10:56:38.657Z
---

State as of 2026-11-20: the plan is ONE file, `~/Desktop/anthology/PLAN.md`, rev. 6, **approved, building**.
- Part 1: design §1–14. Part 2: 17 build tasks. Part 3: build decisions (rulings, U1–U9 findings, U5 pins).
- Don't recreate a docs folder or split the plan.

**Build process**
- The owner said "start building", made the GitHub repo PUBLIC, and left with "no more questions — figure it out yourself". The lead decides and records `Ruling:` lines in the ledger.
- Subagent-driven development: Sonnet builds, Opus reviews/investigates, Haiku scouts.
- Ledger at `.superpowers/sdd/PLAN/progress.md` (git-ignored). Trust it plus `git log` after compaction.
- The SDD skill copy lives in `.superpowers/sdd/PLAN/sdd-skill/`; Task 16 uninstalls the official superpowers.
- Tasks 2–17 go on branch `build` (draft PR for CI); merge to main after the final review.
- Repo-local git email = GitHub noreply address. Never commit personal details: the repo is public.

**The user's long-term decisions**
- The routing guide is delivered by a SessionStart hook.
- Memory is active only in approved repos (keyed by remote URL).
- Superpowers is vendored and renamed `anthology-superpowers` (versioned independently, 0.1.0).
- macOS + Linux only. Runtime Python 3.9+.
- Memory design: Markdown notes in the repo are the source of truth; MEMORY.md indexes and the SQLite FTS graph (`anthology-mem`) are derived; personal notes go in `~/.claude/anthology/personal-memory`; export/import is supported.
- Accepted risk: the blanket `Bash(rtk *)` allow. Don't relitigate it.

**Why:** The user works plan → approval → build and wants long-term choices, never effort-based shortcuts.

**How to apply:** On resume, read the ledger and continue the next task. See [[anthology-user-workflow]] and [[anthology-machine-facts]].
