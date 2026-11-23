---
name: 'Measuring a macOS number with the host cargo'
description: 'Host has Homebrew cargo 1.100; build into target/host with CARGO_HOME under target so nothing lands in the home folder or clashes with the Linux container target'
metadata:
  type: reference
  modified: 2026-11-22
---

For a macOS-only figure (the perm classify latency target is macOS): CARGO_TARGET_DIR=<worktree>/target/host CARGO_HOME=<worktree>/target/cargo-home-host cargo build --release --locked --manifest-path <worktree>/Cargo.toml -p anthology-cli. A separate target dir keeps the Linux artifacts of the container run apart. Measure through the real plugins/anthology/bin/anthology shim with CLAUDE_CONFIG_DIR pointing at a scratch folder holding anthology/bin/<lock version>/anthology plus a non-empty .sha256; the shim alone costs about 15 to 20 ms. Measure with no container running, or the numbers double.
Why: the design states latency targets for macOS, and the container cannot give them.
