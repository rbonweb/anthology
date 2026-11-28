---
name: 'Slop gate: repo-wide before/after scan in the container'
description: 'How to count gate hits per rule over every tracked file, with a baseline binary from a clean archive'
metadata:
  type: reference
  modified: 2026-11-28
---

Baseline: extract the base commit into a throwaway folder inside the worktree (git archive HEAD piped to tar -x -C that folder), build anthology-cli there with --target-dir /target/base in a rust-test.sh session container (ANTHOLOGY_RUST_SESSION=<name> scripts/rust-test.sh run ...). Write the file list with a redirect (git ls-files -z > folder/files.lst) because a container command that names git is refused in a worktree-isolated agent. Scan: xargs -0 /target/base/debug/anthology slop check < folder/files.lst > /target/before.txt, then the same with /target/debug/anthology for after; xargs exits 123 when any file has a finding, so that is not an error. Count with grep -c on the rule id. Why: editing the source while the first build runs would contaminate the baseline, so the baseline must come from a clean archive; delete the folder before committing. The 2026-11-28 baseline: 20 text.ai-vocab, 1 chat-residue, 2 not-x-but-y, 0 hype-cta over 3814 tracked files.
