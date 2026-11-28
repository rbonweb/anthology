---
name: 'Slop density thresholds: where the numbers are repeated'
description: 'Density limits (150 and 300 word minimums, 1 per 80 dashes, 1 per 150 triads) live in density.rs but are repeated in the rule summary, the report table test and the docs page; density.rs also decides which dashes and lists are structure'
metadata:
  type: project
  modified: 2026-11-28
---

crates/anthology-slop/src/rules/density.rs holds the limits: em dashes need 150 new words and fire above 1 per 80; lists of three need 300 new words and fire above 1 per 150. A spaced en dash between range ends (digits, times, day or month names, am or pm) is not counted. The same numbers are printed in crates/anthology-slop/src/rules/mod.rs (summary strings), crates/anthology-slop/src/report.rs (the rules table test pin) and docs/slop-gate.md (rules table). Change all four together or the docs and the table drift. Tests in density.rs pad their docs with filler words to pass the minimums: a negative test with fewer words than the minimum proves nothing, so pad it. Why: the review fix touched only density.rs and left the summary text saying 1 per 120.

Structure is not counted (review 2, I4): density.rs reads the line of each dash once (DashLine, linear). Skipped: every dash in an ATX heading or a table row; every dash inside a [...] pair on the line; the first two dashes from the one after a list item label (bold, linked bold, underscore, masked code, task-list box, optional colon and short value); the one dash after a leading date or version (changelog line). Every dash of a line is read, new or not, so the skips fall on the first ones. The mask view (markdown.rs LEAD_TERM) also records bold-lead dashes in dash_skip; density.rs repeats the check so plain views behave the same. Lists of three use [^\S\r\n] between words, so a list never crosses a line break, and a match followed by the, a, an, your, their, its, this, our, his, her, my, it or them is a verb phrase and does not count. Do not add is/are/was to that list: "is fast, secure and scalable" is the typical tell. plugins/anthology/agents/scout.md and marketer.md must stay clean: a unit test reads both with include_str.
