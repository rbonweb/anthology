---
name: 'Proving guard bypasses in bash and zsh'
description: 'A container recipe to show whether a command runs a marker program in bash 5.2 and zsh 5.9'
metadata:
  type: reference
  modified: 2026-11-22
---

debian:bookworm-slim ships bash 5.2.15, and `apt-get install zsh` there gives zsh 5.9, the two shells the guard reviews name. Put a marker script named npx in /tmp/bin that appends to /tmp/marker, then run each payload line with `PATH=/tmp/bin:$PATH timeout 5 bash -c "$line"` and the same with zsh, and check the marker. Keep a matching file in the work dir for glob payloads. A one-off cargo test that prints `check()` for the same lines shows the guard verdicts side by side.
**Why:** guard security fixes must be proven against the real shells; many reported shapes only run in one shell (zsh glob qualifiers, `${(e)x}`, `functions[u]=`) or need a setup step (`unset` on an array element runs nothing unless the array exists).
**How to apply:** use it before writing a bypass test, and add both the bypass and an everyday look-alike to the must-allow list.
