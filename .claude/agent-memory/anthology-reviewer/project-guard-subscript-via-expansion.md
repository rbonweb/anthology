---
name: 'A literal subscript tail can reach a name through an expansion'
description: 'Word-local name-adjacency checks for subscripts miss tails joined at runtime by a positional, a variable, a strip or a default value'
metadata:
  type: project
  modified: 2026-11-22
---

Any guard rule that decides a bracket follows no name, so opens no subscript, from one word alone is unsound. bash 5.2 and zsh 5.9 both run the payload when a literal bracket tail holding a command substitution is stored by set --, for s in, read with a here-string, mapfile or a function argument, and then joined after a name with a positional or variable in unset, $((...)) or ((...)). A one-word form also works: unset with a default value expansion whose value starts with the bracket.
Why: PR 153 (subscript_opens in crates/anthology-shell/src/collect.rs) passed every frozen table and its own tests but turned these from G0 into Allow; base already allowed the read and mapfile forms.
How to apply: for any relaxation of the subscript or substitution scan, probe two-step forms and default or replace expansions on base and head with a scratch test that reads cases from a file, and prove each in both shells with a marker program on PATH. Write such notes without literal substitution syntax: the guard blocks the save command as G0.

Status: covered by tests in crates/anthology-guard/tests/g0_fp.rs (PR 191); the guard answers G0 for every storage and join form listed here.
