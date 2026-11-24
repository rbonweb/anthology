---
name: 'anthology-mem save --repo and reindex: where the code is'
description: 'Repo notes reuse check_note_text and write_note in save.rs; reindex lists roster folders only and refuses a home-folder repo; help text drift blesses the mem golden'
metadata:
  type: project
  modified: 2026-11-24
---

save --repo and reindex share crates/anthology-memory/src/save.rs: write_note is the common tail of an agent save and a repo save, check_note_text is the one privacy and secret gate, and reindex walks .claude/memory plus the roster agents folders (never names read from disk, so nothing escaped is needed) and refuses any symlinked folder before writing. The CLI parser, run_reindex and the home-folder refusal (refuse_home_repo, for save --repo and reindex --repo/--all) are in crates/anthology-memory/src/index.rs. Why it matters: a new route into repo memory must call check_note_text and the SAVE_TYPES and slug checks (tests pin each by spelling) or it bypasses the privacy scan or writes outside the folder. Changing the save or reindex flags changes the help text, so only mem seed:7 drifts; bless it with scripts/dev.sh golden bless --surface mem (several minutes, run it in the background). To test one crate quickly: scripts/rust-test.sh run sh -c "cargo build -p anthology-cli --locked && cargo test --locked -p anthology-memory"; plain test -p adds --workspace and runs everything.
