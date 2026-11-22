---
name: 'Workspace version bump must also update fuzz/Cargo.lock'
description: 'A Cargo.toml workspace version bump leaves fuzz/Cargo.lock stale; the weekly fuzz job fails on cargo metadata --locked'
metadata:
  type: project
  modified: 2026-11-22
---

Check fuzz/Cargo.lock in any PR that changes [workspace.package] version. fuzz is excluded from the workspace, so cargo test --workspace does not notice, and PR CI does not run it.
Why: ci/jobs/fuzz.sh runs cargo metadata --locked and fails on a stale lock. Precedent: commit 3ab586cd had to follow the 0.1.1 bump. PR 155 bumped to 0.1.3 and left it at 0.1.1.
Also: vendor check cannot run from a linked worktree (git fetch fails on the .git file); run it from a git archive of the head, git init and commit in a temp dir, then scripts/dev.sh vendor check, and remove that tree volume anthology-rust-target-<cksum of path> afterwards.
