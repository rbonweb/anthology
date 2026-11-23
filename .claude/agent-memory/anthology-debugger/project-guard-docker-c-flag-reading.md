---
name: 'Guard docker -c reading: psql data only by exact position, no backslash'
description: 'Why docker exec psql -c SQL was G0, the final rule that fixes it, and what stays fail-closed'
metadata:
  type: project
  modified: 2026-11-23
---

crates/anthology-shell/src/collect.rs add_one calls docker_script_starts_in (crates/anthology-shell/src/shell_args.rs) for a docker or docker-compose command. The base reading takes any word matching -[A-Za-z]*c[A-Za-z]* as a shell -c flag and analyses the next word as a script, so SQL such as count(*) failed to parse and the command was G0.
Final rule (PR 193): the -c words of psql are skipped only when (1) the command is docker exec, docker compose exec or docker-compose exec, (2) the options before the container are all in EXEC_VALUE_FLAGS or EXEC_BOOL_FLAGS (any unknown option, attached forms such as -u=x, docker run: old reading), (3) the word after the container is exactly psql (case-sensitive, no path), and (4) the -c value holds no backslash, because psql runs a shell for \! and \o |. A shell word after psql ends it. script_starts_in itself (frozen golden surface shell.segments script_starts, used by the lenient lexer) stays unchanged and a test in crates/anthology-shell/tests/lexer.rs pins that.
Why: the first version (any word named psql, plus a runner denylist) let su psql -c, flock /tmp/psql -c, -u psql c python3 -c and psql -c with a backslash command drop a script, losing G3/G4/G6 and G0 (security review found it). Position, not name, is the rule.
Docker exempts G1 and G2 for the whole docker command (frozen row test_allowed_docker_exemption uses docker-compose run and docker compose run), so docker exec c npx playwright install is Allow by design; G3, G4 and G6 still apply inside docker. Use G4 (agent-browser open x) as the marker in tests that prove a script is still read.
Still G0 inside docker exec: grep -c "a(b)", python3 -c "print(1)", gcc -c, docker compose exec -T (unknown option). Widen only by adding listed options or programs after checking.
How to prove a new branch: mutate each branch by hand (value-flag skip, boolean skip, unknown-option fallback, exact name, backslash, shell reset, compose arms, docker run, call site, script_starts_in pin); every one must fail a test in crates/anthology-guard/tests/g0_fp.rs or crates/anthology-shell/tests/lexer.rs. A git checkout of origin files over uncommitted edits loses them: commit before taking a base probe.
The golden check on guard.check, guard.hook, shell.analyze and shell.segments showed 0 unexplained diffs, so no bless was needed.
