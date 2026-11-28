---
name: 'macOS cwd is the resolved path; slop-only PRs skip macOS CI'
description: 'Why a path test passed on Linux and failed on macOS temp dirs, and how to make CI run macOS for a PR'
metadata:
  type: project
  modified: 2026-11-28
---

std::env::current_dir (getcwd) returns the resolved path, so on macOS a temp dir given as /var/... is reported as /private/var/.... Code that compares a user-given absolute path with the cwd by prefix (slop check, crates/anthology-cli/src/cmd/slop.rs shown_below) must compare the resolved folders when the plain prefix fails. Linux has no such link, so only a test that builds its own symlink (check_shows_an_absolute_path_through_a_link_from_the_cwd) catches it off macOS. A PR that touches no platform-sensitive path gets no macOS shard (ci/scope-rules.tsv), so add the full-ci label to the PR to have CI confirm a macOS-only fix. Why: PR 323 broke the macOS shards and Linux CI stayed green. Also: remote-build.sh runs can fail with a rustup clippy conflict, exit 137 or a missing remote folder when another agent shares the host; re-run.
