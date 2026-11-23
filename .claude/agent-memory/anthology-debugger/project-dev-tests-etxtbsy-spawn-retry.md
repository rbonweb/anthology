---
name: 'Freshly written stub scripts hit ETXTBSY in threaded tests'
description: 'Why a just-written executable stub fails to spawn on Linux CI, and why rename does not help'
metadata:
  type: project
  modified: 2026-11-23
---

In crates/anthology-dev tests, write_exec (tests/smoke.rs) writes a stub and run_process (src/smoke/runner.rs) executes it at once. On Linux another test thread forking while the write fd is open leaves the fork child holding that fd until its exec, so execve fails with ETXTBSY. run_process turned the failed spawn into Proc(-1, "", "could not run X: OSError"), so a test saw empty output. Renaming the file does not help (same inode, still open for writing). The fix is spawn_retrying in runner.rs: retry ETXTBSY 20 times, 25 ms apart. It reproduced about 1 in 150 runs of the smoke test binary in the rust container, which is why CI failed only now and then. When a stub test shows empty output, read Proc.err and Proc.code first. The version-bump job only checks plugins, so crates/anthology-dev changes need no bump.
