---
name: 'Committing other agents'' memory notes to the public repo'
description: 'Notes from read-only agents sit untracked in the main checkout; scrub private details and hold back unfixed security notes'
metadata:
  type: project
  modified: 2026-11-22
---

Read-only agents (architect, researcher, reviewer) write notes into the main checkout, untracked. Copy them into the worktree, never edit the originals, and rebuild each MEMORY.md from the main checkout copy so notes already on build stay listed.
Scrub other-project session details down to kinds of problems, counts and fixes, and hold back notes that describe unfixed guard bypasses until the fix ships.
Why: the repo is public, and the main checkout index can list notes the build index lacks.
How to apply: grep the notes for home paths, emails and agent ids, then run the docs and cutover_docs tests; the agent-memory folder is not scanned for the word Python.
