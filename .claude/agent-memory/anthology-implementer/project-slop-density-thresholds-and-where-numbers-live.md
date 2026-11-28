---
name: 'Slop density thresholds: where the numbers are repeated'
description: 'Density limits (150 and 300 word minimums, 1 per 80 dashes, 1 per 150 triads) live in density.rs but are repeated in the rule summary, the report table test and the docs page'
metadata:
  type: project
  modified: 2026-11-28
---

crates/anthology-slop/src/rules/density.rs holds the limits: em dashes need 150 new words and fire above 1 per 80; lists of three need 300 new words and fire above 1 per 150. A spaced en dash between range ends (digits, times, day or month names, am or pm) is not counted. The same numbers are printed in crates/anthology-slop/src/rules/mod.rs (summary strings), crates/anthology-slop/src/report.rs (the rules table test pin) and docs/slop-gate.md (rules table). Change all four together or the docs and the table drift. Tests in density.rs pad their docs with filler words to pass the minimums: a negative test with fewer words than the minimum proves nothing, so pad it. Why: the review fix touched only density.rs and left the summary text saying 1 per 120.
