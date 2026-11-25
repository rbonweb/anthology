---
name: 'Routing guide lines pinned by tests'
description: 'Which guide and large-builds phrases tests pin, so a trim does not break them'
metadata:
  type: project
  modified: 2026-11-24
---

Editing plugins/anthology/templates/routing-guide.md: crates/repo-tests/tests/roster.rs (test_guide_has_the_lead_stop_and_per_worktree_rules) pins the exact sentences "Merge conflicts: implementer.", "User stop or pause beats the plan: start nothing new till they say go." and "Before a restart, agents commit work in progress.". crates/anthology-runtime/tests/session_start.rs (guide_test) pins the Lead, Inline only, project-map, commit-author, delete-literal-paths, user-notes and merge-or-pull memory lines. Add new text after a pinned sentence, never inside it. The Docker, tests and cleanup rules are also in every agent prompt and in large-builds, so those guide lines are the cheapest to trim for budget. Why: the 3800-character cap leaves almost no room, and the needle tests fail late.
