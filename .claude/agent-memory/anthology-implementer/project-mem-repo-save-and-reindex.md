---
name: 'anthology-mem save --repo and reindex: where the code is'
description: 'Repo notes reuse check_note_text and write_note in save.rs; --slug is optional (slug_from_name); help text drift blesses the mem golden'
metadata:
  type: project
  modified: 2026-11-28
---

save --repo and reindex share crates/anthology-memory/src/save.rs: write_note is the common tail of an agent save and a repo save, check_note_text is the one privacy and secret gate, and reindex walks .claude/memory plus the roster agents folders (never names read from disk, so nothing escaped is needed) and refuses any symlinked folder before writing. The CLI parser, run_save, run_reindex and the home-folder refusal (refuse_home_repo, for save --repo and reindex --repo/--all) are in crates/anthology-memory/src/index.rs. Why it matters: a new route into repo memory must call check_note_text and the SAVE_TYPES and slug checks (tests pin each by spelling) or it bypasses the privacy scan or writes outside the folder.

--slug is optional (2026-11): run_save calls save::slug_from_name(name) when the flag is absent (ASCII lowercase, runs of other characters become one dash, cut at DERIVED_SLUG_CHARS = 60, dashes trimmed) and refuses an empty result with a message asking for --slug. An explicit --slug, even an empty one, is passed through unchanged, and the derived slug goes through the same check_slug, secret and privacy checks, so the email refusal on the name still applies. The save help text (description plus per-flag help) says which flags are required; tests/save.rs pins it.

Changing the save or reindex flags or their help changes the save help, so only mem seed:7 drifts; bless it with scripts/dev.sh golden bless --surface mem (a few minutes, run it in the background). To test one crate quickly: scripts/rust-test.sh run sh -c "cargo build -p anthology-cli --locked && cargo test --locked -p anthology-memory"; plain test -p does not build the binary, so memory_index tests fail with no anthology binary.
