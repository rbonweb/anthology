---
name: 'Shell analysis Entry.cwd is lowercase and already holds git -C'
description: 'anthology_shell::analyze lowercases the folders it tracks and folds the last -C word into Entry.cwd; how privacy.rs takes it back out'
metadata:
  type: project
  modified: 2026-11-28
---

anthology_shell::analyze gives each Entry a cwd that is (1) lowercased (collect.rs normalize_in uses pystr::lower), so on a case-sensitive disk a cd into a folder with capitals names another folder, and (2) for git, tar and make already holds the last `-C` word among all the args, resolved against the cd folder, with no adding up of several -C. That includes `git commit -C <commit>`, where the value is a commit, not a folder. A consumer that wants the real folder must put the case right against the disk (read_dir, compare lowercased names) and take the -C back out. crates/anthology-cli/src/cmd/privacy.rs does both: commit_dir (one global -C and no stray -C: use entry.cwd as is; otherwise strip the lowercased last -C value from the end of entry.cwd and apply the options itself) and exact_case. Why it matters: the phase 2 plan said dir = JSON cwd, then entry.cwd, then -C, which doubles the -C folder and fails on a disk that cares about case; tests on Linux caught it because tempfile folder names hold capitals.
