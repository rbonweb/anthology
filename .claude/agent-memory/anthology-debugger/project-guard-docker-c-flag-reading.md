---
name: 'Guard reads every -c word inside docker as a shell script'
description: 'Why docker exec psql -c SQL was G0, where the reading lives, and what stays fail-closed'
metadata:
  type: project
  modified: 2026-11-23
---

crates/anthology-shell/src/collect.rs add_one calls docker_script_starts_in (crates/anthology-shell/src/shell_args.rs) for a docker or docker-compose command: any word matching -[A-Za-z]*c[A-Za-z]* is read as a shell -c flag and the next word is analysed as a script, so SQL such as count(*) failed to parse and the command was G0. Fixed in 0.1.6 with the DATA_C_PROGRAMS allowlist (only psql) and a SHELL_RUNNERS reset (su, sudo, env, docker, ...). script_starts_in itself is a frozen golden surface (shell.segments op script_starts) and the lenient lexer uses it, so it stays unchanged; only the docker call site uses the data-aware variant. Still G0 inside docker exec: grep -c "a(b)", python -c "print(1)", gcc -c; add a program to DATA_C_PROGRAMS only after checking that its -c takes data. Docker exempts G1 and G2 for the whole docker command, so docker exec c npx playwright install is Allow by design (frozen row test_allowed_docker_exemption); only a substitution in the argument, which runs on the host, blocks. The golden check on guard.check, guard.hook, shell.analyze and shell.segments showed 0 unexplained diffs, so no bless was needed.
