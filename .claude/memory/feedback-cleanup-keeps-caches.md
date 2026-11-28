---
name: 'Cleanup keeps images and shared caches'
description: 'Remove per-run containers, folders and volumes; keep pulled images, toolchains and dependency/build caches; prune caches only under disk pressure'
metadata:
  type: feedback
  modified: 2026-11-28
---

Cleanup keeps what later runs reuse:
- **Kept:** pulled base images, toolchains (rustup), dependency caches (cargo registry, sccache, npm/pnpm/pip stores), build-cache stores, and an agent's per-worktree target volume while that worktree exists.
- **Removed after use:** per-run containers, per-run folders and work volumes, and images built only for one worktree.
- **Disk safety:** shared caches are pruned only under pressure (disk over 85% or a cache over its size cap), least-recently-used first, ours only.

Why: on 2026-11-28 the owner found that the "clean up your own mess" rule made every run download images and rebuild from nothing ("every images needs to be downloaded again and again... some sort of stuff which needs to be persistent"). Sinrix saw the same failure when a per-job buildx builder threw away the cache, and builds took 18-47 minutes.

How to apply: every cleanup rule names what it keeps, and gets a performance check (cache hits, build time before and after). See [[feedback-builds-on-zorin]], [[feedback-lead-process-lessons]].
