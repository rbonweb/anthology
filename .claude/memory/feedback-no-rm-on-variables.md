---
name: no-rm-on-variables
description: Never run rm or rmdir on a variable path (rm -rf $x); it makes Claude Code prompt the owner even in bypass mode
metadata:
  node_type: memory
  type: feedback
  originSessionId: 1c9fb279-a45d-4d27-8e78-1ddd82c1e344
  modified: 2026-11-22T03:45:00.951Z
---

Don't write `rm -rf $x`, `rm -rf "$d"` and the like in Bash commands. Leave mktemp folders under /tmp alone, or delete only literal paths.

**Why:** on 2026-11-22 the owner was asked for permission again and again while in bypass mode. Claude Code still asks for removals that might hit a critical path, and a variable path can't be proven safe. The owner: "I already put you in a bypass permission mode, so why are you asking again and again?"

**How to apply:** for cleanup, use `git worktree remove`, `docker rm` with named containers, or nothing at all for /tmp. When briefing agents, pass the same rule; the Anthology version of it is queued in [[after-v010-first-task]].
