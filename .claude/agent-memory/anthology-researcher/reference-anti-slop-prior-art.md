---
name: 'Anti-slop design and writing gates (survey 2026-11-28)'
description: 'Which anti-slop skills, linters and hook gates exist, their stars, evidence and offline fitness'
metadata:
  type: reference
  modified: 2026-11-28
---

Why: owner wants hard anti-slop gates for ui-designer, ui-reviewer, docs-writer, marketer and product-planner; this saves a re-survey.
Design: pbakaus/impeccable (Apache-2.0, about 118k stars, skill-v4.11.1) ships a no-LLM detector, 62 rules, exit 2 on findings, SARIF output, prebuilt per-OS engine binaries, and a Claude Code edit hook. Vendored ui-taste v14 embeds impeccable at commit 8a0e198 (2026-10-15), 1518 commits behind main on 2026-11-28. funboy322/avoid-ai-design (MIT, 176 stars) has an 81-tell catalog and a zero-dep Node scanner, exit 2 on P0/P1, PostToolUse hook mode, per-line ignores. hankimis/ai-design-tells calibrated 27 weighted tells on 202 human sites and found Inter alone is not a tell.
Writing: blader/humanizer (MIT, about 74k stars) won the Growth Method 10-skill benchmark (81/100, one article, Nov 2026). conorbronsdon/avoid-ai-writing (MIT, about 6.4k) has a zero-dep detector, gate threshold, and ignore-start/end markers for quoted specimens. hardikpandya/stop-slop (MIT, about 19k) is rules plus a 35/50 rubric, no checker. tbhb/vale-ai-tells (MIT, 137 Vale rules, corpus-checked against pre-LLM docs) and thrash-d/slop-linter (Vale rules plus PowerShell PostToolUse hook, should-pass tests) are the offline lint options; Vale is a single Go binary and skips fenced code by default.
Repo state then: no anthology agent, hook or crate had any writing anti-slop rule.
