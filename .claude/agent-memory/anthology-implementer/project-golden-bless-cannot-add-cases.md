---
name: 'Golden bless only rewrites recorded cases, it cannot add rows'
description: 'After the oracle is gone, new golden rows come from a scratch twin script plus bless; surface seeds and the golden must stay in step'
metadata:
  type: project
  modified: 2026-11-22
---

difftest golden bless only rewrites expected results of cases already in the golden; it never adds cases, and freeze needs the deleted Python oracle. To add rows (for example an atl twin of every legacy-name case): append twin rows to the .jsonl.gz with a scratch script (placeholder expect copied from the original, one row given exit 99 so bless rewrites the file with the Rust gzip), then run bless, which records the real result. golden check only needs cases >= surface seeds + generated, so extra ids are fine; give a twin the id the surface would make when the source data (crates/anthology-guard/tests/data/test_guard_rules.jsonl) also grows, since guard.check rows:<py>:<n> and guard.hook check:<id> seeds derive from it. Digested cases (case_sha256) cannot be twinned, and a list the generator picks from cannot grow without changing every digested generated case; append new seeds after the last existing one instead, so recorded ids stay put. Why: the lead assumed bless adds rows; it does not. How to apply: plan golden changes as scratch-twin then bless, then diff gzip -dc old and new to prove only rows were added. On macOS use gzip -dc, not zcat (it looks for .Z).
