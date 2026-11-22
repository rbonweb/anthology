---
name: 'Batch guard probes: case file plus base and head diff'
description: 'Fast way to judge many guard payloads on base and head and prove them in bash and zsh'
metadata:
  type: project
  modified: 2026-11-22
---

In the rust image container (shared cargo and rustup volumes, git archive of head into /src and of the merge-base into /base), add a scratch test in crates/anthology-guard/tests that reads a case file split on lines of four dashes and prints the check verdict per case via support/gd.rs. Run it under both trees (separate CARGO_TARGET_DIR) and paste the outputs side by side. Prove runtime in debian:bookworm-slim with zsh installed (bash 5.2.15, zsh 5.9) and a marker npx in /usr/local/bin that appends its arguments to a file; run each case with bash -c and zsh -c in a fresh temp dir with timeout 5.
Why: the PR 159 review checked about 230 payloads this way in minutes; the base column separates regressions from pre-existing gaps.
How to apply: zsh +cmd qualifiers call the marker with a file name, not the payload arguments, so use the e-colon form for a runtime proof; a pattern group needs a matching file to exist.
