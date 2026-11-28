---
name: 'Session-start golden replay pins the day; compare a re-bless per case'
description: 'The day-dependence is fixed (header pins the clock); how to prove a hook.session-start bless changed only the intended lines'
metadata:
  type: project
  modified: 2026-11-28
---

The hook.session-start and memory.sync goldens replay on the day recorded in the golden header (crates/difftest/src/golden.rs pin_day, legacy 2026-11-22), so bless and check give the same result on any day and no TZ trick is needed. An older version of this note said they failed on every other day; that is fixed.
Why it matters: a re-bless rewrites every case that differs, so a reviewer cannot see the intent from the file alone.
How to apply: keep the old gz (git show HEAD:<path>), bless with scripts/dev.sh golden bless --surface hook.session-start (runs fast locally), then load both gz with python3 -I and compare by case id (rows are id, case, expect{exit,stdout,...}, marks). stdout is the hook JSON line, a newline, then the state file text (<none> when absent). Compare structurally: parse the first line, drop the one new systemMessage line, parse the state tail as JSON and drop the new key. For a session-start text change, require equality for every changed id, then count them and name the cause in the PR.
Session-start step that writes a new state key (the check-repo hint, key check_repo_hinted): changes cases in three ways, all expected: the hint line in systemMessage plus the key; the key only when the 2000-character message cap cuts the line off; and a new state file holding only that key where there was none. The report from difftest golden check --report only holds truncated outputs and the first differing byte, so it cannot prove this alone.
