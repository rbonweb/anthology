---
name: 'remote-build.sh: how it cleans up and where it is pinned'
description: 'scripts/remote-build.sh sweeps containers by target volume, not name; fake-ssh repo test; timings; CI shellcheck list not updated'
metadata:
  type: project
  modified: 2026-11-27
---

scripts/remote-build.sh runs rust-test.sh on a Linux host named by ANTHOLOGY_BUILD_HOST (env only, never in a file). rust-test.sh runs docker run --rm with no --name, so the only handle on its containers is the target volume: the script removes every container where docker ps -aq --filter volume=anthology-rust-target-<cksum of the remote folder path> lists it. Why this matters: a dropped ssh link without a tty does not stop the remote process, so the remote script reads its stdin in the background and stops the run when stdin ends; a fifo held open by a local sleep keeps that stdin alive. The volume name must stay equal to what rust-test.sh computes (cksum of its root path); if rust-test.sh changes how it names the volume or names its containers, change the sweep in remote-build.sh too.
Pinned by crates/repo-tests/tests/remote_build.rs (fake ssh and docker on PATH, no host needed). The ssh-drop test closes stdin two seconds in: closing it at once races the fake container start and flakes. The rsync test runs only with GNU rsync 3 (macOS ships openrsync, the CI image has none); ANTHOLOGY_REMOTE_SYNC=tar forces the tar path.
Measured once on the approved host (i7-8700, 12 threads, 32 GB) against the owner Mac in Docker: cold target test -p anthology-guard took 2m41s remote and 54s local; warm 1m50s remote and 44s local, so the remote host was slower for that crate. The first run on an empty host took 6m22s for test -p anthology-pycompat (image pull, apt, toolchain).
Open: ci/jobs/shellcheck.sh bash_scripts does not list scripts/remote-build.sh yet (CI files were out of scope). A repo note, no-remote-build-server, says the owner banned the build server on 2026-11-26; the brief for this task said one host was approved; ask the lead which holds.
