---
name: 'Test fixtures must not contain home-style paths'
description: 'repo-tests docs test_no_real_home_paths fails on any tracked file with a home-folder path unless the name is a known placeholder'
metadata:
  type: project
  modified: 2026-11-22
---

A fake transcript or project path under a home folder in a Rust test fixture fails repo_content_test::test_no_real_home_paths (crates/repo-tests/tests/docs.rs). It showed up only in CI on macOS because cargo stops after the first failing test binary. Why: the check scans every tracked file for a Users or home folder followed by a name that is not in PLACEHOLDER_NAMES. How to apply: write fixture paths outside home folders (for example under /var/projects), and run cargo test -p repo-tests --test docs locally before pushing a test that holds paths.
