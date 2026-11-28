---
name: 'Design-feedback lead rules (PR 325) review findings'
description: 'Head bd656c7e: vendor check clean, guide at 4050 cap; gaps are banned-names list in repo memory, identity Ruling vs never-default, act-only-on-asks scope, literal brand words, signed-in capture'
metadata:
  type: project
  modified: 2026-11-28
---

PR 325 (W2 of the 2026-11-28 design-feedback wave) adds a Design and UI work section and process bullets to plugins/anthology/skills/large-builds/SKILL.md, the "before merge" edit to plugins/anthology/templates/routing-guide.md, and an F9 edit to vendor/patches/website-clone.patch.
Checked at head bd656c7e: scripts/dev.sh vendor check is clean from a detached scratch worktree (dev.sh resolves the repo root from its own path, so it can be run by absolute path without cd); the guide is exactly 4050 characters, the cap in crates/anthology-runtime/tests/session_start.rs.
Findings: a banned-names list kept in project memory puts owner and legal names into committed .claude/memory, and anthology-mem save refuses user names, so it belongs in a user note; "never default" identity facts contradicts "record a Ruling per gap" while the owner is away (safe default is to omit); "act only on asks" reads against Keep capacity busy; website-clone now tells agents to render the literal words "the user's brand"; a like-X capture runs in the signed-in BrowserOS with section 2 exercising primary links and no read-only limit.
Why: the same rule kinds (identity, privacy, signed-in browser actions, rule scope) recur in every lead-rule PR, so check them first.
