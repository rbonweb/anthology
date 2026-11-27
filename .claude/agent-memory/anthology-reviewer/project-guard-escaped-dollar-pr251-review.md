---
name: 'Escaped-dollar plain_text loosening (PR 251) review findings'
description: 'Round 2 head 3f59e941: search-only gate restores every round 1 repro to G0; zsh arithmetic backtick, rg --pre and ugrep --filter gaps are pre-existing'
metadata:
  type: project
  modified: 2026-11-27
---

PR 251 (base ed8eab8b) lets plain_text in crates/anthology-shell/src/collect.rs skip a dollar that a backslash escapes, that a backslash follows, or that ends a double-quoted string.
Round 1 (head 24c5f4d8), Critical, runtime proven in bash 5.2.37 and zsh 5.9: the exemptions reached same-shell nested texts. A for, select or read binder holding a bracket payload with an escaped dollar-paren, then trap, a dot-source of a process substitution, or of /dev/stdin with a here-string, joining a name to the variable inside [[ -eq ]], went from G0 to Allow and ran the npx marker.
Round 2 (head 3f59e941): the exemptions apply only when the first decoded word of every segment is grep, egrep, fgrep, rg, cut, head, tail, sort, uniq or wc. Nested segments (process substitution, subshell, function body) are in lexed.segments, so an eval or dot inside them turns the gate off. Every round 1 repro is G0 again; command grep and =grep are G0; backslash-grep and quoted grep stay Allow and run grep. Odd backslash counts, dollar-quote, ANSI-C quote, a dollar before a closing quote, dollar-equals, dollar-bracket and (e) flag verdicts are the same on base and head.
Pre-existing on base and head, not this PR: zsh runs a backslash-escaped backtick inside an arithmetic subscript, (( a[escaped backtick npx playwright install escaped backtick] )) with a an array or with path, and the guard allows it (G1 bypass, bash 5.2 does not run it). rg --pre sh FILE runs the file, rg --pre=npx runs npx, and ugrep --filter (the snapshot grep) runs its command; all Allow.
Why: the gate is a first-word allowlist, so its safety rests on segment completeness and on the ten programs not running text.
How to apply: probe new search-only carve-outs against arithmetic commands whose first word is a listed name, and against program-running options of the listed programs.
