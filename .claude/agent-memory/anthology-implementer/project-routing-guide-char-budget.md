---
name: 'Routing guide has almost no room under its 3800 character limit'
description: 'plugins/anthology/templates/routing-guide.md sits near the 3800 char test limit and several lines are test-locked'
metadata:
  type: project
  modified: 2026-11-22
---

The routing guide test (crates/anthology-runtime/tests/session_start.rs, guide_test) caps the file at 3800 characters (wc -m); it was at 3795 before the repo-memory-only change. Adding a line means cutting words elsewhere in the same PR.

Lines the tests lock by exact text: the Lead and Inline only lines, the user notes line (must contain "never into the repo" and "Never commit a `user` note"), "personal memory at the path given below", the "Before each merge or pull, and at task end" memory rule, the BrowserOS lines, and phrases like chore(memory) and Docker unavailable: stop and tell the user. Trim prose lines that no test names (blocked-agent wording, Worth remembering aside) instead.

Why: a failing guide test blocks CI, and the guide is injected into every session, so its size is a real cost.
