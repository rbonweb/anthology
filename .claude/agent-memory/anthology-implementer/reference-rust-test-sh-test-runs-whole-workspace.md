---
name: 'rust-test.sh test -p narrows the cargo scope, but sessions are the fast path'
description: 'test -p X drops --workspace (still runs plain and all-features); run cargo test -p for one test file'
metadata:
  type: reference
  modified: 2026-11-27
---

scripts/rust-test.sh test -p <crate> drops --workspace (rust_test_script.rs pins it) and still runs the plain and the --all-features pass, so it builds and runs twice. For one test file use: scripts/rust-test.sh run cargo test --locked -p <crate> --test <file>. Start a session first (scripts/rust-test.sh session-start anthology-release-NAME, then export ANTHOLOGY_RUST_SESSION=NAME) so the target stays incremental between runs; session-stop removes the volume. A golden check for one surface: scripts/dev.sh golden check --surface <name>.
