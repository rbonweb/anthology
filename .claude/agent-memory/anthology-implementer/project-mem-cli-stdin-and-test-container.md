---
name: 'anthology-mem CLI: stdin, body-file policy, container runs'
description: 'mem reads fd 0 by dup (main holds the stdin lock); --body-file path policy reuses the shell crate secret list; how to run anthology-memory tests in Docker'
metadata:
  type: project
  modified: 2026-11-22
---

The anthology binary main holds std::io::stdin().lock() for the whole run, so a command that locks stdin again hangs forever. anthology-mem save --body-file - therefore duplicates fd 0 with stdin().as_fd().try_clone_to_owned() (crates/anthology-memory/src/index.rs, read_body_file). Why: anthology-cli is outside the memory slice, and its mem::run ignores the stdin it is given.
A --body-file path is canonicalized, must be inside the repo or a temp folder, and must not be a secret location; the secret list is anthology_shell::is_secret_path, so memory depends on anthology-shell (update Cargo.lock and fuzz/Cargo.lock by hand). The file is stat-ed before it is opened (a FIFO would block the open) and refused, never cut, when over 16384 raw bytes.
The anthology-memory integration tests spawn the real binary, so run cargo build -p anthology-cli before cargo test -p anthology-memory. In a container the worktree .git file points into the main checkout, so mount the git common dir read-only at its own path or git calls fail with not a git repository. Use cargo test --no-fail-fast: cargo stops at the first failing test binary otherwise.
DEFAULT_BUDGET (crates/anthology-memory/src/render.rs) is 150 lines / 8000 bytes. The mem golden changes whenever the save usage text changes, and hook.session-start (runtime crate) changes whenever the session-start pointer suffix changes: bless both with scripts/dev.sh golden bless --surface.
