---
name: 'Anthology in SDK-embedded hosts (audit 2026-11-23)'
description: 'How Anthology behaves when another app drives Claude Code via the SDK; baseline for later audits'
metadata:
  type: project
  modified: 2026-11-23
---

Some of the owner's apps drive Claude Code through the SDK (entrypoint sdk-cli) and load every user-scope plugin, so Anthology runs there with nobody at the keyboard. Bot hosts seen so far load no user plugins at all.
Findings to re-check after fixes: G0 blocked every host CLI call whose program word is a quoted env var holding an absolute path (PE-1 carve-out covers only HOME); after G0 the agents moved commands into temp scripts run with sh, which the guard does not read (Opaque Source), so the habit removed guard visibility; session start injected the full routing guide, superpowers bootstrap, setup nag and personal-memory titles into one-shot JSON calls with no host check.
Pre-install design-app sessions are a clean baseline: same commands, no blocks, no script files.
Full report: .superpowers/sdd/2026-11-20-rust-port/audit-2026-11-23/other-projects.md
Why: the next audit can compare against these counts instead of starting over, and the host-detection open question decides most of the fixes.
