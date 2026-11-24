---
name: 'G0 halt_cause hints (PR 224) review findings'
description: 'Head 9981fa99: halt_cause is message-only, set just before an existing halt; golden re-bless changed only G0 stderr'
metadata:
  type: project
  modified: 2026-11-24
---

PR 224 added Analysis.halt_cause in crates/anthology-shell (set by Ctx::name_cause right before halt at the curl x | sh site, the sh <<< "$(f)" site, and in add_one when resolve_holders reports a substitution program). Ctx never clears halted, so the cause cannot outlive a real stop. check.rs cause_of takes halt_cause before entry marks, so some old hints changed (Source to stdin-shell, quoted-script to program); the implementer note claimed the reverse order. Hints are constants, no command text is echoed. Embedded limit, panic and watchdog lines use the flag from host::embedded_from_env.
Why: a later change to cause_of or name_cause should keep the verdict independent of the cause; per-case golden diff (5110 rows, 33 changed, exit equal everywhere) was the verdict proof.
How to apply: when hints change again, decode the guard.hook golden per case and confirm only expect.stderr differs.
