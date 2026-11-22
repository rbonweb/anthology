---
name: 'Alerts mod (PR 169) review findings'
description: 'Gaps in hooks/mods/alerts.tsx at head 3220073a, proven by a merged P5-P8 tree run in Docker'
metadata:
  type: project
  modified: 2026-11-22
---

A throw after next(e) in an alerts.tsx hook, answered by its .catch next(e), drops what other anthology hooks on the same event wrote: in a merged P5+P6+P7+P8 tree, 15 afterwork tests lost the after-work block because alerts trackReport threw on an unmocked clock.now. Inside one plugin, .catch next(e) is memoized (a probe saw one spawn and one report), so it does not run beneath twice.
Two Notifications at once both alert (2 toasts, 2 sounds): lastAlertAt is checked across awaits. classic.Stop is the main loop Stop and clears pending prompts of background subagents.
The pane shows Bash commands, file paths and URLs of pending prompts, beyond design section 5 (agents plus PRs).
P6 head 1251f5cd reads anthology.coordinator but types/anthology.d.ts does not declare it; validate --strict fails on the merged tree.
Why: these are integration traps for any later mod sharing an event in the one module.
How to apply: when reviewing a mod that does work after next(e), check it wraps that work in try/catch; build a merged tree of all open mods branches and run claude plugin test before ruling a collision impossible. Tar with --no-xattrs on macOS or delete ._ files, or the kit fails to load them.
