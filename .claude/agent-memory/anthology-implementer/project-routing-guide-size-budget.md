---
name: 'Routing guide size budget'
description: 'routing-guide.md is capped at 3950 chars (3919 now, owner 2026-11-28); what is pinned, what was already tightened'
metadata:
  type: project
  modified: 2026-11-28
---

plugins/anthology/templates/routing-guide.md is capped by guide_test::test_size_and_shape in crates/anthology-runtime/tests/session_start.rs (wc -m counts characters). The cap was 3800; the owner raised it to 3950 on 2026-11-28 for the fan-out rule, and the guide is 3919 now. A rule added later needs a cap decision from the owner or the lead, because almost every line is pinned.
Pinned by exact text (keep verbatim): the Lead and Inline only lines, the user notes line, the merge-or-pull memory rule, the BrowserOS lines, the attribution line, the literal-path removal line in section 5, Docker unavailable: stop and tell the user, tag images, name containers per worktree, never git worktree add, and the roster.rs needles (Merge conflicts: implementer., Checks over ~2 min, No lead coding tasks in plans., Brief by plan path and task ID; agents read the plan., User stop or pause beats the plan, Before a restart). Also pinned: the project-agents line, and the starts of the seven safety-floor lines (FLOOR_LINES in crates/anthology-runtime/src/project.rs).
The 2026-11-28 tightening already used the cheap cuts: the agents table has no outer pipes, the ATL condensed-output sentence, the licensing, blocked-agents and web lines, and the anthology-mem save line were shortened. Measured: with every unpinned line tightened, two extra rules of about 295 characters still came to 4033, so do not promise the owner that tightening alone makes room.
Per-repo content goes into session start instead of the guide: MAP_LINE in session_start.rs and the own-agents section from crates/anthology-runtime/src/project.rs. The guide is never cut at session start; memory gives way under the 9000 budget.
Why: the guide is injected into every session, so its size is a context cost, and a failing guide test blocks CI.
