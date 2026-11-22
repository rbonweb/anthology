---
name: 'Pinning a new anthology binary release: lock recipe and traps'
description: 'dev.sh lock write needs its inputs inside the worktree; every dev.sh call is slow on a busy machine'
metadata:
  type: project
  modified: 2026-11-22
---

scripts/dev.sh runs in a container that mounts only the worktree, so checksums.txt and the release JSON must be copied under target/ (gitignored), not left in /tmp. Order: lock write, lock check, verify one asset with gh attestation verify plus shasum, lock check-fresh, set-version for both plugins, CHANGELOG line in each plugin Unreleased section, then the anthology_lock, session_start and bootstrap tests.
Why: each dev.sh or rust-test.sh run installs apt packages again, so on a saturated Docker VM one call took 5 to 40 minutes. Run them in the background and run only one at a time; a second call fights over the same target volume.
The guardrails Unreleased section may have no Changed heading yet; add one. A shell until-loop on git diff --quiet exits at once, because quiet returns 0 when nothing changed: wait with while instead.
