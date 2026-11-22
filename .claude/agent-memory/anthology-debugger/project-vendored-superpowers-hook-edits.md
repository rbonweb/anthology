---
name: 'Editing the vendored superpowers plugin'
description: 'How to change plugins/anthology-superpowers files so vendor check stays clean, and how to run targeted tests'
metadata:
  type: project
  modified: 2026-11-22
---

plugins/anthology-superpowers is generated from upstream plus vendor/patches/superpowers.patch. A direct edit to a file there fails `scripts/dev.sh vendor check` unless the patch carries it.
How to apply: edit the file, then run `scripts/dev.sh vendor diff superpowers > vendor/patches/superpowers.patch` (stdout is the whole patch; git diff must show only your hunk), bump the version with `scripts/dev.sh release-edit set-version plugins/anthology-superpowers/.claude-plugin/plugin.json <v>` AND `plugin_version` in vendor/sources.json, then `scripts/dev.sh vendor check` must say clean.
Why: hand-typed patch hunks lose the single-space context lines and break; regenerating avoids that.
Targeted tests: `scripts/rust-test.sh run cargo test -p <crate> --locked --test <file>` runs one test file in the pinned image with a per-checkout target volume; `scripts/dev.sh golden check|bless --surface <name>` replays or re-blesses a golden. The rust-test.sh docker run has no --name, so put a small docker shim first on PATH to name the container.
