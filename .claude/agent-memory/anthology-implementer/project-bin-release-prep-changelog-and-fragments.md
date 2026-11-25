---
name: 'Binary release prep: CHANGELOG has no bin section, fragments stay'
description: 'A bin bump PR edits Cargo.toml and both locks, adds one summary bullet under anthology Unreleased, and leaves changelog.d fragments for the plugin release'
metadata:
  type: project
  modified: 2026-11-24
---

CHANGELOG.md has sections per plugin only, so a bin release has no section of its own. The 0.2.2 prep (PR 216) put entries in the plugin Unreleased sections and changed no fragment. For 0.2.3 the prep adds one short summary bullet under the anthology Unreleased Changed heading and leaves changelog.d/* alone, because release.sh compiles Unreleased plus fragments at the plugin release and deletes them then. Copying fragment text into Unreleased would print it twice. Locks: scripts/rust-test.sh run cargo update --workspace --offline, and the same with --manifest-path fuzz/Cargo.toml; expect 22 and 10 changed lines. No test pins the workspace version; plugins/*/anthology.lock stays on the old version until the lock PR. When a monitor waits on pgrep -f, write the pattern as [d]ev.sh so the monitor own command line does not match and loop forever.
