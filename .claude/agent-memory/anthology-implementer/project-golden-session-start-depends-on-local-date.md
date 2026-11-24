---
name: 'Session-start golden replay pins the day; compare a re-bless per case'
description: 'The day-dependence is fixed (header pins the clock); how to prove a hook.session-start bless changed only the intended lines'
metadata:
  type: project
  modified: 2026-11-24
---

The hook.session-start and memory.sync goldens replay on the day recorded in the golden header (crates/difftest/src/golden.rs pin_day, legacy 2026-11-22), so bless and check give the same result on any day and no TZ trick is needed. An older version of this note said they failed on every other day; that is fixed.
Why it matters: a re-bless rewrites every case that differs, so a reviewer cannot see the intent from the file alone.
How to apply: keep the old gz in /tmp, bless with scripts/dev.sh golden bless --surface hook.session-start, then gunzip both and compare by case id (rows are id, case, expect, marks; large cases have case null). For a session-start text change, edit the old expect.stdout the way the code now does (for example drop one systemMessage line) and require it to equal the new one for every changed id. Count the changed ids and name the cause in the PR.
