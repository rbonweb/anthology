---
name: 'Where to verify project map claims'
description: 'Which branch holds each fact when the project map is refreshed before a PR lands'
metadata:
  type: project
  modified: 2026-11-22
---

The project map budget is 7000 characters for the whole file (wc -m); a refresh that adds pieces needs about 1000 characters of cuts elsewhere, so trim wording first.
Mods live on the mods-integration branch (plugins/anthology/hooks/mods/, types/anthology.d.ts under plugins/anthology/, ci/jobs/mods.sh). The perm classify command (crates/anthology-cli/src/cmd/perm.rs plus perm/) lives on release-0.1.5, its token and path lines are parsed by classifiedPaths in permissions.ts. atl_rename.rs and the post-rename names live on atl-rename. Release.sh and check-fresh behavior is in scripts/release.sh and crates/anthology-dev/src/lock/fresh.rs.
Why: none of these were on build when the map was refreshed, so each claim had to be read with git show from the remote branch.
How to apply: fetch those branches, read with git show origin/<branch>:<path>, and keep the map on the post-merge names.
