---
name: 'Release integration branch: golden replay trick and CI traps'
description: 'Reading a big golden diff by replaying another PR golden; real-home-path test; empty macOS failure; CHANGELOG after a release lands'
metadata:
  type: project
  modified: 2026-11-22
---

To explain a large golden diff after merging a PR that blessed its own golden, copy that PR golden over the file and run golden check for the surface: only the changes from other PRs remain, so the rest is proven to be the PR own. Restore or bless afterwards. Use awk (not grep, which the rtk hook truncates) to pull the diff lines from the check output.
crates/repo-tests docs.rs test_no_real_home_paths fails the Linux and macOS jobs on any /home/<name>/ text in a public file, for example a fake transcript_path in crates/anthology-runtime/tests/session_start.rs; use a path that is not a home path.
A failed rust-test job shows an empty log until the whole run completes, so wait for the run, not the job. gh run rerun --job re-runs one job.
After build releases a version, take build CHANGELOG as the base (git checkout --theirs during the merge) and re-add only this branch own entries under Unreleased; merging by hand keeps entries inside the released section.
Why: each of these cost a CI round or an hour of guessing on release 0.1.5.
