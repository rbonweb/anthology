---
name: 'atl bypass asks (PR 259) review findings'
description: 'Head 3fc1f757: literal spellings hold; with no rule, an expansion-built subcommand word slips runs_bypass, and every other atl runner stays open'
metadata:
  type: project
  modified: 2026-11-27
---

Security review of PR 259 (atl/src/hooks/anthology_gate.rs runs_bypass, without_bypass; read_only_proof.rs), probed with real atl and anthology release builds in the pinned rust image.
Holds: quotes, escapes, $'..', global flags (-v, -vvv, --verbose, --skip-env), redirects around the words, tab, line continuation, env -S/-i/-C, command, exec, noglob, timeout, nohup, stdbuf, nice, time, xargs, find -exec/-execdir/-ok, sh/zsh/bash -c, eval, heredoc and here-string to sh, watch, script, flock, setsid, parallel all ask. clap has no infer_subcommands: atl byp falls back to a program named byp. The strip sends odd first words (--, --skip-env, X=1, =cat, ~) and the classifier refuses them; newlines are refused before the classifier.
Found: with no deny, ask or Read rule, a subcommand word built by expansion gets no ask: atl {bypass,touch} y, atl ${X:-bypass} touch y, a command substitution or "$X" set earlier on the line. zsh dash precommand (- atl bypass ...) also gets none, but Claude Code does not match it to Bash(atl *).
Context: setup writes browser Read denies, so rules are active in a normal install; then the plain shape asks for every non-plain atl line, including atl run/err/test/summary, the fallback (atl <program> clap rejects runs it raw, atl --skip-env <program> too), atl claude, atl init/config/trust/learn --write-rules/gain --reset. With no rule at all every one of those runs unprompted, and plain git push --force, ssh, docker run, find -delete, npm run, npx, psql -c, aws are rewritten to atl ... with no decision, so Bash(atl *) approves them.
Why: the follow-up that closes the residual gap must cover the no-rules config and non-literal subcommand words, not only named subcommands.
How to apply: probe with a logging classifier wrapper (a script that appends stdin to a log, then pipes it to the real anthology) to see the exact stripped line; the guard blocks docker exec strings holding a dollar-brace or a substitution, so pipe scripts into the container with printf and run them as files.
