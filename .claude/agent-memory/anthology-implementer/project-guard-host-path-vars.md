---
name: 'Guard host path variables (carve-out d): layout, review lesson, mutation map'
description: 'Where the ANTHOLOGY_GUARD_PATH_VARS read-through lives, why name checks must read quote-removed words, and which mutants survive by design'
metadata:
  type: project
  modified: 2026-11-24
---

Carve-out (d), ADR-038: the guard reads a whole double-quoted "$NAME" word as the file the variable names, only for names listed in ANTHOLOGY_GUARD_PATH_VARS (opt-in, because the shell's value can differ from the hook's).
Where: crates/anthology-guard/src/path_vars.rs (HostPathVars::new does the name, value, file, temp and cwd checks once, under the watchdog through hook_resolving); crates/anthology-shell/src/path_vars.rs (PathVars trait, path_vars_allowed text and word check); collect.rs read_path_vars in add_one puts the canonical path in place and drops the holder, and pushes the value-as-written reading into the alternates loop (its raw words are the literal ones, so G4 sees a link named agent-browser); analyze_with_vars re-runs the analysis with None when the check fails, so any failure gives the old verdict exactly.
Review lesson (security review of PR 217, Critical): a check on the raw command text is beaten by quote splitting when the shell acts after quote removal. zsh typeset -m 'TOOL<'-'>=x' set TOOL1 past a raw-text rule; the fix checks the analysis words (quotes removed) for < or > and refuses -m on typeset-like builtins. Check new name-setting forms on Entry.raw, not on the text.
Why the tests look odd: the text check and the raw-word name check overlap. An unread "$NAME" form leaves $NAME in a raw word unless it is a redirection target, so guard-level kills for the quote sub-checks use redirection targets, and the closing-quote, word-end and quote-tracking checks are only killed by the unit tests in the shell module.
How to apply: rerun one mutant per sub-check and per list entry (DENIED, DENIED_PREFIXES, TEMP_DIRS, TEMP_VARS, PATTERN_NAMERS) after any change. Expected survivors: the used.is_empty() guard (skips a second analysis) and the literal reading only when something was read (keeps atl's work budget identical); /var/folders is only killed on macOS.
