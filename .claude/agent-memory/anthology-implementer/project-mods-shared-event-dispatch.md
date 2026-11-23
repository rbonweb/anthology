---
name: 'Mods: shared events are registered once in index.ts'
description: 'agent.report is hooked by afterwork and alerts; index.ts registers it once and isolates each part; how the guard is pinned'
metadata:
  type: project
  modified: 2026-11-22
---

plugins/anthology/hooks/mods/index.ts registers agent.report once: next(e) once, then afterworkReport and alertsReport each inside isolated() from util.ts. Features export their part (a plain function taking $, e and the core result) instead of registering the shared event. Why: a second on() of one event throws and unloads the whole module, and a throw after next makes the hook .catch call next again and drop the other feature's result. Pinned by plugins/anthology/tests/mods/integration.test.ts (clock down, state down, after-work part throwing). Mutation: dropping isolated() around alertsReport turns the clock and state tests red; dropping it around afterworkReport turns the third red. Removing quietly() in alerts.tsx alone turns nothing red now, since the report path no longer uses it. Dev loop: keep one node container with npm ci done once and docker exec claude plugin test plugins/anthology (3 s per run, instead of 3 min through ci/jobs/mods.sh). The afterwork tests do not stub clock.now, so they pass on the merged module only because isolated() swallows the failing alerts part.
