---
name: 'Guard carve-outs are allowlists, not setter lists'
description: 'Why the three PR 159 carve-outs became character and program allowlists, and what they over-block'
metadata:
  type: project
  modified: 2026-11-22
---

The directory-prefix carve-out (crates/anthology-shell/src/collect.rs prefixes_allowed) reads "$NAME/..." or "$NAME"/... through only when every $ in the command text is one of those forms and every other character is in [A-Za-z0-9_./:=,+@%-], a blank, a quote or ;&|<> (no <<), the name appears nowhere else, and nothing sources or evals. Read loops (evaluates_values) are back to the strict rule: any arithmetic, let, [[ -eq ]], or subscript naming a variable in a command that reads input is G0. Function positional calls (positional_read, body_keeps_positionals, literal_call_word) need charset call words, no number words in the body and none of read, print, printf, typeset, declare, local, export, set, shift, zparseopts, getopts, eval, source, ., builtin, command, alias.
Why: rounds 1 and 2 of the PR 159 review each found names the denylists missed (brace-built HO{M,}E, zsh typeset -m patterns, ${n:=line}, for n in line, zsh 1+=x, extendedglob # and ^). The coordinator ruled allowlists on 2026-11-22. Accepted over-blocks: counted read loops, retry wrappers with for i in 1 2 3, ${HOME}/ and $HOME inside sh -c scripts.
How to apply: widen a carve-out only by adding to an allowlist; never add a setter or pattern-character list. The benign fixture crates/anthology-shell/tests/fixtures/sh_fw_verify_benign.txt and goldens hold some over-blocked forms.
