---
name: 'rtk release prep: Cargo bump vs rtk.lock test'
description: 'Bumping rtk/Cargo.toml alone turns repo-tests rtk_lock red; anthology-shell in rtk/Cargo.lock is a path dep'
metadata:
  type: project
  modified: 2026-11-22
---

crates/repo-tests/tests/rtk_lock.rs test_version_matches_cargo_toml requires plugins/anthology/rtk.lock version to equal rtk/Cargo.toml. The release workflow needs Cargo.toml bumped before the tag, but rtk.lock needs the release checksums, so a prep PR that bumps only Cargo.toml is red on that one test until the rtk.lock PR lands. Past bump (chore(rtk): 1.2.1-anthology.4) touched Cargo.toml, Cargo.lock and DECISIONS.md only.
**Why:** the order is forced by the checksums; ask the lead how to treat the red test before opening the PR.
**How to apply:** also note rtk depends on anthology-shell by path (rtk/Cargo.toml), so the 0.1.0 line in rtk/Cargo.lock is not a pin: the crate always builds from the workspace, and cargo test --locked proves the lock is consistent. Only the rtk package version line in the lock needs editing. scripts/rtk-test.sh uses fixed volume names and no container name or timeout, so for per-worktree runs write a temporary wrapper script with its own volumes and remove them after.
