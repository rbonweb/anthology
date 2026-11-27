---
name: 'Guard known bypasses closed in PR 257: where each fix lives'
description: 'Option-run commands, zsh arithmetic backtick and zsh glob group: functions, exemptions, runtime facts, mutants'
metadata:
  type: project
  modified: 2026-11-27
---

PR 257 (branch fix/guard-known-bypasses, workspace 0.2.7) closes three bypasses that were on build before it. Where each is closed:
1. Programs that run a program from an option. crates/anthology-shell/src/prefixes/hidden.rs option_commands lists them: rg --pre and --hostname-bin (abbreviation of at least 4 characters), grep/egrep/fgrep/ugrep/ug --filter (comma list of exts:command) and --pager (attached value only), sort/gsort --compress-program (abbreviation of at least 4). less_command reads LESSOPEN and LESSCLOSE (a leading pipe or dash is a mode). Three uses: the values become scripts (prefixes.rs strip_prefixes tail, assignment_scripts, and the exporter loop in collect.rs add_one) so an install in them is G1; runs_option_command marks them for atl; runs_unread_command and sets_less_command make add_one set unproven (G0) unless the whole value is in HARMLESS_VALUES or is a sort compressor.
2. Escaped backtick in arithmetic: lexer.rs escaped_backtick, entered from the run loop when arith or arith_text is set; the pair is lexed as a command, an unclosed one sets unproven.
3. Glob group among a command's words: lexer.rs glob_group, called where the paren branch sets unproven. It flags a paren after a blank when the segment already has words, except after if/then/else/elif/while/until/do/!/{, after function NAME, case ... in, for ((, and where glob_qualifier says no. It stays quiet when swallowed (unclosed heredoc) or cut (a heredoc body cut at a line holding a paren in a dollar-paren reading): fw_h tests ordinary_100_kb_commands_stay_clear and review_round_0_findings_are_closed need that, and the Exact pass (the one zsh shares) lexes the text again without cuts.
Why G0 for a bare program value and not only a script: rg runs PROG FILE, ugrep pipes the file, sort passes its temp file, so rg --pre sh x p.sh runs sh p.sh and rg --pre=playwright x install runs playwright install; the text cannot show it.
Runtime facts (trixie: bash 5.2.37, zsh 5.9, ugrep 7.4.2, ripgrep 14.1.1): rg --pre takes a program path only and rg 14 has no abbreviations; ugrep --filter needs the equals form and --pager runs its value; ugrep --exec and -Q run nothing; sort runs the compress program only when it spills to temp files (use a big file and -S 100k); a zsh glob group needs a pipe, so (a) alone did not run the marker in zsh 5.9.
Known limit: lenient.rs (unbalanced quote fallback) does not flag a glob group unless the text has a pipe, which already makes it G0.
Goldens: three generated cases went from Allow to G0 and were blessed (guard.check, guard.hook); shell.analyze and shell.segments had no diffs.
Mutants: cargo mutants --in-diff, -p anthology-shell, tests analyze, lexer, fw_i and lib only (about 25 s per mutant) leave plain_text and two add_one or-chain lines (not mine) and the equivalent less-than versus less-or-equal in take_heredocs (index equals text length, nothing follows).
Why: reviewers proved all three on build; they were not regressions.
How to apply: add a new program-running option to option_commands and its rows in the three test tables (guard known_bypasses.rs, shell analyze.rs and fw_i.rs).
