---
name: 'Known bypasses close (PR 257) review findings'
description: 'Head 2942a4c0: three fixes sound; rg --pre=vim / ugrep --filter vim stay Allow and execute via HARMLESS_VALUES; awk and config-file vectors pre-existing'
metadata:
  type: project
  modified: 2026-11-27
---

PR 257 (base fc3d2f74, head 2942a4c0) closes three bypasses on build: option-run programs (rg --pre/--hostname-bin, ugrep --filter/--pager, sort --compress-program, LESSOPEN/LESSCLOSE), zsh escaped backtick in an arithmetic subscript, and an unquoted zsh glob group among a command words.
All three verified sound in bash 5.2.37 and zsh 5.9 (debian trixie, rg 14.1.1, ugrep 7.4.2), proven by direct check() verdicts on both trees plus a marker program.
Arithmetic: base G0 to head Allow for (( .. )), $(( .. )), nested a[b[..]], and ++ forms, but only when the escaped-backtick body is benign; dangerous bodies (an install) are G1, and let, typeset -i and [[ -eq ]] stay G0. No regression.
Paren rule: head G0 for every (a|b) after a blank following command words (attached f(lock|oo) too); after ;, &&, ||, |, &, inside [[ ]], case patterns and function bodies stay Allow (subshell/pattern). time (cmd) and ls (a|b) && echo are now G0 (intended). Sound.
Residual gap the PR nearly closes: rg --pre=vim / --pre=vi and ugrep/grep --filter=EXT:vim|vi are Allow on head yet run arbitrary commands from the searched file (vim executes the file :! lines via stdin). Cause: vi and vim are in HARMLESS_VALUES, which hidden::option_commands consults, though the PR ruling lists only cat, less, more and the sort compressors as harmless. rg --pre=cat, nano, less, more do not execute (nano/pager need a tty). Fix: drop vi/vim (and nano) from the harmless set used for these file-feeding option/LESS contexts.
Not this PR (pre-existing Allow on base and head, out of scope): awk BEGIN{system("cmd")} runs (proven); config-file vectors RIPGREP_CONFIG_PATH=f rg, ugrep --config=f / ---f / a cwd .ugrep run their option command (proven). man -P, ssh -o ProxyCommand and GIT_PAGER are Allow but tty-gated and did not execute non-interactively.
Method: ugrep and rg (clap) reject long-option abbreviations, so the guard mins (--pre 4, --filter 8, --pager 7) over-match harmlessly, no gap there.
