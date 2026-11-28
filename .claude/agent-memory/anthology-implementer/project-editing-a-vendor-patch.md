---
name: 'Editing an existing vendor patch and re-syncing it'
description: 'Edit only the plus lines of vendor/patches/<name>.patch, fix the hunk header if the line count changes, then dev.sh vendor sync <name> --ref <tag>; the bare sync prints usage'
metadata:
  type: project
  modified: 2026-11-28
---

To change vendored text, edit the added lines of vendor/patches/<name>.patch in place. If the number of added lines changes, raise the new-side count in that hunk header (for example -44,8 +44,14 becomes +44,16); a later hunk in the same file would need its start shifted too. Then run scripts/dev.sh vendor sync <name> --ref <ref from vendor/sources.json> and scripts/dev.sh vendor check. Running vendor check right after the patch edit and before the sync is the red step: it reports the generated file differs from the build. The sync needs a name and --ref, and it re-clones upstream, so it takes a few minutes on a busy machine; sources.json stays unchanged when the ref is a tag. Run the two commands one after the other: pgrep -f patterns match your own wait shell, and a check run beside a sync reads half-written files. Plugin folder changes also need: version in plugin.json above the latest tag, the same plugin_version in the entry in vendor/sources.json (prior bumps changed both), and a fragment in changelog.d/<plugin>/<name>.md (the superpowers folder does not exist until a fragment creates it).
