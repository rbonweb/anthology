---
name: 'docker exec psql -c literal reading (PR 247) review findings'
description: 'Round 2 head 98ae962f: earlier repros hold; --command values go unchecked and zsh (a|b) glob groups hide psql words; several -c unmask backslash meta-commands'
metadata:
  type: project
  modified: 2026-11-27
---

PR 247 (base a5529497) reads docker exec psql -c SQL as data when the command is plain literal (crates/anthology-shell/src/shell_args.rs docker_exec_psql and psql_words_known, literal_command in collect.rs add_one).
Round 1 (head 267e62f6): Critical zsh numeric glob c<1-5> hid a word before psql; Important psql -o, --output= and -f - with a here-string went base G0 to head Allow.
Round 2 (head 98ae962f) closed both by refusing any redirect or here-string and a positional allowlist after psql. Held: -v PROMPT1, -Ao, -tf, --, >2 names, conninfo, --out and --comm abbreviations, =(...) procsub, ANSI-C backslash.
Round 2 gaps, runtime proven in postgres:16 with zsh 5.9: the --command value is never a script start (is_shell_c_flag needs one dash), yet psql_words_known accepts it, so -c "select count(*) from t" --command with a backslash-bang or backslash-o pipe value is base G0, head Allow and runs. A zsh glob group after psql, c psql -c SQL (-o|-x) with a pipe word, is base G0, head Allow: the lexer splits at the paren, zsh expands it to -o when a file -o exists in cwd. Several -c values unmask a backslash-copy to program value (base G0, head Allow).
Pre-existing on base and head: docker exec c (flock|x) psql -c "select 1; MARKER" is Allow, zsh runs flock with a file flock in cwd (proven). A lone --command or backslash-copy program value is Allow on base.
Harmless: psql does not read stdin when -c is given; SQL in -c goes to the server with no psql interpolation, so a gexec or :var in SQL never becomes a meta-command; backslash-bang does not expand :var.
Why: the literal check is per lexer segment, and the lexer does not model zsh glob groups; a skipped value list and the script scanner must name the same options.
How to apply: in later rounds probe (a|b) groups before and after psql, --command, and backslash meta-commands next to a count(*) value.
