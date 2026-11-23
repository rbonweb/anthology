---
name: 'PR 159 carve-out gaps (round 2 head 583c129b)'
description: 'Carve-out bypasses still open on PR 159 round 2, each proven with a marker npx in bash 5.2 or zsh 5.9'
metadata:
  type: project
  modified: 2026-11-22
---

Status: closed in anthology-bin 0.1.4 (see crates/anthology-guard/tests/param_expansion.rs and g0_fp.rs).

All round 1 carve-out bypasses are G0 at 583c129b. Still allowed and proven to run:
(a) HOME named through brace expansion or a zsh pattern, so the text never spells it and holds no other dollar: IFS= read -r HO{M,}E < file, export HO{M,}E=value, printf -v HO{ME,%s} (format in single quotes), zsh print -v HO{ME,}, zsh typeset -m with a quoted HO?E pattern. Each then pastes the value into sh -c with a double-quoted HOME slash.
(b) A read value reached through a plain name set without an assignment word: a default-value expansion n:=line, or for n in line; then n evaluated in arithmetic (bash).
(c) zsh positional setters outside sets_positionals: 1+=px, print -v 1, read -r 1; zsh extendedglob patterns np#x and np^y in a direct call word.
Why: found in the PR 159 round 2 re-review on 2026-11-22; text-match rules miss names that brace expansion or a pattern builds.
How to apply: for a carve-out that matches names in the text, probe brace-built and pattern-matched names first; for literal-word checks, include zsh extendedglob characters.
