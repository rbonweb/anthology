---
name: 'docker exec psql -c data reading (PR 193) review findings'
description: 'Round 1 (8850f677) and round 2 (666ce056) gaps: backslash and runner fixed, then expansion words split or fill in what the guard reads as literal'
metadata:
  type: project
  modified: 2026-11-23
---

PR 193 makes docker_script_starts_in (crates/anthology-shell/src/shell_args.rs) skip the -c values of psql inside docker exec. Inside docker G1 and G2 are exempt (frozen test_allowed_docker_exemption rows), but G3, G4 and G6 still apply to scripts read from -c words, so a dropped script loses them.
Round 1 (head 8850f677): any word named psql triggered it; psql -c with \! or \o |cmd, su psql -c, flock /tmp/psql -c and -u psql c python3 -c all went to Allow. Fixed in round 2 by a positional parser (allowlisted exec options, container, then exactly psql) and a no-backslash rule.
Round 2 (head 666ce056) regressions, base G4 or G0, head Allow, runtime proven in postgres:16-alpine under zsh and bash: a word before the program that the shell splits or expands (docker exec $(echo c su) psql -c CMD, {c,su}, unquoted $C, zsh ${=C}, c*, also inside an option value such as -e $(echo A=b c su)); a -c value whose backslash arrives by expansion (Q=backslash-bang CMD then psql -c "$Q", "${B}! CMD", -c *, zsh ${(#):-92}). Name trust: a plant such as cp /bin/sh /usr/local/bin/psql in the container is Allow on base and head, and -e PATH=... or --env-file are on the allowlist.
Pre-existing, not regressions: host psql is unmodeled; psql -o |cmd, --command=, attached -cVALUE, PAGER or PSQLRC env and COPY TO PROGRAM are Allow on base for paren-free SQL; sh -c "$(cat f)" is Allow on base.
Tests: every round-2 branch mutant dies except passing the literal docker at the call site, which is equivalent for exec.
Why: per-word literal checks miss runtime word splitting and expansion; the fix must require every word from exec through psql and each skipped value to be a plain literal (no holder: param, substitution, glob, brace).
How to apply: re-run the probe cases with the split-word and expanded-value shapes in any further round.
