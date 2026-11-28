---
name: 'Slop gate S4: word-grain edit replay, 5 s hook deadline, exempt names'
description: 'Markdown edits mark changed words only (code keeps lines), the hook runs rules on a worker thread with a 5 s deadline, check skips nested clones, legal file names are exempt'
metadata:
  type: project
  modified: 2026-11-28
---

crates/anthology-slop/src/edit.rs: apply_with(before, edit, Grain) with Grain::Words for Markdown and Grain::Lines for Html, Jsx and Css (grain(kind)); apply() is the lines replay. Words: lines are matched as in added_lines, each unmatched new line is paired with the most similar removed line within an 8-line window (at least half its words shared, a removed line over 4x+256 bytes longer is not tried so cost stays linear), then a word diff (common head and tail, LCS table capped at 90000 cells, else all middle words changed) marks only changed words. An unpaired line is new whole. Why: review-2 I2, one paragraph per line made a one-word fix re-flag an old tell.
crates/anthology-cli/src/cmd/slop.rs: hook_within runs edit replay plus rules on a worker thread (8 MiB stack) and waits HOOK_DEADLINE (5 s) on an mpsc channel; timeout allows with one stderr line (SKIPPED), a panic or failed spawn allows silently. Tests inject a slow fn pointer (Rules type). slop-gate.sh drops stderr on exit 0, so the skip line only shows when the binary is run directly. check: walk skips a subfolder holding a .git FOLDER (a clone), not a .git file; shown paths are relative to --root or the cwd when under it.
paths.rs: EXEMPT_DIRS has testdata and __mocks__; EXEMPT_NAME_PREFIXES (license, licence, copying, notice, disclaimer, code_of_conduct, prefix match on lower-cased file name) exempts legal files. Tests run in a session container; the anthology-cli crate is bin-only so use cargo test -p anthology-cli --bins --test slop.
