---
name: 'Guard probe and runtime proof must use the same payload shape'
description: 'G1 payloads hold spaces; an unquoted subscript splits on them, so a guard Allow can be harmless; prove with a marker that has the same spaces'
metadata:
  type: feedback
  modified: 2026-11-22
---

When a guard probe uses npx playwright install inside an unquoted or re-lexed subscript, the shell splits the word at the spaces, the subscript is invalid and nothing runs. In the PR 153 re-review, ten wrapper probes (trap, watch, flock -c, script -c, su -c, git alias, dot-source of a process substitution, ssh, awk and perl system) went from G0 to Allow, but all were harmless for that reason; with the spaces kept inside double quotes in the subscript, every one stayed G0.
Why: a first pass proved the runtime with a one-word marker while the guard saw a three-word payload, which nearly produced a false Critical.
How to apply: give the runtime marker the same word structure as the guard payload (for example a marker with an argument, in the same quotes) before calling a verdict change a bypass.
