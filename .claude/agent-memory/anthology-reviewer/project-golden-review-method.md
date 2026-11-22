---
name: 'How to review golden re-blesses'
description: 'Decode old and new jsonl.gz goldens and diff per case after stripping intended keys'
metadata:
  type: project
  modified: 2026-11-22
---

Goldens (setup, hook.session-start, memory.sync) are jsonl.gz with id, case, expect, marks. Cases over a size cap store only stdout_len and stdout_sha256 (and case_sha256), so they cannot be diffed. Bless drops marks such as D-4 on rewritten cases (no Python oracle), which is not a regression.
Why: the Python oracle is gone, so a bless can hide a regression; per-case diffs after removing the intended new keys show what else changed.
How to apply: git show the base golden, gunzip both, compare per id with python3 -I, strip the PR intended keys, and list the residual differences.
