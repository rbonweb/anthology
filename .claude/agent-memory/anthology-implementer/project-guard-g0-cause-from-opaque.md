---
name: 'G0 cause hints come from Opaque marks, and many G0s carry none'
description: 'check_explained names a cause only when an entry holds an Opaque mark; piped or heredoc shells and many G0s have none'
metadata:
  type: project
  modified: 2026-11-24
---

check_explained (crates/anthology-guard/src/check.rs) returns the first Opaque mark found on the entries, in the order DynamicCommandWord, Eval or ShellScriptFromExpansion, Source, StdinShellUnreadable, only for a TooComplex verdict whose analysis is incomplete. Many real G0s have no mark: curl x | sh, a heredoc shell with a substitution, and a substitution as the program word leave the entry opaque list empty, so they get no cause hint. Source is also set when a shell is given a script file (sh p.sh), so its hint says sourced file or script. Source and StdinShellUnreadable are often set on commands the guard allows, so beside an unrelated G0 reason they give a false (text only) hint. Why: the marks are anthology-shell facts, not the reason incomplete was set; naming the true reason needs an anthology-shell change, which must keep ATL analysis byte-identical. How to apply: probe a command with anthology_shell::analyze in a scratch test under crates/anthology-guard/tests before writing a hint test; pin expected verdicts as literals in verdicts_are_unchanged, not computed from the code under test; guard.hook golden bless rewrites only G0 stderr rows, and bless also adds a today key to the golden header (same day as the legacy default, harmless).
