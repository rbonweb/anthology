---
name: 'Porting an rtk-hook fix to the atl-rename branch'
description: 'Where the hook logic and tests live after the rename, and the targeted-test limits in a worktree'
metadata:
  type: project
  modified: 2026-11-22
---

A fix to plugins/anthology/bin/rtk-hook ports to plugins/anthology/bin/atl-hook with the same text, atl names, and the tests go in crates/anthology-runtime/tests/atl_install.rs (wrapper_test module). Tests with a ledger pin keep the rtk-named Rust fn (port/ledger/test_rtk_install.tsv); new Rust-only tests take atl names. The repo-tests atl_rename done-check needs git inside the container; a worktree .git points outside the mount, so it cannot run there and is left to CI. Why: the diff of such a port is mechanical, and the only judgment is which names stay legacy. How to apply: copy the hook block and tests from the release branch, rename rtk to atl, run cargo test -p anthology-runtime --test atl_install in the pinned image plus shellcheck on the hook.
