---
name: 'atl release prep: Cargo bump vs atl.lock test'
description: 'Bumping atl/Cargo.toml alone turns repo-tests atl_lock red; anthology-shell in atl/Cargo.lock is a path dep'
metadata:
  type: project
  modified: 2026-11-22
---

crates/repo-tests/tests/atl_lock.rs test_version_matches_cargo_toml requires plugins/anthology/atl.lock version to equal atl/Cargo.toml. The release workflow needs Cargo.toml bumped before the tag, but atl.lock needs the release checksums, so a prep PR that bumps only Cargo.toml is red on that one test until the atl.lock PR lands. A past bump under the legacy name (chore(rtk): 1.2.1-anthology.4) touched Cargo.toml, Cargo.lock and DECISIONS.md only.
**Why:** the order is forced by the checksums; ask the lead how to treat the red test before opening the PR.
**How to apply:** also note atl depends on anthology-shell by path (atl/Cargo.toml), so the 0.1.0 line in atl/Cargo.lock is not a pin: the crate always builds from the workspace, and cargo test --locked proves the lock is consistent. Only the atl package version line in the lock needs editing. scripts/atl-test.sh uses fixed volume names and no container name or timeout, so for per-worktree runs write a temporary wrapper script with its own volumes and remove them after.
