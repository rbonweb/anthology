---
name: 'CI job scoping (PR 271) review findings'
description: 'Head 03d689c4: job trigger paths checked against each job script; gaps are macOS repo-tests drop and the shims-distros musl build graph'
metadata:
  type: project
  modified: 2026-11-27
---

PR 271 targets the build branch, not main. Local origin/main can be stale, so diff against the PR baseRefOid (gh pr view --json baseRefOid), not origin/main.
Checked at head 03d689c4: every job trigger list in ci/scope-rules.tsv matches what its ci/jobs script or inline ci.yml command reads (ledger, mods, cli-syntax, startup-budget, vendor-check, version-bump, memory-diff, lock-fresh, rust-lint, plugin-validate). No branch protection or ruleset exists, so no check is required today.
Gaps found: (1) on macOS a PR that hits a macos rule but no shims effect drops repo-tests, which holds non-shim shell tests (host_predicate runs plugins/anthology-superpowers/hooks/session-start under /bin/bash 3.2, superpowers_session_start, remote_build, atl_rename_script); a hook-only edit never runs on macOS (no macos effect on that path, pre-existing). (2) shims-distros builds repo-tests for musl, which links anthology-dev, anthology-memory and anthology-guard, but the crate rows list only core, pycompat and testkit. Why it matters: a wrong skip shows up only on the push to build.
