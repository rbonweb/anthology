---
name: 'Git spawn timeout (PR 196) review findings'
description: 'Head 1c58af57: root cause holds; the mpsc handoff can drop a late Child unreaped; the stress test does not prove SPAWN'
metadata:
  type: project
  modified: 2026-11-23
---

Head 1c58af57, crates/anthology-core/src/git.rs spawn(). Root cause matches std 1.100 (see reference-std-spawn-macos-fork-path).
Finding: spawn() hands the Child over an unbounded mpsc::channel. If the helper sends after recv_timeout returned Timeout but before rx is dropped, send succeeds, the queued Child is dropped with rx, and it is never killed or reaped. Proven in Docker by pausing 200 ms after the timeout: a Z (git) child of the test process stayed. mpsc::sync_channel(0) closed it (no zombie, git unit tests pass).
Also: a helper stuck in spawn holds SPAWN, so every later git call in that process waits out its full timeout until the leaker exits. Queued helpers still spawn and then kill a git after their caller gave up. concurrent_hung_gits_each_time_out_on_schedule passes with or without SPAWN once the caller is deadline-bound, so only a_stuck_spawn_is_abandoned_at_the_timeout pins the lock (a mutant that drops the lock fails it).
Why: re-reviews of this code should confirm the handoff and not re-derive the std facts.
How to apply: on a re-review check the channel kind in spawn() and rerun the pause probe.
