---
name: 'Docs tests: plain rust image, and no "Python" in docs'
description: 'How to run docs and cutover_docs tests with only the worktree mounted, and the word rule they enforce'
metadata:
  type: project
  modified: 2026-11-22
---

Run the docs tests with: docker run --rm -v "$PWD":/w -w /w <FROM line of ci/images/rust.Dockerfile> sh <script>, where the script runs rustup toolchain install, sets CARGO_TARGET_DIR=/target and runs cargo test --locked -p repo-tests --test docs --test cutover_docs. Takes about a minute and leaves no volumes.
**Why:** scripts/rust-test.sh also mounts the main checkout git dir read-only; the docs tests fall back to a file walk without git, so mounting only the worktree works.
cutover_docs fails if a doc other than the allowed ones mentions Python. Write "0.1.x layout" style wording instead.
**How to apply:** any docs edit to CONTRIBUTING.md, README.md, PLAN.md. cargo stops at the first failing test binary, so fix cutover_docs before expecting docs to run.
