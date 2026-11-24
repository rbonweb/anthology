---
name: 'G0 cause hints: halt_cause first-class, then Opaque marks'
description: 'cause_of prefers Analysis.halt_cause (three anthology-shell halt sites), then an entry Opaque mark'
metadata:
  type: project
  modified: 2026-11-24
---

Analysis.halt_cause (crates/anthology-shell/src/analyze.rs, set by Ctx::name_cause in collect.rs) names stops that leave no entry mark: a shell fed by another command (curl x | sh), a shell fed only a substitution, and a substitution as the program word (resolve_holders). cause_of in crates/anthology-guard/src/check.rs checks halt_cause FIRST and reads the entry marks only when it is None, so halt_cause takes precedence: some older hints were replaced by more precise ones (a sourced-file case now gets the stdin-shell hint, several quoted-script cases now get the program hint; 7 of the 33 re-blessed guard.hook rows are replacements, not additions). Still unnamed: work-budget stops, subscripts, functions fed a pipe. Why: the field is additive (atl and the shell goldens ignore it); the earlier debugger note saying not to add an Analysis field is outdated, since atl only builds Analysis in one test with ..Default. How to apply: probe a command with anthology_shell::analyze before writing a hint test; pin verdicts as literals; guard.hook bless rewrites only G0 stderr rows (33 in PR 224); mutate name_cause, the halt_cause precedence in cause_of and the embedded limit, panic and watchdog lines to prove the tests.
