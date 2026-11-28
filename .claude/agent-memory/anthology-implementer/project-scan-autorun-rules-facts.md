---
name: 'check-repo autorun rules: wiring and hostile-input traps'
description: 'What scan.rs must call from rules/autorun.rs, and the quadratic and stack traps the rules guard against'
metadata:
  type: project
  modified: 2026-11-28
---

rules/autorun.rs exposes three hooks beyond surfaces(): entry_findings(paths, out) is wired as Hooks.entry_findings in scan.rs (autorun.git-hooks HIGH for every .git/hooks entry, symlinks and unreadable included; surfaces() does not repeat it); raise_gitattributes_filters(file, findings) is NOT wired yet (HIGH when another text file runs git config filter.NAME); npm_scripts feeds the npm run chain. Every setup.py is an Install surface (rule autorun.setup-py, command python setup.py) so a HIGH finding inside it reaches CRITICAL through the chain; a clean setup.py therefore shows MEDIUM. Surface has no why field, so the hidden-task note (reveal never, echo off) lives in the file inventory finding. Hostile-input traps found by tests: recursion on dependsOn chains and nested initializeCommand overflowed the stack (depth caps), per-entry clones or lowercase of a huge INI section header went quadratic (share an Rc, work out a section name once), line continuations joined by format! went quadratic (push_str), per-match line_at over a 2 MB file went quadratic (count lines in one pass). Why: the scan reads untrusted repos, so one 2 MB file must never hang or abort it.
