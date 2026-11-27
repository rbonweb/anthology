---
name: 'Escaped-dollar plain_text loosening (PR 251) review findings'
description: 'Head 24c5f4d8: every exempted dollar form reaches a same-shell nested text (trap, dot-source of a procsub or here-string) and runs a hidden subscript'
metadata:
  type: project
  modified: 2026-11-27
---

PR 251 (base ed8eab8b, head 24c5f4d8) lets plain_text in crates/anthology-shell/src/collect.rs skip a dollar that a backslash escapes, that a backslash follows, or that ends a double-quoted string.
Critical, runtime proven in bash 5.2.37 and zsh 5.9 (debian trixie-slim): those three forms are how a writer hands a live dollar to a nested text. The outer text then counts as plain, so a for-loop variable holding a subscript payload with no name before the bracket is not flagged, while the nested text joins the name. Shapes that were base G0, head Allow and ran a G1 npx marker: typeset -a a, then a for loop over a double-quoted bracket word holding an escaped dollar-paren payload, then trap with a double-quoted [[ a, escaped dollar, X -eq 0 ]] at EXIT; the same with a dot-source of a process substitution echo, or of /dev/stdin with a here-string; the same with the dollar-ends-string and dollar-backslash forms; a single-quoted backtick payload too. A grep before it does not help.
Held: set -- positional payloads stay G0 on head (positional tracking), eval, sh and flock -c stay G0; read and printf -v binders stayed G0 or G1.
Why: trap and the dot builtin are not in TEXT_CONSUMERS, and plain is computed once per text while nested texts get the decoded word, where the escape is gone.
How to apply: in later rounds probe every binder (for, select, getopts, read) with trap and dot-source sinks; zsh needs the name to be an array for [[ -eq ]] subscript evaluation to run a command; bash does not.
