---
name: 'Host path variables (PR 217, T4) review findings'
description: 'Round 1 (1a9f8568) zsh typeset -m quote-split bypass and tied arrays; closed at 8c40f77e by a no-angle-bracket word rule'
metadata:
  type: project
  modified: 2026-11-24
---

Round 1 (head 1a9f8568): a rule that looked only at the raw text for a less-than sign before a digit or dash missed zsh typeset -m with the pattern split by quotes (TOOL, a quoted less-than, a bare dash, a quoted greater-than, then an assignment), which set TOOL1 to npx; zsh tied lowercase arrays (manpath and kin) set the uppercase name unspelled. Round 2 (head 8c40f77e) closed both: any analysis word, quotes removed, holding an angle bracket disables the read-through, -m is refused on the typeset family, and the tied names are denied. Probed builtin, noglob, command, the dash precommand, emulate -c, private and trap forms: all G0; redirections and pipes still Allow. Arithmetic on a += built value can still set a listed name to a number; ADR-038 accepts that as no worse than typing a numeric program.
Why: text checks on the raw command miss what a builtin sees after quote removal; the word-level check is the one that holds.
How to apply: for any later relaxation of carve-out d, probe quote-split patterns and tied arrays first, with a scratch test that implements PathVars directly and a debian zsh container with a marker npx.
