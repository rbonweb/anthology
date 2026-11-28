---
name: 'Editing an existing vendor patch and re-syncing it'
description: 'Edit only the plus lines of vendor/patches/<name>.patch, keep hunk line counts, then dev.sh vendor sync <name> --ref <tag>; the bare sync prints usage'
metadata:
  type: project
  modified: 2026-11-28
---

To change vendored text, edit the added lines of vendor/patches/<name>.patch in place and keep each hunk line count unchanged, then run scripts/dev.sh vendor sync <name> --ref <ref from vendor/sources.json> and scripts/dev.sh vendor check. The sync needs a name and --ref, and it re-clones upstream, so it takes a few minutes on a busy machine; sources.json stays unchanged when the ref is a tag. Run the two commands one after the other: pgrep -f patterns match your own wait shell. Why: the first try without arguments only printed usage after a long container start, and a check run beside a sync reads half-written files.
