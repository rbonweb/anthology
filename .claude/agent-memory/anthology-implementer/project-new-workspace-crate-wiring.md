---
name: 'Adding a workspace crate: every list it must join'
description: 'test-weights row, shims-distros crate row, release workflow -p list, locks (write them in local Docker, not remote-build); weights test reads git ls-files'
metadata:
  type: project
  modified: 2026-11-28
---

Why: the slop gate Task 1 and the check-repo scan crate Task 1 each hit these; the plan named only some.
- ci/test-weights.tsv needs a row. ci_scope.rs the_weights_name_every_workspace_crate_once lists crates through git ls-files, so an unstaged new crate is missing from the left side and the test fails until git add.
- A crate that repo-tests links needs a crate:<name> job:shims-distros row in ci/scope-rules.tsv (the_shims_distros_crate_rows_name_every_workspace_crate_repo_tests_links).
- A crate that anthology-cli depends on joins the shipped graph: add -p <name> to the Test the shipped crates step of .github/workflows/anthology-release.yml (repo-tests anthology_lock.rs the_test_step_runs_exactly_the_shipped_graph).
- Workspace version bump: scripts/rust-test.sh run cargo update --workspace, then the same with --manifest-path fuzz/Cargo.toml; only version lines and the new entry change. Run these in local Docker: remote-build.sh deletes its copy at the end, so a lock written there never comes back. Run the update again after wiring the new crate into cli and repo-tests Cargo.toml, so their dependency lists are in the lock.
- Stub modules frozen ahead of their users need allow(dead_code) on the module (as cmd/mod.rs does), or clippy -D warnings fails.
- cargo fmt --all in local Docker writes the fixes back; remote-build fmt --check only reports.
