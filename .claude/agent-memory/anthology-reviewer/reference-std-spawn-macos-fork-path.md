---
name: 'When std Command::spawn forks on macOS, and what it then waits for'
description: 'Rust 1.100 std: macOS pipes are pipe() then FD_CLOEXEC, fork path when PATH is set and the program has no slash, and spawn reads a status pipe'
metadata:
  type: reference
  modified: 2026-11-23
---

Checked against rust-lang/rust 1.100.0 library/std/src/sys/process/unix/unix.rs and sys/pipe/unix.rs.
macOS (and every target not in the pipe2 list: android, dragonfly, freebsd, hurd, illumos, linux, netbsd, openbsd, cygwin, redox) creates pipes with pipe() and sets FD_CLOEXEC on each end afterwards, so a fork by another thread in that window inherits them.
posix_spawn is skipped (fork plus exec) when uid, gid, groups, chroot or pre_exec closures are set, or when the Command env sets PATH and the program contains no slash. Only the fork path makes spawn read a status pipe until every write end closes; a posix_spawn path has no status pipe, so it cannot stall that way. Apple posix_spawn in std does not set POSIX_SPAWN_CLOEXEC_DEFAULT, so posix_spawn children inherit leaked fds too.
Why: PR 196 rested on these facts; crates/anthology-cli/src/cmd/perm/git.rs runs git by absolute path, so it takes posix_spawn and cannot stall in spawn, only see a leaked stdout or stderr pipe.
How to apply: when judging a spawn stall or fd leak, first check program_is_path and env PATH to know which path std takes.
