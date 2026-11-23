---
name: 'macOS spawn can block on another thread child (RealGit)'
description: 'Why a_hung_git_is_killed_at_the_timeout took 30 s on macOS CI, and what bounds git spawn now'
metadata:
  type: project
  modified: 2026-11-23
---

On macOS, Rust std creates pipes with pipe() then sets FD_CLOEXEC in a second step, and Command::spawn forks (not posix_spawn) when PATH is in the child env. A fork by another thread in that window gives its child the status pipe, and spawn blocks until that child or its descendants (a sleep 30) exit. Linux uses atomic CLOEXEC, so it cannot reproduce there; only macOS CI shows it. Fix in crates/anthology-core/src/git.rs: spawns take turns under the SPAWN mutex and run on a helper thread the caller waits for only until the deadline. Any other in-process Command spawn on a thread (for example crates/anthology-cli/src/cmd/perm/git.rs run_git) is still exposed, but its children are short-lived. Diagnose a 30 s test time by asking which sibling test left a sleeper of that length.
