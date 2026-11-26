---
name: 'docker exec psql -c literal reading (PR 247) review findings'
description: 'Head 267e62f6: every PR 193 shape holds; a zsh numeric glob hides a word before psql (Critical); psql -o and -f - unmask pre-existing runs'
metadata:
  type: project
  modified: 2026-11-26
---

PR 247 (head 267e62f6, base a5529497) reads docker exec psql -c SQL as data when the command is plain literal (crates/anthology-shell/src/shell_args.rs docker_exec_psql, literal_command in collect.rs add_one).
Held on head (same verdict as base): every PR 193 split-word, expanded-value, planted-PATH, env-file, su/flock/-u psql, wrong-name, attached -u=x, --, compose -T and -f, env -S, xargs, watch, parallel, sg, eval shape; ANSI-C backslash forms; alias -g lines (G0).
Critical regression, runtime proven in zsh 5.9: zsh reads c<1-5> (also c<->, u<1-5> as an option value, c<1-5>flock) as a numeric glob word, the guard reads a < redirect plus a > redirect that swallows the next word. touch c1; docker exec c<1-5> flock psql -c "select 1; MARKER" is base G4, head Allow, and zsh runs docker exec c1 flock psql -c ..., so flock runs the value with sh. bash errors on the redirect and runs nothing.
Important, runtime proven in postgres:16: psql -o with a pipe value, --output=, and -f - fed by a here-string run a shell; with count(*) SQL these went base G0 to head Allow. Pre-existing: the same with paren-free SQL is Allow on base. COPY TO PROGRAM is Allow on both. ssh h docker exec ... with a joined ; value is Allow on base.
Harmless head Allows checked: sudo -s and -i (sudo 1.9.16 backslash-escapes the joined words), ANSI-C \0134 (VT then 4 in bash and zsh), PGDATABASE (libpq does not expand it as a conninfo), psql -- -c, -v PROMPT1 (no prompt with -c).
Why: per-word literal checks still trust the guard lexer, which does not model zsh numeric globs outside readonly.rs; and psql options other than -c can run programs.
How to apply: in any later round, probe zsh numeric globs between exec and psql and the psql -o and -f options; the probe and runtime case files from this round used a G4 marker program.
