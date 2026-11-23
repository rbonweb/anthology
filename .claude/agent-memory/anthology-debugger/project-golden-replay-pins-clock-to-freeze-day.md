---
name: 'Golden replay runs on the freeze day (clock pin)'
description: 'Why committed goldens failed on any day but their freeze day, and how replay now pins the clock'
metadata:
  type: project
  modified: 2026-11-23
---

The goldens hold Python answers recorded on the freeze day (2026-11-22 for all committed ones), and setup (approved), hook.session-start (migrated-DATE) and memory.sync (modified, today normalisation) write or compare the real date. So the_committed_goldens_replay_clean passed only on the freeze day and failed on every later one (CI run on 2026-11-23: 326 unexplained: setup 284, session-start 24, memory.sync 18). Fix: golden header field today (written by freeze), and golden check/bless pin anthology_core::clock::today_local to it through the test-clock feature (pin_today); old headers fall back to LEGACY_DAY in crates/difftest/src/golden.rs. A re-freeze on a new day records that day. Prove a day-dependence with faketime in the rust container (apt faketime): faketime "2026-11-23 12:00:00" /target/debug/difftest golden check --surface setup. Do not use faketime on rtk_install.reconcile: it compares real file mtimes with the faked clock and shows false diffs. Why: the first red CI looked like a repo-file leak (a settings change landed the same day) but the replay is hermetic (tmp roots, fake git); the date was the cause.
