---
name: 'Pinning a new anthology binary release: lock recipe and traps'
description: 'dev.sh lock write needs its inputs inside the worktree; every dev.sh call is slow on a busy machine; a lock PR always needs a plugin version bump'
metadata:
  type: project
  modified: 2026-11-22
---

scripts/dev.sh runs in a container that mounts only the worktree, so checksums.txt and the release JSON must be copied under target/ (gitignored), not left in /tmp. Order: wait for the anthology-release run, lock write, lock check, verify one asset with gh attestation verify plus shasum, lock check-fresh, bump both plugin.json versions, CHANGELOG line in each plugin Unreleased section, then the anthology_lock, session_start and bootstrap tests (repo-tests and anthology-runtime packages, one cargo test call via scripts/rust-test.sh run).
Why: each dev.sh or rust-test.sh run installs apt packages again, so on a saturated Docker VM one call took 5 to 40 minutes. Run them in the background and run only one at a time; a second call fights over the same target volume.
Version gate: the version-bump CI job (crates/anthology-dev/src/version_bump.rs) compares plugin.json against the merge base with the PR base branch, not the last release tag. A lock PR changes both plugins, and both have release tags, so each needs a version above what the base branch holds, even when the base version is itself unreleased.
The guardrails Unreleased section may have no Changed heading yet; add one. The release JSON only needs asset name and digest, so it can be written with the Write tool. A shell until-loop on git diff --quiet exits at once, because quiet returns 0 when nothing changed: wait with while instead.
