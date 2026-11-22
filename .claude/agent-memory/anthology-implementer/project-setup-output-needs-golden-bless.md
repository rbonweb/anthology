---
name: 'Changing setup or memory-sync output needs a golden bless'
description: 'Setup, session-start and memory-sync output is replayed from committed goldens in CI; bless them in the container after a deliberate change'
metadata:
  type: project
  modified: 2026-11-22
---

Any change to what `anthology setup`, the SessionStart hook or `memory::sync::run` prints or writes changes the frozen goldens (crates/anthology-runtime/tests/golden/setup.jsonl.gz, hook.session-start.jsonl.gz, crates/anthology-memory/tests/golden/memory.sync.jsonl.gz). CI test `the_committed_goldens_replay_clean` (difftest crate, Linux) fails until they are re-recorded.

Run in the pinned container, after the Rust code is final: `scripts/rust-test.sh run cargo run --locked -q -p difftest -- golden bless --surface setup --surface hook.session-start --surface memory.sync`. Check first with `golden check` in place of `bless` to see which surfaces differ.

Why: the Python oracle is gone, so the goldens are the only reference; bless records Rust as the new expected result and shows up as a binary diff in the PR.
