---
name: 'autoMemoryEnabled false in a repo and the memory sync step'
description: 'How setup''s repo-level autoMemoryEnabled false interacts with memory::sync::run and why the index is injected every session'
metadata:
  type: project
  modified: 2026-11-22
---

Setup writes autoMemoryEnabled: false into an approved repo's committed .claude/settings.json (state.json key repo_settings records it for remove). memory::sync::auto_memory_off used to treat that value as a user opt-out and return nothing, which would have switched the whole memory step off (no personal section, no hint). It now ignores a false in the committed settings.json (local settings, user settings and CLAUDE_CODE_DISABLE_AUTO_MEMORY still opt out) and sync_repo injects the repo memory index every session while that switch is on, because Claude Code no longer loads it.

Why: with Claude Code's own auto memory off, only the SessionStart hook can put repo memory into context.

How to apply: touch auto_memory_off or sync_repo only together with the tests in crates/anthology-memory/tests/auto_memory_by_type.rs. Session start still copies all old auto-memory notes into personal memory (ledger by sha); setup --move-old-notes puts them in the repo first and ledgers them so they are not copied twice.
