---
name: 'rust-test.sh volume clean-up, gc and sccache: rulings and test seams'
description: 'One-shot runs and session-stop drop the target volume; gc heals leaks; sccache caches deps only; how the stub tests read it'
metadata:
  type: project
  modified: 2026-11-27
---

scripts/rust-test.sh: a one-shot run (no ANTHOLOGY_RUST_SESSION) and session-stop remove anthology-rust-target-<cksum of root> unless ANTHOLOGY_KEEP_TARGET is set and not 0. Sessions stay warm; one-shots are cold by default. gc (also run first by session-start) removes only dangling anthology-rust-target-* volumes whose worktree is not in git worktree list (a folder that is gone counts), or any dangling target or anthology-atl-target volume older than ANTHOLOGY_GC_HOURS (default 48). Age is read from docker volume inspect CreatedAt with awk (offsets and fractions handled). Never touches anthology-cargo, anthology-rustup-*, anthology-sccache. sccache 0.10.0-* from Debian trixie, volume anthology-sccache at /sccache, off with ANTHOLOGY_SCCACHE=0 and in CI. It caches deps only: workspace crates are incremental and non-cacheable by design (280 of 363 compile requests in a workspace build). Measured on a 14-core arm64 Mac: cargo test --workspace --no-run with an empty target took 37.0 s without sccache, 38.5 s with a cold cache, 29.8 s with a warm one. A one-shot test -p anthology-guard is about 16 s of apt setup plus about 35 s of test run, so compile is a small share. Why it matters: stub-docker tests in crates/repo-tests (rust_test_script.rs, rust_test_jobs.rs) keep only the last docker call, so any new test that reads the cargo run through a one-call stub must set ANTHOLOGY_KEEP_TARGET=1, or the volume rm call hides it.
