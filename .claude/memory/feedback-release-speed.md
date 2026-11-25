---
name: release-speed
description: Plan the whole release chain at batch start and overlap its steps; a ~40 min Rust CI job times 4 serial PRs made a release take hours
metadata:
  node_type: memory
  type: feedback
  originSessionId: 1c9fb279-a45d-4d27-8e78-1ddd82c1e344
  modified: 2026-11-24T10:35:42.208Z
---

The owner said the work was "too slow" after a release took about 4 hours. Most of that time was four PRs in a row (next batch, binary version prep, lock pin, main sync), each waiting about 40 minutes on the Rust test job.

**Why:** the lead found out late that the batch changed Rust code, so it needed a new binary release, and prepared that only after the batch merged. Version-only and lock-only PRs also ran the full Rust suite.

**How to apply:**
- At the start of a batch, list the release chain it needs. Rust changes mean a binary prep, a tag, a lock PR and the plugin releases.
- Open the binary version prep PR while the batch PRs are still in CI. Merge it right after them.
- Sync main once, at the very end.
- Keep CI fast (see the `ci: faster rust tests` work) so metadata-only PRs don't run the full suite.
- When estimating, count serial CI rounds times the measured job time, and say the number.
