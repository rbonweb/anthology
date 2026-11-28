---
name: 'remote-build.sh runs safely in parallel: locks, per-run folders, slots'
description: 'How scripts/remote-build.sh isolates concurrent runs on one host, and the traps found while testing it'
metadata:
  type: project
  modified: 2026-11-28
---

scripts/remote-build.sh gives each run its own folder <checkout-hash>-<run> (kept runs use <checkout-hash>), a flock on <folder>.lock held by a host-side holder session, a shared flock on .runs.lock, slot locks under .slots, and an install lock .toolchain-<digest>.lock. The kernel drops a flock when its holder dies, so a lock that can be taken marks a dead run; no PID or clock is used. Why: the old design shared one folder and one target volume per checkout, so a run finishing or a second run sweeping deleted the live one (exit 137 from docker rm -f, rsync exit 23), and --clean-ours rm -rf-ed the whole root. Traps: (1) the fake ssh in crates/repo-tests/tests/remote_build.rs applies its stdin-drop only to commands that contain the text rust-test.sh, so keep that text out of remote_head and hold_body; (2) a flock conversion shared to exclusive with -n loses the old lock on failure, so toolchain waiters use a 5 s timeout loop, not a blocking exclusive wait that would queue behind a builder; (3) rm of .runs.lock in --clean-all-ours races a starting run, so that file is never deleted; (4) another agent on the same host may still run the old script, whose --clean-ours wipes the root, including markers: the new script reinstalls the toolchain then. Real proof: two test -p anthology-core runs at once on the build host pass, and --clean-ours says it left both alone.
