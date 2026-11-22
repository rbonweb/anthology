---
name: 'claude plugin test kit traps for the anthology mods'
description: 'How the test kit answers $ calls and which engine-set verdict fields a test cannot raise; why mutants survived at first'
metadata:
  type: project
  modified: 2026-11-22
---

In plugins/anthology/tests/mods, the test hooks sit beneath the plugin and stand for Claude Code.
- A hook on a $ op (process.run, session.cwd, session.root, settings.read) answers { value: X }, not a bare X.
- A $ call that no test hook answers throws beneath. A mod that catches failures then quietly keeps the verdict, so a guard test passes for the wrong reason. That is how the marker mutants first survived. Give every test the full engine() helper (env, session, settings, process.run) and mutate each guard to prove it.
- The kit drops a test-written plugin or ceiling on a tool.check verdict, because both are engine-set and pinned. To raise plugin, load an inline append-tier plugin whose tool.check asks. A ceiling cannot be raised from a query at all.
- hook, rule and setting do pass through as the test wrote them.
Why: these cost two mutation rounds on P5 (permissions.ts).
