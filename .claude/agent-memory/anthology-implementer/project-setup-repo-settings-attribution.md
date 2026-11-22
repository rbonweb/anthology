---
name: 'Setup writes the attribution keys into a repo settings file'
description: 'Which keys setup adds to an approved repo .claude/settings.json, why object form not false, and how user values are protected'
metadata:
  type: project
  modified: 2026-11-22
---

Setup adds autoMemoryEnabled false and attribution.commit and attribution.pr set to empty strings to an approved repo committed .claude/settings.json (crates/anthology-runtime/src/setup.rs, attribution_ops and repo_plan_report). The object form was chosen over attribution false because false needs Claude Code 2.1.281 and older versions skip the WHOLE settings file that holds it, which would drop a teammate's committed permissions.
The settings Op::Set overwrites a differing value, so attribution_ops reads the file first and drops an op whose key holds a non-empty value (or a non-object parent), returning a note instead. Preview and apply report it as notes plus a fixed user_scope text under memory.repo.auto_memory (preview) or memory.auto_memory (apply).
Why: user-set values must survive apply and remove; reconcile keeps old records, revert only touches a value equal to the one setup set.
How to apply: changing the op list changes the setup golden only (bless --surface setup). A shipped-src change also needs the workspace version bump (Cargo.toml plus cargo update --workspace --offline) so a binary release can be tagged.
