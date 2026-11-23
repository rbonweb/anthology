---
name: anthology-release-approvals
description: "Owner's 2026-11-21 standing yes for the anthology-bin--v0.1.0 tag and the eval baseline spend"
metadata:
  node_type: memory
  type: project
  originSessionId: 1c9fb279-a45d-4d27-8e78-1ddd82c1e344
  modified: 2026-11-22T12:11:31.017Z
---

On 2026-11-21 the owner pre-approved two release steps:
- Publish the public tag `anthology-bin--v0.1.0` (CO1) once every in-flight fix has merged into `rust`, with no further check-in.
- Run the eval baseline (API spend) at the release step, with no further check-in.

On 2026-11-22 the owner also approved `anthology-bin--v0.1.1`. It ships the guard fix for the `$((`/`$(((` holder bypass (#133), published after a clean 10k comparison run.

Later on 2026-11-22 the permission system blocked a subagent's tag push. The owner then said "proceed with yourself for everything". That means the main session does the remaining release steps itself (tags, pushes, merges through CO2/CO3 and Task 17) instead of handing pushes to the owner. If a subagent is denied a push or merge, the main session does it only when that step falls under this approval, and says so.

The first eval run (2026-11-22) produced nothing, because the runner lacked socat and bubblewrap. The owner approved ONE more eval run after the sandbox fix (#148), with the result committed as `evals/baseline.json`.

Later on 2026-11-22 that run was found to have gone out on old main (it failed 7 smokes, no baseline). The owner approved one more run on current main (e0e0f4d7), dispatched by an agent. That is the last approved eval run; any further run needs a new yes.

**Why:** the owner wanted to avoid waiting on them and accepted the cost.
**How to apply:** don't ask again for these two. This covers only v0.1.0, v0.1.1 and one baseline run, not later tags or extra eval runs. See [[anthology-status]] and [[rust-port-decision]].
