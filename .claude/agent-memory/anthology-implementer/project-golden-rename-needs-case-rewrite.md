---
name: 'Renaming a surface needs golden case rewrites, not only bless'
description: 'Goldens hold cases (hex blobs, digested cases, hex priors) that golden bless alone cannot fix after a rename; run difftest as non-root'
metadata:
  type: project
  modified: 2026-11-22
---

golden bless only rewrites expected results. After renaming names in a surface generator, stored cases still carry the old names, digested cases (case_sha256) fail to resolve, hex fields (tarball blobs, state priors) hide old names, and plain core goldens (crates/anthology-core/tests/golden/*.jsonl) have their own replay test.
Procedure that worked: rewrite stored cases by decoding JSON and renaming strings (keys and values, hex priors via bytes), then bless with a temporary uncommitted patch that re-takes digested or all cases from the surface generator by id. Compare old expects (renamed) with new ones to show only renames or key re-sorting changed.
Run difftest golden check/bless as the host uid, not root: root ignores mode-0 files and changes runtime.guardrails_loc results. The slim image has no ps, so 3 bootstrap tests fail there; install rustfmt and clippy per run as root.
Why: the ATL rename (T3) hit every one of these; bless alone would have recorded hollow results.
