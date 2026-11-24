---
name: 'After-work map-missing check (PR 225) review findings'
description: 'Head 41e925d4: the missing-map verification is bypassed when the report names the map file; goldens and budgets were exact'
metadata:
  type: project
  modified: 2026-11-24
---

plugins/anthology/hooks/mods/afterwork.ts: MAP_ABSENT is cut from the text with without(), but MAP_DEALT_WITH still matches any leftover project map or reference-project-map text. So "Map file not found at .claude/memory/reference-project-map.md" and "reference-project-map.md does not exist" count as dealt with even when the file is there; the tests only use claims without the file name. MAP_ABSENT also cannot cross a dot, so the file name form never matches it. Why it matters: the new agent rule tells writers to quote the Read error, which names that file. Golden proof method that worked: gunzip old (git cat-file at the base sha) and new (gh api raw into a temp dir) and compare per row; startup-budget rows differed only by each agent byte delta, upstream-watch rows only by the scout budget table row.
