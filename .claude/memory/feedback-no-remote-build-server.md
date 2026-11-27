---
name: 'no-remote-build-server'
description: 'Never use the owner''s first remote build server; one second host is approved, reached only through ANTHOLOGY_BUILD_HOST and scripts/remote-build.sh'
metadata:
  type: feedback
  modified: 2026-11-27
---

The owner, 2026-11-26: never use the remote build server they first set up for builds or tests (no remote-rust-test.sh, no SSH builds there), even when CI is down.

The owner, 2026-11-27: a second host is approved for builds. Its alias lives only in personal memory and the ANTHOLOGY_BUILD_HOST env var, never in the repo. Use it through scripts/remote-build.sh.

**Why:** the first server was off limits; the second one takes build load off the Mac.

**How to apply:** build locally by default; a cold guard test is about 54 s on the Mac against about 2m41s on the remote host. Use the remote host when the Mac is busy or several agents build at once. Never name either host in the public repo.
