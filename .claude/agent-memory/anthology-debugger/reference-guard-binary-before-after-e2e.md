---
name: 'Prove a guard fix on the real binary in bash and zsh, before and after'
description: 'Build base and branch binaries, probe container recipe, verdict plus marker per payload'
metadata:
  type: reference
  modified: 2026-11-27
---

To show a guard fix on the real binary (not only in tests): build the old and the new anthology binary, then run every payload through the hook and through bash and zsh. Steps used for PR 257: git archive the base into a scratch folder inside the worktree, build it in the rust session container with CARGO_TARGET_DIR=/target/before (scripts/rust-test.sh run sh -c 'cd /w/scratch && CARGO_TARGET_DIR=/target/before cargo build -p anthology-cli --locked'), build the branch the normal way, docker cp both binaries into the worktree, and start a probe container from debian:trixie-slim with the worktree mounted read-only (apt: ugrep ripgrep zsh bash less findutils jq; trixie has bash 5.2.37, zsh 5.9, ugrep 7.4.2, ripgrep 14.1.1). A script loops over one payload per line: feed {tool_name Bash, tool_input command} built with jq to the binary as guard hook (exit 0 allow, exit 2 block), then run the payload with PATH holding a marker npx that appends to a file, once in bash and once in zsh, in a fresh work dir with files f, a.txt, p.sh, install, big (seq 300000) and an empty file named flock; a fake docker script drops its first two words and execs the rest, so docker exec c ARGS runs ARGS. Print verdict, bash and zsh marker result; ALLOW with a marker is a bypass. Delete the scratch folder and binaries before committing.
Why: the unit tests only prove the analysis; the run in both shells proves the payload really executes and that the fix blocks it, and the everyday lines show nothing broke.
How to apply: keep the case file free of lines that fail to parse in bash (a zsh-only glob group parses in zsh only); create a file named like the first glob alternative or the zsh case does not run; the guard binary reads HOME, so set it.
