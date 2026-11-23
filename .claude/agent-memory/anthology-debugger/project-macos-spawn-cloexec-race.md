---
name: 'macOS spawn can block on another thread child (RealGit)'
description: 'Why a_hung_git_is_killed_at_the_timeout took 30 s on macOS CI, and what bounds git spawn now'
metadata:
  type: project
  modified: 2026-11-23
---

On macOS, Rust std creates pipes with pipe() then sets FD_CLOEXEC in a second step, and Command::spawn forks (not posix_spawn) when PATH is in the child env and the program is not a path. A fork by another thread in that window gives its child the status pipe, and spawn blocks until that child or its descendants (a sleep 30) exit. Linux uses atomic CLOEXEC, so it cannot reproduce there; only macOS CI shows it. Fix in crates/anthology-core/src/git.rs: spawns take turns under the SPAWN mutex and run on a helper thread the caller waits for only until the deadline. The hand-over channel must be sync_channel(0): with a buffered channel a late Child sent after the caller timed out is dropped unkilled and unreaped with the receiver. A helper that gets its turn after the deadline starts nothing. Unit tests with cfg(test) hooks in git.rs pin both. The stress test in tests/git.rs passes even without the lock, so it is only a smoke test. crates/anthology-cli/src/cmd/perm/git.rs runs git by absolute path, so std uses posix_spawn there and spawn cannot stall; it can only see a sibling pipe for that git short life.
