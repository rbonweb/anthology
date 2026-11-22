---
name: 'Probing a PR without touching the worktree'
description: 'How to run hook probes and targeted cargo tests for a PR head in Docker without checking it out'
metadata:
  type: project
  modified: 2026-11-22
---

To test a PR head without editing the checkout: git fetch origin pull/<N>/head:pr<N>, then pipe git archive pr<N> into the pinned image from ci/images/rust.Dockerfile with only the shared anthology-cargo and anthology-rustup-<digest> volumes, CARGO_TARGET_DIR=/tmp/target inside the container. Targeted runs (one test binary per crate) take a few minutes from cold. Shell hooks (plugins/anthology/bin/atl-hook, plugins/anthology/hooks/*.sh) can be probed in debian:bookworm-slim and alpine:3.20 with a fake atl binary named in a temp CLAUDE_CONFIG_DIR/anthology/state.json.
Why: the reviewer may not edit files or check out branches, and scripts/rust-test.sh mounts the current worktree, which is not the PR head.
