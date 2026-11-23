---
name: 'Container runs need the git common dir mounted'
description: 'Tests that call git in a worktree fail in the container unless the common git dir is mounted read-only'
metadata:
  type: reference
  modified: 2026-11-22
---

In a linked worktree the .git file points at the main checkout git dir. A container that mounts only the worktree root cannot run git there, and repo-tests, anthology-dev and the sync script tests fail. Mount the common dir (git rev-parse --path-format=absolute --git-common-dir) read-only at the same path.
**Why:** the rule is to mount only the worktree root, and the read-only common dir is the smallest addition that keeps git working.
**How to apply:** build the docker run line with the extra -v for that path; run as a non-root user, with CARGO_HOME and CARGO_TARGET_DIR under the gitignored target/ directory, a separate target dir per container name so cargo locks never clash, and cargo --locked. Test targets stop at the first failing target unless --no-fail-fast is given.
