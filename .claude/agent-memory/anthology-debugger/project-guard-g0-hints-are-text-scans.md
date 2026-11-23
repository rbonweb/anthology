---
name: 'Guard G0 hints are text scans in hook.rs'
description: 'Where a G0 message hint is added, why it is a scan of the command text, and which backtick shapes are G0'
metadata:
  type: project
  modified: 2026-11-22
---

A hint on the G0 line (single quotes for a double-quoted script with a dollar, or for a lone backtick in double quotes) is added in crates/anthology-guard/src/hook.rs decide_with, from a scan of the command text, only when the verdict is TooComplex. It never changes a verdict. Why: check() returns only a Verdict, so the reason is not available there; keeping the hint in the hook leaves anthology-shell and Analysis untouched. Which lone-backtick commands are G0 depends on the rest of the command: grep with a backslash-pipe pattern or a pipeline after it is G0, while echo or rg with a bare lone backtick at the end is allowed (the lexer reads an unterminated substitution as nothing runnable). How to apply: a new hint needs a block in decide_with and a test in crates/anthology-guard/tests/param_expansion.rs; a fuzz-generated guard.hook golden case (gen:N) may change its message, so run difftest golden check on guard.hook and bless only the case that changed.
