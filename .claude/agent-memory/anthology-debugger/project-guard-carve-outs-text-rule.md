---
name: 'Guard carve-outs must not rely on setter lists'
description: 'Why the prefix carve-out checks the whole command text and refuses any other dollar, and why PWD is excluded'
metadata:
  type: project
  modified: 2026-11-22
---

The directory-prefix carve-out (crates/anthology-shell/src/collect.rs changes_prefixes) is off when a read-through name appears anywhere outside the forms $NAME/, ${NAME}/, $NAME"/ or ${NAME}"/ in the command text or any lexed word, when any other $ appears, or when the command sources a file or runs eval. PWD is never read through.
Why: the first version listed setters and the PR 159 re-review found zsh ${HOME::=}, print -v HOME and bash printf -vHOME; a name built from pieces (x=HO; x+=ME; printf -v "$x") needs a $ to reach a sink, so refusing every other $ closes that class. Claude Code keeps the cwd between calls, so a directory named with $(...) makes PWD unsafe in a script.
How to apply: when widening a carve-out, prefer a whole-text rule that fails closed over listing the commands that change a value; re-probe the reviewer note inputs in bash and zsh first.
