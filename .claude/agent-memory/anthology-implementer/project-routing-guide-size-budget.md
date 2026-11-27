---
name: 'Routing guide size budget'
description: 'routing-guide.md has a 3800-char test cap (3799 now); what to move out, what is pinned, where to test'
metadata:
  type: project
  modified: 2026-11-27
---

plugins/anthology/templates/routing-guide.md is capped at 3800 characters by guide_test::test_size_and_shape in crates/anthology-runtime/tests/session_start.rs (wc -m counts characters). It is at 3799 after the project-agents change, so any added line needs words cut in the same PR.
Pinned by exact text (keep verbatim): the Lead and Inline only lines, the user notes line (never into the repo, Never commit a `user` note), the merge-or-pull memory rule, the BrowserOS lines, the delete and attribution lines, Docker unavailable: stop and tell the user, tag images, name containers per worktree, never `git worktree add`, the roster.rs needles (Merge conflicts: implementer., Checks over ~2 min, No lead coding tasks in plans., Brief by plan path and task ID, User stop or pause beats the plan, Before a restart). Unpinned and safe to tighten: licensing line, section headings 1 and 7 (now Roles and ATL output), the blocked-agents sentence.
Content that is per repo goes into session start instead of the guide: the project-map and architect line moved to MAP_LINE in session_start.rs (added only for a repo approved in setup), and the repo own agents and skills section is built by crates/anthology-runtime/src/project.rs. That frees guide space and keeps unapproved repos out of an architect run.
Why: the guide is injected into every session, so its size is a context cost, and a failing guide test blocks CI.
