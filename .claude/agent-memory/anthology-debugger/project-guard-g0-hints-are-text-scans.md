---
name: 'Guard G0 hints: two text scans plus cause marks from check_explained'
description: 'Where a G0 message hint is added: hook.rs hints_for, from text scans of the command and from the Opaque mark check_explained returns'
metadata:
  type: project
  modified: 2026-11-24
---

A hint on the G0 line is built in crates/anthology-guard/src/hook.rs hints_for, only when the verdict is TooComplex, and never changes a verdict. It has two sources. Text scans of the command: a double-quoted script with a dollar (single quotes advised) and a lone backtick in double quotes. Cause marks: check_explained in check.rs returns the first anthology-shell Opaque mark on the entries (DynamicCommandWord, then Eval or ShellScriptFromExpansion, then Source, then StdinShellUnreadable), which maps to a program, quoted-script, sourced-file-or-script or heredoc-shell hint. Each hint appears once. Why: check() alone returns only a Verdict; the mark is the only reason available without changing anthology-shell or Analysis, so a G0 whose entries carry no mark (curl x | sh, a substitution as the program) gets no cause hint. Which lone-backtick commands are G0 depends on the rest of the command: grep with a backslash-pipe pattern or a pipeline after it is G0, while echo or rg with a bare lone backtick at the end is allowed. How to apply: a new hint needs a case in hints_for and a test in crates/anthology-guard/tests/g0_message.rs; a guard.hook golden row (gen:N) may change its message, so run difftest golden check on guard.hook and bless only the rows that changed, then confirm the rows differ only in G0 stderr.
