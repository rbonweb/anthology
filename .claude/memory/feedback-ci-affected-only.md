---
name: 'ci-affected-only'
description: 'PR CI must run only the tests the change affects, and each round should take minutes; the full suite runs on build, main and release'
metadata:
  type: feedback
  modified: 2026-11-27
---

The owner, 2026-11-27: a PR round of about 17-40 min is too slow. Split the work and run only the tests a change needs.

**Why:** every fix round, review round and conflict re-run pays the full CI wait; today's batch had many rounds.

**How to apply:**
- PRs run Rust tests only for changed crates and the crates that depend on them; golden replays run only for changed surfaces; docs and plugin changes run no Rust tests.
- macOS runs on PRs only for platform-sensitive paths.
- The full suite runs on pushes to build and main, and at release.
- When estimating, use the measured per-round time, not a guess. Related: [[release-speed]].
