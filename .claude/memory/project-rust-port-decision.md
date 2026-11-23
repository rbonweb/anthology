---
name: rust-port-decision
description: Owner decided 2026-11-20 to port all first-party Anthology Python to Rust before launch; delivery and fail-mode answers
metadata:
  node_type: memory
  type: project
  originSessionId: 1c9fb279-a45d-4d27-8e78-1ddd82c1e344
  modified: 2026-11-20T17:20:16.280Z
---

On 2026-11-20 the owner decided to port **all** of Anthology's own Python to Rust before the 0.x launch: plugin scripts, tooling scripts, eval harness and tests. Their words: "maintain only one language: Rust … no matter how much time is needed", using multi-agent and nested agents, without compromising quality.

**Why:**
- They want a single language to maintain.
- My pitch named two real gains:
  - closing the "python3 missing → guard off" hole;
  - one shell parser shared with rtk instead of two that can disagree.

**Their answers:**
- **Delivery:** one signed `anthology` binary from this repo's GitHub releases. A tiny sh bootstrap downloads it at first session and checks it against a pinned sha256, like rtk.lock.
- **Fail mode:** fail closed. Until the guard binary is installed, shell, write and MCP tools are blocked with a "run /anthology:setup" message. Read-only tools still work.

**How to apply:**
- The launch is held until the port lands.
- The reviewed Python build and its ~1,500 tests are the behavioural spec, with differential testing between Python and Rust.
- Vendored upstream content (superpowers, knowledge scripts) stays as upstream wrote it.

See [[anthology-status]].
