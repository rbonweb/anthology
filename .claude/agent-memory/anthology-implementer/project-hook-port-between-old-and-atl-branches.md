---
name: 'Porting a hook fix from the old-name branch to atl-rename'
description: 'Where the hook logic and tests live after the rename, and the targeted-test limits in a worktree'
metadata:
  type: project
  modified: 2026-11-22
---

A fix to the old-name hook in plugins/anthology/bin ports to plugins/anthology/bin/atl-hook with the same text and atl names, and the tests go in crates/anthology-runtime/tests/atl_install.rs (wrapper_test module). Tests with a ledger pin keep their old-name Rust fn (port/ledger/test_rtk_install.tsv); new Rust-only tests take atl names. The repo-tests atl_rename done-check scans every tracked file, memory notes included, so a note must not spell the old tool name outside a line that says legacy. It also needs git inside the container, and a worktree .git points outside the mount, so it cannot run there: grep your diff for the old name instead. Why: the macOS CI job failed on exactly this note. How to apply: copy the hook block and tests from the release branch, rename to atl, run cargo test -p anthology-runtime --test atl_install in the pinned image plus shellcheck on the hook.
