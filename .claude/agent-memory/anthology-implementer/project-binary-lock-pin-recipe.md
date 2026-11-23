---
name: 'Pinning a new anthology binary release: lock recipe and traps'
description: 'dev.sh lock write needs its inputs inside the worktree and a url per asset; every dev.sh call is slow on a busy machine; a lock PR always needs a plugin version bump'
metadata:
  type: project
  modified: 2026-11-23
---

scripts/dev.sh runs in a container that mounts only the worktree, so checksums.txt and the release JSON must be copied under target/ (gitignored), not left in /tmp. Order: wait for the anthology-release run, lock write, lock check, verify one asset with gh attestation verify plus shasum, lock check-fresh, bump both plugin.json versions, CHANGELOG line in each plugin Unreleased section, then the anthology_lock, session_start and bootstrap tests.
Run the tests with one call: scripts/rust-test.sh run cargo test -p repo-tests -p anthology-runtime --test anthology_lock --test session_start --test bootstrap. Why: cargo test takes only one positional name filter, and name filters after -- match test names, not test file names, so they run almost nothing.
Why background: each dev.sh or rust-test.sh run installs apt packages again, so on a saturated Docker VM one call took 5 to 40 minutes. Run them in the background and run only one at a time; a second call fights over the same target volume.
Version gate: the version-bump CI job (crates/anthology-dev/src/version_bump.rs) compares plugin.json against the merge base with the PR base branch, not the last release tag. A lock PR changes both plugins, and both have release tags, so each needs a version above what the base branch holds, even when the base version is itself unreleased.
The release JSON needs name, digest and url per asset (lock write fails with "the release listing has no single asset <url>" without url): write it with the Write tool from gh release view --json assets, keeping those three fields. gh attestation verify prints nothing on success; check its exit code or use --format json. The guardrails or anthology Unreleased section may have no Changed heading yet; add one. A shell until-loop on git diff --quiet exits at once, because quiet returns 0 when nothing changed: wait with while instead.
