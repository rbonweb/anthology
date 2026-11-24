---
name: 'CI Rust tests: shards, gate jobs and the release-metadata scope'
description: 'rust-test-linux/-macos are gates over -shard jobs; rust-scope.sh skips the shards for release metadata; where each is pinned'
metadata:
  type: project
  modified: 2026-11-24
---

The difftest golden replay (the_committed_goldens_replay_clean, Linux only) takes about 9 of the 16 minutes of one Linux run; the build is only about 1.5 minutes, so a build cache saves little. ci.yml runs rust-test-linux-shard as plain/all x replay/rest and rust-test-macos-shard as plain (then shims) and all (then the ledger macOS listings). The jobs named rust-test-linux and rust-test-macos are gates (ci/jobs/rust-gate.sh) over the shards and keep the old names. The rest shard uses -- --exact --skip NAME so only that test is dropped; the replay shard keeps --workspace so --all-features unifies features as in the full run, and fails if the filter lists the test other than once. ci/jobs/rust-scope.sh prints full=true|false for a PR; full=false only when every change is release metadata (memory notes, CHANGELOG.md, changelog.d, plugins/<p>/anthology.lock, and version-only edits of plugin.json, Cargo.toml package tables and the two Cargo.lock files). Then the gate runs cargo test -p repo-tests instead. All of it is pinned in crates/repo-tests/tests/rust_test_jobs.rs. Why: renaming the replay test, adding a shard axis or a new metadata path must update the scripts and that test together.
