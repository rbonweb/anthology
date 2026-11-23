---
name: 'Committing other agents'' memory notes to the public repo'
description: 'Notes from read-only agents sit untracked in the main checkout; scrub private details and hold back unfixed security notes'
metadata:
  type: project
  modified: 2026-11-22
---

Read-only agents (architect, researcher, reviewer) write notes into the main checkout, untracked. Copy them into the worktree, never edit the originals, and rebuild each MEMORY.md from the main checkout copy so notes already on build stay listed.
Scrub other-project session details down to kinds of problems, counts and fixes. Hold back every note that describes an unfixed guard bypass until the fix ships, even one the task list names: read each note for phrases like "Allow on build" or "bash runs it" (a shell-alias gap note was one). Drop the held-back notes' index lines too.
Once a released bin closes the gaps and committed tests prove it, the notes can go in with a one-line "Status: closed in <release>" under the frontmatter. Spot-check the named tests with grep before trusting the claim; index hooks sort by byte order (capitals first).
Why: the repo is public, and the main checkout index can list notes the build index lacks.
How to apply: grep the notes for home paths, emails and agent ids, then run the docs and cutover_docs tests; the agent-memory folder is not scanned for the word Python. The researcher index in the main checkout can lack lines for new researcher notes; leave it, the session-start sync regenerates it.
Also: scripts/dev.sh starts Docker even for --help, so read crates/anthology-dev/src/lib.rs for the command list instead.
