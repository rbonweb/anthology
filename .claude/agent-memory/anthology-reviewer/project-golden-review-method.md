---
name: 'How to review golden re-blesses'
description: 'Decode old and new jsonl.gz goldens and diff per case after stripping intended keys; guard.hook specifics'
metadata:
  type: project
  modified: 2026-11-24
---

Goldens (setup, hook.session-start, memory.sync, guard.hook) are jsonl.gz: a header line, then one line per case with id, case, expect, marks. Cases over a size cap store only stdout_len and stdout_sha256 (and case_sha256), so they cannot be diffed. Bless drops marks such as D-4 on rewritten cases (no Python oracle), which is not a regression. Bless also writes a today key into the header; replay pins the clock to today, or to LEGACY_DAY (2026-11-22) when the key is absent, so adding today as 2026-11-22 changes nothing.
guard.hook expect holds only exit and stderr. Its Rust side is in-process anthology_guard::decide (crates/difftest/src/surfaces/guard_hook.rs), so process env never reaches it; CLI tests use env_clear through anthology-testkit run_program_timeout.
Why: the Python oracle is gone, so a bless can hide a regression; per-case diffs after removing the intended new keys show what else changed. In PR 215 (T5) this diff was the only real verdict proof: its verdicts_are_unchanged test compared check_with with check_explained, which share one body.
How to apply: git show the base golden, gunzip both, compare per id with python3 -I (ids equal, case equal, exit equal, stderr equal to the old line plus only the expected suffix), and list residual differences. In zsh quote rev:path as "${H}:path", since $H:c is a modifier.
