---
name: 'Docker cargo runs in a linked worktree: git config fails inside the mount'
description: 'The worktree .git file points outside the mount, so git config --global in the container fails unless run from another folder'
metadata:
  type: reference
  modified: 2026-11-22
---

In a linked worktree /w/.git is a file pointing at the main checkout's git dir, which is not mounted. Any git command run with /w as the working directory dies with fatal: not a git repository, including git config --global. Put the container setup in a script that cd / before git config, then cd /w, and keep the script under the gitignored target/ folder so it is never committed. A compound sh -c string that names git is also refused by the worktree command check, which is another reason to use a script file. Also set DEBIAN_FRONTEND=noninteractive for apt. Tests that make their own temp repos work fine once git is installed.
Why: cost two failed runs of minutes each on a busy machine.
