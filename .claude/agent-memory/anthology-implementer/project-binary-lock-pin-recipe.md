---
name: 'Pinning a new anthology binary release: lock recipe and traps'
description: 'dev.sh lock write needs its inputs inside the worktree and a url per asset; atl.lock is hand-written; a lock PR needs a plugin version bump and a fresh check-fresh; how to wait without noisy output'
metadata:
  type: project
  modified: 2026-11-24
---

scripts/dev.sh runs in a container that mounts only the worktree, so checksums.txt and the release JSON must be copied under target/ (gitignored), not left in /tmp. Order: wait for the release run, lock write, lock check, verify one asset with gh attestation verify plus shasum, lock check-fresh, bump both plugin.json versions, CHANGELOG line in each plugin Unreleased section, then the lock tests.
Run the tests with one call: scripts/rust-test.sh run cargo test -p repo-tests -p anthology-runtime --test anthology_lock --test atl_lock --test session_start --test bootstrap. Why: cargo test takes only one positional name filter, and name filters after -- match test names, not test file names, so they run almost nothing.
Why background: each dev.sh or rust-test.sh run installs apt packages again, so on a saturated Docker VM one call took 5 to 40 minutes (the first call after a Docker restart took over 3 minutes). Run them in the background and one at a time; a second call fights over the same target volume.
Version gate: the version-bump CI job (crates/anthology-dev/src/version_bump.rs) compares plugin.json against the merge base with the PR base branch, not the last release tag. A lock PR changes both plugins, so each needs a version above what the base branch holds, even when the base version is itself unreleased. The anthology dependency range on guardrails (plugin.json) is a floor raised only when anthology needs a guardrails feature, not on every guardrails bump; ~0.2.7 accepts 0.2.8.
The release JSON needs name, digest and url per asset (lock write fails with "the release listing has no single asset <url>" without url): write it with the Write tool from gh release view --json assets. gh attestation verify prints nothing on success; use --format json with a --jq to see the cert source and tag.
atl.lock has no dev.sh subcommand: lock handles only anthology.lock. Edit plugins/anthology/atl.lock by hand from the atl release checksums.txt (a placeholder lock holds all-zero hashes and the new version already); crates/repo-tests/tests/atl_lock.rs checks it.
check-fresh compares the shipped set (crates the CLI is built from, including anthology-core Cargo.toml and src) between the lock tag and HEAD. Merging build after a core change makes it fail, so pin from the newest build and rerun check-fresh after every merge of build.
The guardrails or anthology Unreleased section may have no Changed heading yet; add one. A shell until-loop on git diff --quiet exits at once, because quiet returns 0 when nothing changed: wait with while instead.
Waiting without noise: gh run watch prints the whole job tree every 3 seconds, so use `timeout 170 gh run watch <id> --exit-status --compact --interval 60`. dev.sh calls move to the background after the tool timeout and print only at the end; block on them with `timeout 170 tail -f <task output file>` in repeated calls. dev.sh lock write took about 8 minutes, lock check about 6, check-fresh about 7. Do not run dev.sh --help: it starts the container build and runs for minutes.
