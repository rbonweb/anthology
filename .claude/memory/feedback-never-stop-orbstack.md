---
name: never-stop-orbstack
description: "Never quit OrbStack/Docker or remove containers, images or volumes that aren't Anthology's: other projects use it"
metadata:
  node_type: memory
  type: feedback
  originSessionId: 1c9fb279-a45d-4d27-8e78-1ddd82c1e344
  modified: 2026-11-21T13:02:49.287Z
---

Never quit, stop or restart OrbStack/Docker. Never touch containers, images, volumes or the build cache that belong to other projects. The owner's other projects (sinrix, among others) run their tests in it.

**Why:** on 2026-11-21 I quit OrbStack to free memory. That stopped the other project's running test containers, and the owner had to tell me not to.
**How to apply:** clean up only items that are clearly Anthology's (named `anthology-*`, or images I built myself). Leave everything else, including the shared build cache. Even if the owner agreed to quit or stop it earlier, check first whether another project is using Docker. Related: [[anthology-machine-facts]].
