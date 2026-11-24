---
name: 'anthology-mem save --repo and reindex: where the code is'
description: 'Repo notes reuse check_note_text and write_note in save.rs; reindex checks every folder for symlinks first; help text drift blesses the mem golden'
metadata:
  type: project
  modified: 2026-11-24
---

save --repo and reindex share crates/anthology-memory/src/save.rs: write_note is the common tail of an agent save and a repo save, check_note_text is the one privacy and secret gate, and reindex walks .claude/memory plus agent folders and refuses any symlinked folder before writing anything. The CLI parser and run_reindex are in crates/anthology-memory/src/index.rs. Why it matters: a new route into repo memory must call check_note_text (user notes are refused first) or it bypasses the privacy scan. Changing the save or reindex flags changes the help text, so only mem seed:7 drifts; bless it with scripts/dev.sh golden bless --surface mem (it takes several minutes, run it in the background). To test one crate quickly: scripts/rust-test.sh run sh -c "cargo build -p anthology-cli --locked && cargo test --locked -p anthology-memory --test save"; plain test -p adds --workspace and runs everything.
