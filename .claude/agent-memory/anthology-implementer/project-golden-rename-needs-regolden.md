---
name: 'Renaming a word in a difftest surface needs a regolden, not a bless'
description: 'Frozen goldens keep case inputs and case digests inline; bless only rewrites expected results, so removing a name from a golden means rebuilding its cases first'
metadata:
  type: project
  modified: 2026-11-22
---

A difftest golden (crates/*/tests/golden/*.jsonl.gz) stores each case input inline, or as case_sha256 when the input is over 8 KB. golden bless rewrites only the expected result of a case that differs; it never changes an input. So after renaming literals in a surface source, golden check fails with "keeps this case as a digest" for every digested case, and the old name stays in every inline input.
What worked (ATL rename T4): a temporary, uncommitted patch in crates/difftest/src/main.rs, active only when DIFFTEST_REGOLDEN is set, that rebuilds each chosen surface cases from its own seeds() and generate() (header seed and gen), keeps the old expect and marks by id, gives a new id a placeholder expect {"exit": 0}, and writes the golden with golden::write. Then golden check shows the real diffs and golden bless records Rust results. New seeds can be appended to a surface this way; check_shape refuses a golden with fewer cases than seeds plus gen.
Revert the main.rs patch before committing (git checkout on that one file).
Why: the repo test atl_rename.rs reads every golden decompressed and fails on the old name outside the shell and guard goldens.
How to apply: only the surfaces you own; each surface needs its own regolden, and the surface seeds must keep their order so ids stay stable.
