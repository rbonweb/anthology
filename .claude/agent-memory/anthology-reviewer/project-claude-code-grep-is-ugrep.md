---
name: 'The snapshot grep is ugrep, whose options differ from GNU grep'
description: 'ugrep -NUM is a recursion depth and -d has dereference-recurse; GNU grep -d takes abbreviations; check option meaning per real program'
metadata:
  type: project
  modified: 2026-11-22
---

In the Claude Code shell snapshot, grep is a function that runs the embedded ugrep (ARGV0=ugrep, with --hidden, --ignore-files and -I added). In ugrep, -1, -2 and so on set a recursion depth and turn on -r, where GNU grep reads them as context lines. ugrep -d also takes dereference-recurse (same as -R). GNU grep -d and --directories accept unambiguous abbreviations (-d rec recurses). In the P3 fix round of PR 147, the classifier (crates/anthology-shell/src/readonly.rs, GREP row) kept digits: true and only compared the -d value with the exact word recurse, so grep -5 KEY src was proven with recursive false while ugrep read src/sub/.env.
Why: a table row written from GNU docs gives a wrong recursive flag or a false proof on the real runtime.
How to apply: for any grep option table, dump the help of the embedded ugrep (ARGV0=ugrep on the claude binary path found in the newest shell snapshot, --help WHAT) and test GNU behavior in debian:bookworm-slim. Also note: diff with two directories reads every common file in them, and macOS APFS is case-insensitive, so name checks on secret paths must ignore ASCII case.
