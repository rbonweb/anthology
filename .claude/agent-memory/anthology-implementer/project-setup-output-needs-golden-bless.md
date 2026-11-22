---
name: 'Changing setup or memory-sync output needs a golden bless'
description: 'Setup, session-start and memory-sync output is replayed from committed goldens in CI; bless them in the container after a deliberate change'
metadata:
  type: project
  modified: 2026-11-22
---

Any change to what `anthology setup`, the SessionStart hook, `memory::sync::run` or doctor prints or writes changes the frozen goldens (crates/anthology-runtime/tests/golden/*.jsonl.gz, crates/anthology-memory/tests/golden/memory.sync.jsonl.gz). CI test `the_committed_goldens_replay_clean` (difftest crate, Linux) fails until they are re-recorded.

Run in the pinned container, after the Rust code is final: `scripts/rust-test.sh run cargo run --locked -q -p difftest -- golden bless --surface setup --surface hook.session-start --surface memory.sync`. Check first with `golden check` in place of `bless` to see which surfaces differ.

Why: the goldens are the only reference for these surfaces now, so bless records the current output as the new expected result and shows up as a binary diff in the PR. Also: repo-tests cutover_docs scans memory notes, so never write the word for the retired reference implementation in a note.
