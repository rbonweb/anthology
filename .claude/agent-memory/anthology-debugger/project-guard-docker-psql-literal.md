---
name: 'Guard docker exec psql -c data reading (PR 247, redo of 193)'
description: 'The plain-literal rule that lets psql -c SQL through docker exec, what stays G0, and how the mutants were run'
metadata:
  type: project
  modified: 2026-11-26
---

PR 247 (branch fix/guard-docker-psql-literal, workspace 0.2.6) redoes closed PR 193. crates/anthology-shell/src/collect.rs add_one computes literal_command (no holder, no subst, no arith, no braces, words.ends_with(cmd), no xargs-style peel) and passes it to docker_script_starts_in in shell_args.rs. That skips a psql -c value only when docker_exec_psql parses exec positionally over listed options with every word matching literal_word, psql is exact, and psql_value_is_data (no backslash, dollar, backtick, tilde, caret, hash).
-e/--env is allowed only for libpq connection variables (PSQL_ENV): PGPASSCOMMAND, PAGER, PSQLRC, LD_PRELOAD and PATH set what runs; --env-file is not listed.
Why: per-word checks miss run-time splitting and expansion; the lexer drops quotes, so the only way to tell a quoted * from a glob is that an unquoted one makes a Holder (glob), hence the whole-command no-holder rule.
Tests: crates/anthology-guard/tests/g0_fp.rs, shapes tables keep the pre-change verdict (G4 for marker, G0 for SQL). Pre-existing, still open: psql -c ^x and -cVALUE attached are Allow, COPY TO PROGRAM, PSQLRC.
Mutants: cargo install cargo-mutants in the session container (scripts/rust-test.sh session-start anthology-release-NAME, then run), cargo mutants --in-diff FILE -p anthology-shell --test-package anthology-guard. After a mutants run touch the source files before the next cargo test: /target keeps a stale mutant build and tests fail oddly.
rust branch is an ancestor of build and has the same docker call site, so the fix reaches it by merge, no separate port.
