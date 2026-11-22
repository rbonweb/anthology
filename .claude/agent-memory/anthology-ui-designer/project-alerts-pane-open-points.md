---
name: 'Alerts pane: command name and double alert'
description: 'Why the status command is /anthology-status and why notify.sh may double the alert'
metadata:
  type: project
  modified: 2026-11-22
---

Commands made with $.command.register take letters, digits, _ and - only, so /anthology:status cannot be registered; the pane command is /anthology-status. A skill file would give the namespaced name.
The mod alert and hooks/notify.sh both fire on a Claude Code that loads mods, so macOS can sound twice. Design section 5 has a marker-file fix (mod writes a live marker, notify.sh exits early); it was left out of the P8 scope.
Why: both are decisions the owner may want to reverse.
