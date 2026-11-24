---
name: 'anthology-mem save --repo and reindex (PR 208) review findings'
description: 'Head 572b147a: write path and symlink checks hold; the --repo type and slug call sites are unpinned; reindex --all prints folder names raw'
metadata:
  type: project
  modified: 2026-11-24
---

Reviewed statically at head 572b147a (crates/anthology-memory/src/save.rs, index.rs, tests/save.rs). Sound: save --repo shares write_note with the agent save (ASCII-only slug, ensure_real_dir per component, create_excl or a symlink-refusing --replace, atomic MEMORY.md); check_note_text scans slug, name, description and body (body-file included); the root is git rev-parse --show-toplevel, so a linked worktree writes into itself; reindex checks every component with unsafe_dir before writing, and render reads frontmatter of regular files only. Gaps: only the kind == user line is pinned by a test; deleting the SAVE_TYPES check in save_repo_note_with lets --type User write a note that normalize_type reads as user, and no test catches it; deleting the check_slug call there also survives. reindex --all prints agent-memory folder names (untrusted, from the clone) without escaping. Like the agent save, a HOME that is a git work tree becomes the repo root, and reindex --all then rewrites the MEMORY.md files under the home .claude/agent-memory.
Why: a re-review can skip the parts already verified.
How to apply: on a fix round, delete the SAVE_TYPES and check_slug call sites in save_repo_note_with and confirm a test goes red.
