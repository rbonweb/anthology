---
name: 'Alerts pane: command name and rulings'
description: 'Why the status command is /anthology-status, and the owner rulings on sound and privacy'
metadata:
  type: project
  modified: 2026-11-22
---

Commands made with $.command.register take letters, digits, _ and - only, so /anthology:status cannot be registered; the pane command is /anthology-status. A skill file would give the namespaced name.
Rulings on P8 review: the mod plays no sound (notify.sh keeps the only one); the pane and toasts never show a command, file_path, path or URL; the gh PR and CI block is deferred; engines is >=2.1.330.
A mutation check showed the kit does not reproduce the lost-result bug when a hook throws after next: removing the try/catch left the test green. The guard is the quietly() wrapper in plugins/anthology/hooks/mods/alerts.tsx, not the test.
Why: decisions the owner may want to reverse, and a test that proves less than it seems.
