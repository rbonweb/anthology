---
name: 'no-remote-build-server'
description: 'Never use the owner''s remote build server for builds or tests; only local Docker and CI. A permanent rule, not a load throttle'
metadata:
  type: feedback
  modified: 2026-11-26
---

On 2026-11-26 the owner said not to use the remote build server for builds and similar work.

**Why:** it is the owner's machine and they don't want Anthology work running on it. This replaces the older idea that "don't use the build server" was a temporary throttle to re-check after 5-10 minutes; for the build server it is permanent.

**How to apply:** no remote-rust-test.sh, no SSH builds or tests, even when CI is down. Use local Docker containers and CI only. Put this in every agent brief. Related: [[owner-build-rules-2026-11-21]].
