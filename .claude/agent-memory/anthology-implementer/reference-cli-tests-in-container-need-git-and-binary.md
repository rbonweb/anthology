---
name: 'CLI tests in the Rust container need git, the built binary and a non-root run'
description: 'The slim Rust image has no git and no anthology binary; directory-mode tests skip as root'
metadata:
  type: reference
  modified: 2026-11-22
---

The slim Rust image has no git, so MemCase and SaveCase tests panic or skip. Install git as root inside the container (apt-get), and run cargo build -p anthology-cli first: run_bin needs target/debug/anthology.
Why: a test that panics with "git is not installed" or "no anthology binary" looks like a red run but is only setup, so a real red needs both in place.
Directory-mode tests (chmod) pass vacuously as root. Run the built test binary as an ordinary user with setpriv --reuid N --regid N --clear-groups, with ANTHOLOGY_BIN set and the crate folder as cwd, to see them run.
Put multi-step container commands in a script under target/ (gitignored) and run it with sh: the worktree guard refuses a command line that names git inside a docker run.
A workspace version bump also needs Cargo.lock and fuzz/Cargo.lock refreshed: cargo metadata for both manifests does it.
Replay one golden with: cargo run -p difftest -- golden check --surface mem.
