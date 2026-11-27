---
name: 'Routing guide size budget'
description: 'routing-guide.md has a 3800-char test cap (3799 now); what to cut, what is pinned, where to test'
metadata:
  type: project
  modified: 2026-11-27
---

plugins/anthology/templates/routing-guide.md is capped at 3800 characters by guide_test::test_size_and_shape in crates/anthology-runtime/tests/session_start.rs (wc -m counts characters). It sits at 3799, so any added line needs words cut in the same PR.
Pinned by exact text (keep verbatim): the Lead and Inline only lines, the user notes line, the merge-or-pull memory rule, the BrowserOS lines, the attribution line, the literal-path removal line in section 5, Docker unavailable: stop and tell the user, tag images, name containers per worktree, never git worktree add, and the roster.rs needles (Merge conflicts: implementer., Checks over ~2 min, No lead coding tasks in plans., the whole line Brief by plan path and task ID; agents read the plan., User stop or pause beats the plan, Before a restart). Pinned also: the project-agents line (session_start.rs). Unpinned and safe to tighten: the licensing line, the section 2 heading (now Agents), the section 7 heading (now ATL), the Condensed output line, the blocked-agents sentence.
The ATL section says Never type atl (added for you), except atl bypass <cmd> when a result is unusable; test_atl_section_says_never_type_atl pins it, and the same sentence is in the shared Shell section of all 11 agent prompts (roster.rs needle).
Per-repo content goes into session start instead of the guide: MAP_LINE in session_start.rs and the own-agents section from crates/anthology-runtime/src/project.rs.
Why: the guide is injected into every session, so its size is a context cost, and a failing guide test blocks CI. The guide is read from the plugin folder at run time, not compiled into the anthology binary, so a guide edit needs only a plugin release.
