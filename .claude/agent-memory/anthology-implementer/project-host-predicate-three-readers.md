---
name: 'Host rule: three readers, one truth table in delegation.test.ts'
description: 'Where the embedded-host rule lives and how the three readers are pinned'
metadata:
  type: project
  modified: 2026-11-24
---

The embedded-host rule (ADR-037 and its amendment) has three readers: crates/anthology-guard/src/host.rs, the delegation mod isEmbeddedHost, and the superpowers session-start hook (edited only through vendor/patches/superpowers.patch, then scripts/dev.sh vendor build superpowers; a patch hunk header count must grow with the lines, and the version bump goes in vendor/sources.json plugin_version, not plugin.json). The one truth table sits between the host-truth-table markers in plugins/anthology/tests/mods/delegation.test.ts: the mods job runs it against the mod, and crates/repo-tests/tests/host_predicate.rs parses it and runs it against host::embedded and the real hook. Change the rule: edit the table first, watch both go red, then fix the three readers. Why: the rule drifted once between readers (sdk- entrypoint beat attended 1 in all three). Attended is read as a decider only when exactly 0 or 1; any other value falls through to the entrypoint. The stale-plugin check in session_start.rs compares the loaded root only with the entry whose installPath is the root (same scope and projectPath); no path match means all applicable entries. Rc suffixes order by number, other prerelease tags (shas) are unordered.
