---
name: github-ignores-union-merge
description: GitHub's merge and update-branch ignore the .gitattributes union rule, so MEMORY.md index edits show as conflicts; push a locally built merge commit instead
metadata:
  type: project
---

The repo's `.gitattributes` marks the generated memory indexes (`MEMORY.md`) as `merge=union`. Local git honours it; GitHub does not. A PR whose branch and `build` both added a line to the same index shows CONFLICTING, and `gh pr update-branch` fails with "Cannot update PR branch due to conflicts", while `git merge-tree --write-tree origin/build origin/<branch>` is clean.

**Why:** seen 2026-11-23 on the project-map PR and the guard docker-exec PR.

**How to apply:** when GitHub reports a conflict, run `git merge-tree --write-tree --name-only origin/<branch> origin/build` first. If it is clean, build the merge with `git commit-tree <tree> -p origin/<branch> -p origin/build -m "Merge branch 'build' into <branch>"` and push it to `refs/heads/<branch>`; no checkout needed. In zsh, write `${c}:refs/...`, because `$c:r` is a modifier.
