---
name: 'Changing setup or memory-sync output needs a golden bless'
description: 'Setup, session-start and memory-sync output is replayed from committed goldens in CI; bless them in the container after a deliberate change'
metadata:
  type: project
  modified: 2026-11-24
---

Any change to what `anthology setup`, the SessionStart hook, `memory::sync::run` or doctor prints or writes changes the frozen goldens (crates/anthology-runtime/tests/golden/*.jsonl.gz, crates/anthology-memory/tests/golden/memory.sync.jsonl.gz). CI test `the_committed_goldens_replay_clean` (difftest crate, Linux) fails until they are re-recorded.

Run in the pinned container, after the Rust code is final: `scripts/rust-test.sh run cargo run --locked -q -p difftest -- golden bless --surface setup --surface hook.session-start --surface memory.sync`. Check first with `golden check` in place of `bless` to see which surfaces differ.

An optional key that appears only in a state no recorded case reaches leaves the golden unchanged: the setup preview key `memory.repo.repos_below` (parent folder holding repos) is such a case, so check first and expect a no-op bless, with no binary diff to explain.

Why: the goldens are the only reference for these surfaces now, so bless records the current output as the new expected result and shows up as a binary diff in the PR. Also: repo-tests cutover_docs scans memory notes, so never write the word for the retired reference implementation in a note.
