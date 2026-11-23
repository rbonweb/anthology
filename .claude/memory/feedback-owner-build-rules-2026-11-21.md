---
name: owner-build-rules-2026-11-21
description: "Owner corrections: load is temporary (re-check in 5–10 min), clean up your own Docker volumes, run only targeted tests during development"
metadata:
  node_type: memory
  type: feedback
  originSessionId: 1c9fb279-a45d-4d27-8e78-1ddd82c1e344
  modified: 2026-11-21T13:53:20.454Z
---

Three owner corrections from 2026-11-21, now also in ~/.claude/CLAUDE.md and the large-builds skill:

1. **Load is temporary.** A throttle put in place because a machine or server was busy (for example CARGO_BUILD_JOBS=3, or "don't use the build server") must be re-checked after 5–10 minutes and lifted once the load is gone.
   **Why:** a 5-minute spike that becomes a standing rule costs hours of build time.
2. **Docker cleanup.** Remove your own containers, the images you built and their volumes once you're finished with them.
   **Why:** about 470 GB of anthology-rust-target volumes piled up in one day.
3. **Targeted tests only during development.** Run only the tests for the changed part. The full suite runs once, at the final go, which is usually CI.
   **Why:** repeated full runs, including remote ones, wasted time and blocked shared capacity.

**How to apply:** put all three in every agent dispatch. Related: [[never-stop-orbstack]], [[speed-over-review-rounds]], [[build-server]].
