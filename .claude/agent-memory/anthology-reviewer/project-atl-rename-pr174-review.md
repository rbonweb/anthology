---
name: 'ATL rename (PR 174) review findings'
description: 'What was checked and found in the rtk to ATL rename at head ced621b0, and how guard parity was proven from goldens'
metadata:
  type: project
  modified: 2026-11-22
---

Upgrade path at head ced621b0 was sound: reconcile treats a stored untagged add of Bash(rtk *) as tag token-layer and reverts it, plan() records no op when the rule already existed so a user rule is never touched, setup apply drops rtk_path then deletes the legacy data and history folders only after settings.json is written, and the preview lists them. Guard parity was proven without a build: decode base and head shell.analyze, shell.segments, guard.check and guard.hook goldens, map each atl row to its rtk twin (stdin_hex decoded for guard.hook) and compare expect; all 773 pairs matched and every base row was still present.
Open at that head: both anthology.lock files still pin anthology-bin 0.1.1, whose guard does not peel atl (atl run -c and atl bypass around npx playwright install were allowed, the rtk twins blocked), and no test forces the bump the way NEEDS_RELEASE_HASHES does for atl.lock. Minor: atl gain and atl/hooks/claude/atl-rewrite.sh still send users to upstream issues and install; a downgrade leaves the atl data and history folders because old remove only knows rtk.
Why: a later round of this PR or the release step should check these first.
How to apply: on re-review, confirm the anthology.lock bump to 0.2.0 and the enforcing test or a guard self-test sample written as an atl runner.
