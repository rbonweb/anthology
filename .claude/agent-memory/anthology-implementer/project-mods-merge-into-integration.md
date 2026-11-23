---
name: 'Adding a mod PR to the mods integration branch'
description: 'Conflict spots when a mod PR or a build release or rename merges into mods-integration, and which repo-tests it trips'
metadata:
  type: project
  modified: 2026-11-23
---

Conflicts land in the same files each time: CHANGELOG.md and README.md (keep both feature entries), plugin.json (keep the types line), mods/index.ts (header comment only; one import and one register call per feature), mods/util.ts (keep isolated() from the integration branch plus the PR helpers) and the MEMORY.md index (keep both note lines).
Only an event that two features hook goes through index.ts with isolated(); tool.check has one owner (permissions), so it stays in registerPermissions.
repo-tests docs.rs rejects a home-folder path with a real-looking user name in tracked files; the placeholder name user passes. Use it in mod tests and keep any relative twin of that path in step. PRUNED already lists the generated types folder on the integration branch.
Merging build after the ATL rename: atl_rename.rs fails on any rtk word in mod code or mod tests (comments naming the rtk hook, a Bash(rtk *) rule fixture, the old history folder). Rename them to atl; only CHANGELOG and the memory folders are exempt. In CHANGELOG keep ONE Added and ONE Changed heading under Unreleased: move the mod entries into build headings by hand. The mods plugin version stays above build (0.4.0 over 0.3.x); guardrails and workspace versions come from build.
Why: the same three repo-tests failures (PRUNED row, home path, node pin) hit each PR that was green only against its own base, and the rename adds a fourth.
How to apply: after the merge run ci/jobs/mods.sh, then scripts/rust-test.sh test -p repo-tests (the raw rust image has no git, so tests that list tracked files fail there; the script installs git).
