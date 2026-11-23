---
name: 'A reused cargo target volume can replay a mutant build'
description: 'git archive files carry commit-time mtimes, older than a mutant build, so cargo skips the rebuild and tests a restored tree with mutant code'
metadata:
  type: feedback
  modified: 2026-11-23
---

After a mutation run on a named target volume, a later probe run that extracts git archive of the same head into a fresh tree reused the last mutant build: cargo saw the source mtimes (the commit time) as older than its fingerprints, replayed the mutant warnings and gave wrong verdicts.
Why: in the PR 193 re-review a probe batch ran against the call-site mutant and showed bogus Allow rows until the volume was removed.
How to apply: use a separate target volume for mutants and for probes, or remove the volume (or touch every source file) before probing a tree after mutants ran on it.
