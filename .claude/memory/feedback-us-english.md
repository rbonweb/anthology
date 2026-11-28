---
name: 'US English everywhere'
description: 'All first-party text and every agent''s output use US English spelling (color, behavior, organize, license, canceled); vendored upstream text stays as written'
metadata:
  type: feedback
  modified: 2026-11-28
---

Owner, 2026-11-28: "make sure that everywhere, US English has been used, not UK English. And whatever agents are there, make sure that they are going to use US English."

**Why:** the owner's standard for the product and the public repo.

**How to apply:**
- Every brief, plan, report, doc, code comment, user-facing string, commit message and memory note uses US spelling. That includes the lead's own messages.
- All 12 agent prompts carry one pinned US English line. A repo-tests guard fails on UK words in first-party files.
- Leave vendored upstream text alone: vendor/, plugins/anthology-superpowers/, plugins/anthology/knowledge/, atl/ upstream files and quoted third-party rules.
- Keep external contract strings (config or state keys, CLI flags, JSON fields) stable. Allow-list them with a reason.
