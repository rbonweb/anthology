---
name: 'What a tool.check allow skips in Claude Code 2.1.330'
description: 'A mod allow over core ask pre-empts PermissionRequest hooks, the SDK host callback, auto-mode tool.judge and the headless deny; approver and session.version help gate it'
metadata:
  type: reference
  modified: 2026-11-22
---

From the 2.1.330 claude-code.d.ts (ToolCheckInput, ToolCheckResult, EngineEventOf tool.check and tool.judge): tool.check fires before the mode settles an ask, so a hook allow over an unmarked core ask skips whoever would have settled it: the dialog, a PermissionRequest hook (managed ones included), the SDK host callback (approver host), auto mode tool.judge classifier, and the dontAsk or headless deny (approver absent). Only plan mode and tools only the classifier may allow ignore a hook allow. Org controls over mods are allowManagedModsOnly and allowManagedHooksOnly; allowManagedPermissionRulesOnly does not stop a mod. e.approver is person, host or absent. $.session.version() gives the engine version for gating. S1 spike: tool.check loads on 2.1.311, and engines is ignored before 2.1.323, so a tool.check-only module runs on old builds.
Why: found in the PR 164 security review (permission-levels mod, 2026-11-22); the lift-an-ask design rests on these.
How to apply: in any review of a mod that answers allow, check approver, mode, org policy and the oldest build the module can load on, not only the verdict markers.
