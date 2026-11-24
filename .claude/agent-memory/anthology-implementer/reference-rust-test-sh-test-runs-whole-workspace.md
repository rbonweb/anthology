---
name: 'rust-test.sh test runs the whole workspace twice, even with -p'
description: 'scripts/rust-test.sh test adds --workspace, so -p does not narrow it; use run cargo test -p for one crate'
metadata:
  type: reference
  modified: 2026-11-24
---

scripts/rust-test.sh test [ARGS] runs cargo test --workspace --locked ARGS and then again with --all-features, so passing -p <crate> still tests every crate, twice, and takes many minutes on a busy machine. For one crate use: scripts/rust-test.sh run cargo test --locked -p <crate> [--test <file>]. A golden check for one surface: scripts/dev.sh golden check --surface <name>. Why: the full difftest replay test (the_committed_goldens_replay_clean) alone can run over 10 minutes when other worktrees share the machine.
