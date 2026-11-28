---
name: 'Routing guide size budget'
description: 'routing-guide.md is capped at 3950 chars (3949 now after the credentials line, 2026-11-28); what is pinned, what was cut, what is left'
metadata:
  type: project
  modified: 2026-11-28
---

plugins/anthology/templates/routing-guide.md is capped by guide_test::test_size_and_shape in crates/anthology-runtime/tests/session_start.rs (wc -m counts characters). The cap was 3800; the owner raised it to 3950 on 2026-11-28 for the fan-out rule. The credentials line (owner, 2026-11-28, end of section 8) brought the guide to 3949, so there is 1 character of room: any further rule needs a cap decision from the owner or the lead.
Pinned by exact text (keep verbatim): the Lead and Inline only lines, the user notes line, the merge-or-pull memory rule, the BrowserOS lines, the attribution line, the literal-path removal line in section 5, Docker unavailable: stop and tell the user, tag images, name containers per worktree, never git worktree add, and the roster.rs needles (Merge conflicts: implementer., Checks over ~2 min, No lead coding tasks in plans., Brief by plan path and task ID; agents read the plan., User stop or pause beats the plan, Before a restart). Also pinned: the project-agents line, and the seven safety-floor lines, by their starts in FLOOR_LINES (crates/anthology-runtime/src/project.rs) and by longer prefixes in FLOOR_NEEDLES (session_start.rs tests); the web line in section 6 must not be shortened (two CI shards went red). Do not append a new rule to the floor line Never save secrets: the floor copies that whole line.
Cuts already used, in order: agents table without outer pipes; the ATL condensed-output sentence, the licensing line, the blocked-agents line and the anthology-mem save line shortened (first tightening); then for the credentials line: the second sentence of the opening (Agents never see this guide), a bug; in the idle-agents line, on disjoint files and Several in the fan-out line (the worktree line already says parallel writers get disjoint files), and the Save notes line merged into the Save corrections line. Measured before the credentials line: tightening every unpinned line alone gives about 40 characters, not 100, so a new rule of about 100 characters needs cuts like these.
Why: the guide is injected into every session, so its size is a context cost, and a failing guide test blocks CI.
