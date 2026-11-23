---
name: speed-over-review-rounds
description: "Owner wants Rust-only work, maximum parallelism and one review per task; no Python work, no repeated review rounds"
metadata:
  node_type: memory
  type: feedback
  originSessionId: 1c9fb279-a45d-4d27-8e78-1ddd82c1e344
  modified: 2026-11-21T04:47:36.342Z
---

On 2026-11-21 the owner objected sharply to the pace of the Rust port, in these words:
- "you are wasting my time" (about the Python fix lanes);
- "too much time in review";
- "do it in an hour".

They also raised `CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS` to 60, and asked that the Karpathy guidelines skill (multica-ai/andrej-karpathy-skills) be used in development and added to the stack.

**Why:** they judge progress by finished features and wall-clock time, and they see Python fixes and multi-round reviews as waste, because the Python never ships.

**How to apply:**
- No Python work. Port fixes straight into Rust.
- Give each task ONE independent review. Re-review only after a Critical, and only in security code.
- Run everything in parallel against interfaces that are already published.
- Batch the merges, with one integration test run.
- Give honest time estimates, but lead with what is done.
- Report status in plain feature terms for a non-technical reader, not lane names.
- 2026-11-21 the owner objected that only ~5 agents were busy. Run a 10-minute recurring check (CronCreate) that spawns agents for ANY work startable now. That includes swaps, plumbing, docs, early reviews and release prep, not only feature lanes. Nested subagents are allowed at any depth. Don't let lanes idle waiting on test queues while other work could start.

See [[rust-port-decision]] and [[anthology-user-workflow]].
