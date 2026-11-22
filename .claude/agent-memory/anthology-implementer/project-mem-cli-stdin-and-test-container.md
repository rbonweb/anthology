---
name: 'anthology-mem CLI: stdin lock and container runs'
description: 'main holds the stdin lock, so mem reads /dev/stdin; how to run anthology-memory tests in Docker with a mounted worktree'
metadata:
  type: project
  modified: 2026-11-22
---

The anthology binary main holds std::io::stdin().lock() for the whole run, so a command that locks stdin again hangs forever. anthology-mem save --body-file - therefore opens /dev/stdin (crates/anthology-memory/src/index.rs, read_body_file). Why: anthology-cli is outside the memory slice, and its mem::run ignores the stdin it is given.
The anthology-memory integration tests spawn the real binary, so run cargo build -p anthology-cli before cargo test -p anthology-memory. In a container the worktree .git file points into the main checkout, so mount the git common dir read-only at its own path or git calls fail with not a git repository. Use cargo test --no-fail-fast: cargo stops at the first failing test binary otherwise.
DEFAULT_BUDGET (crates/anthology-memory/src/render.rs) is 150 lines / 8000 bytes; the mem golden (scripts/dev.sh golden bless --surface mem) changes whenever the save usage text changes.
