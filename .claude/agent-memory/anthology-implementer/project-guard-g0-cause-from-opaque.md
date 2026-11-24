---
name: 'G0 cause hints: halt_cause first-class, then Opaque marks'
description: 'check_explained takes an entry Opaque mark, else Analysis.halt_cause set at three anthology-shell halt sites'
metadata:
  type: project
  modified: 2026-11-24
---

Analysis.halt_cause (crates/anthology-shell/src/analyze.rs, set by Ctx::name_cause in collect.rs) names stops that leave no entry mark: a shell fed by another command (curl x | sh), a shell fed only a substitution, and a substitution as the program word (resolve_holders). check_explained in crates/anthology-guard/src/check.rs tries the entry marks first, then halt_cause, so the older hints keep their order. Still unnamed: work-budget stops, subscripts, functions fed a pipe. Why: the field is additive (atl and the shell goldens ignore it); the earlier debugger note saying not to add an Analysis field is outdated, since atl only builds Analysis in one test with ..Default. How to apply: probe a command with anthology_shell::analyze before writing a hint test; pin verdicts as literals; guard.hook bless rewrites only G0 stderr rows (33 in PR 224); mutate name_cause, the halt_cause fallback and the embedded limit, panic and watchdog lines to prove the tests.
