---
name: 'Running cargo for this repo in a container'
description: 'Pinned-image docker recipe for shell, guard and atl tests, fmt and clippy, and the traps in it'
metadata:
  type: reference
  modified: 2026-11-22
---

Image: the FROM line of ci/images/rust.Dockerfile (a rust:1.100-slim digest that may show as <none> locally). Run from the worktree root: docker run --rm -v "$PWD":/w -w /w -e RUSTUP_TOOLCHAIN=1.100.0 -e CARGO_HOME=/w/target/cargo-home -e CARGO_TARGET_DIR=/w/target IMAGE sh -c CMD. /target is gitignored, so build output and the cargo cache stay inside the worktree.
Traps: the slim image has no rustfmt or clippy, so run rustup component add rustfmt clippy first. atl/ is its own workspace: use its own target dir (CARGO_TARGET_DIR=/w/target/atl-t), cargo --locked, and never cargo fmt --all there (it fails on a missing generated module and reformats unrelated files in path deps); run rustfmt on the files you touched instead. Warnings are errors in the atl lib, so a sed mutant that leaves an unused variable or an unreachable pattern shows as does-not-compile, not as killed.
Why: first container run costs minutes of rediscovery; the shell tests alone build in seconds.
