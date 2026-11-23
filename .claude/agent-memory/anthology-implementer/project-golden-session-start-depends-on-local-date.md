---
name: 'Session-start and memory.sync goldens depend on the local date'
description: 'The migrated-<date> file name makes the goldens fail on any other day; bless with a TZ that keeps the golden date'
metadata:
  type: project
  modified: 2026-11-23
---

The hook.session-start and memory.sync goldens contain side-by-side file names such as project-x.migrated-2026-11-22.md. The date comes from the local clock (anthology-core clock::today_local, localtime_r), so cargo test -p difftest --test golden and golden check fail on every other day: about 24 hook.session-start and 18 memory.sync cases, all with only the date digits different.
Why: build went red the day after the last bless and it looked like a leaked repo file; a merge also re-recorded 23 date-only cases that hid the real changes.
How to apply: to bless or check on a later day, run the command inside the container with a POSIX TZ that gives the golden date, for example scripts/rust-test.sh run sh -c 'TZ=HST10 cargo run --locked --quiet -p anthology-dev -- golden bless --surface hook.session-start' (HST10 is UTC-10, so it is still the 22nd until 10:00 UTC on the 23rd; scripts/dev.sh cannot pass TZ). Then gunzip the old and new golden and compare per case id: only the cases you expect should differ.
