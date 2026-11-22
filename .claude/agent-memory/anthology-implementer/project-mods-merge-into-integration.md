---
name: 'Adding a mod PR to the mods integration branch'
description: 'Conflict spots when a mod PR merges into mods-integration, and which repo-tests it trips'
metadata:
  type: project
  modified: 2026-11-22
---

Conflicts land in the same files each time: CHANGELOG.md and README.md (keep both feature entries), plugin.json (keep the types line), mods/index.ts (header comment only; one import and one register call per feature), mods/util.ts (keep isolated() from the integration branch plus the PR helpers) and the MEMORY.md index (keep both note lines).
Only an event that two features hook goes through index.ts with isolated(); tool.check has one owner (permissions), so it stays in registerPermissions.
repo-tests docs.rs rejects a home-folder path with a real-looking user name in tracked files; the placeholder name user passes. Use it in mod tests and keep any relative twin of that path in step. PRUNED already lists the generated types folder on the integration branch.
Why: the same three repo-tests failures (PRUNED row, home path, node pin) hit each PR that was green only against its own base.
How to apply: after the merge run ci/jobs/mods.sh, then cargo test -p repo-tests --test docs --test repo_layout in the rust image.
