---
name: 'Guard G0 on a bracket with backticks comes from subscript_runs, not the lexer'
description: 'Where a G0 for brackets plus backticks is decided and what must stay conservative when loosening it'
metadata:
  type: project
  modified: 2026-11-22
---

A G0 on a word that holds an opening square bracket and two backticks is decided by subscript_runs in crates/anthology-shell/src/collect.rs (through fail_closed), not by the lexer. The lexer already reads an escaped backtick in double quotes as a literal one. Why it matters: bash evaluates a subscript after quote removal, so unset with a name, a bracket and an escaped-backtick command still runs that command; making the escaped backtick not count would be a bypass. Also the lexer takes dollar-paren and backtick substitutions out of a word, so a name before the bracket can vanish; the segment flag subst or arith in add_one is the only trace of it. How to apply: loosen only by what precedes the bracket (a subscript follows a name), keep every bracket conservative in a segment with an array literal or a substitution, and check any change against the pinned tests in crates/anthology-guard/tests/g0_fp.rs. A variable program word already passes the guard unchecked, so the bracket check cannot be the only defense there.
