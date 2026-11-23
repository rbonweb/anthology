---
name: 'Mutate the call site of a defense layer, not only the layer'
description: 'Tests that call a check function directly do not prove the caller uses it; replace the call and rerun'
metadata:
  type: feedback
  modified: 2026-11-22
---

When a change claims two independent layers (for example the scanner plus analyze cross-check in classify_readonly), mutate the call site in the caller (replace the call with a no-op) as well as the layer own logic.
Why: in the P3 review, every logic mutant was killed, but deleting the agrees_with_analyze call in prove() survived all shell and guard tests. The in-crate test called the function directly, and the program table refused every case the cross-check refused.
How to apply: run the mutants in the CI rust image on a scratch worktree (cp the file to a backup, sed, test, restore). In atl, keep the code compiling under deny(warnings): use let _ = ... rather than deleting a line.
