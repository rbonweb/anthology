---
name: 'Shell classifiers must model zsh, BSD userland and the Claude Code snapshot shims'
description: 'Real runtime for Bash tool calls is zsh on macOS with BSD tools, find shadowed by bfs and grep by ugrep; GNU/bash models give false read-only proofs'
metadata:
  type: project
  modified: 2026-11-22
---

The Bash tool runs in the user's zsh on macOS. Claude Code's shell snapshot also runs unalias -a and defines functions that shadow find (bfs) and grep (ugrep, with --hidden and a fallback to the system grep for --null, -z, --filter and similar), and rg when none is installed.
Why: the P3 review of PR 147 (crates/anthology-shell/src/readonly.rs) found two Critical false proofs that a GNU and bash model cannot see. (1) BSD cat, head, tail, wc, du, stat, realpath, ls and uniq stop option parsing at the first operand, so in head a -n 5 the words -n and 5 are files, and uniq a -c writes to ./-c, through a symlink too. (2) zsh reads <1-5> as a numeric glob, so cat <1-5>/dev/null reads 3/dev/null, and cat <1-5>&2 x backgrounds cat and runs a command named 2. Also, zsh expands an empty quote followed by =cmd (EQUALS).
How to apply: for any parser, gate or classifier review, probe zsh-only lexing and BSD option order on the host in a temp dir (harmless commands only), not just bash in the container. Check which program really runs (snapshot functions) before trusting a GNU option table.
