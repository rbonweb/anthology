---
name: 'After-work mod: edits outside a git repo skip the commit and test gaps'
description: 'Edited paths are read from the report text, placed by walking up for .git; tests in a world with no .git read as outside a repo'
metadata:
  type: project
  modified: 2026-11-27
---

afterwork.ts editsOnlyOutsideRepo reads absolute, ~/ and relative (extension or ./) paths from the report text; the event has no edited-path field. Each path is placed by $.fs.exists on .git walking up (MAX_WALK); relative uses the $.agent.list() cwd, ~ uses HOME. No path, one path in a repo, or anything unplaceable keeps both gaps. It is asked only when a gap stands. Why: the engine gives no tool-use list on agent.report, and a conservative text read keeps the old checks whenever unsure. Test trap: a test world with no .git makes any path in the report read as outside a repo, so the map tests use MAP_BASE with no path and give worlds the .git files that mapIsThere also walks to.
