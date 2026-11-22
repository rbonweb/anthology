---
name: 'claude plugin test quirks for mods'
description: 'How the plugin test kit treats throwing hooks, settings.read, state and .catch, found while testing fail-open mods'
metadata:
  type: project
  modified: 2026-11-22
---

In claude plugin test, a test hook that throws is skipped, not rethrown. The kit answers $.state itself, so to make state.get or state.set fail return { deny: "x" } cast to never from the test hook; for events with no kit bottom (env.get, tool.list, session.root) a throwing or absent hook rejects the plugin call.
The settings.read event answers the WHOLE settings object ({ value: { key: v } }); $.settings.read({ key }) is that object projected, so a test hook that returns a bare value is skipped.
Removing a .catch(($, e, next) => next(e)) never turns a fail-open test red, because the engine already skips a throwing hook. To prove a fail-open test is not vacuous, mutate the catch to answer something else (for example .catch(() => ({ deny: "closed" }))) and see the test go red.
A tool.call result has isError and deny; both mean the edit did not happen.
Why: saves a round of rewriting tests that silently passed for the wrong reason.
