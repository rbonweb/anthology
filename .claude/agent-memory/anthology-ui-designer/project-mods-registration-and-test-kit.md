---
name: 'Mods: registration and test-kit rules'
description: 'How on() duplicates, void ops, nested Text and state contracts behave in claude plugin test'
metadata:
  type: project
  modified: 2026-11-22
---

A second on() of the same event with the SAME matcher (or both with none) throws "registered twice" and the whole module fails to load. Different matchers on one event are fine. plugins/anthology/hooks/mods/alerts.tsx gives agent.report a status matcher so afterwork.ts can register it matcherless.
In plugins/anthology/tests/mods, a test registers each event once (a second on() throws), so branch inside one stub. A void op ($.ui.toast, $.ui.status, $.audio.play) must answer { value: undefined }; { } is refused.
ui.mount find() text is only an element own string children, so read nested Text rows from ui.drawn() and join children recursively.
Every $.state key must be declared in plugins/anthology/types/anthology.d.ts (plugin.json "types") inside the one anthology object, or validate refuses it.
Why: each of these cost a failed ci/jobs/mods.sh run (about 2 minutes each).
