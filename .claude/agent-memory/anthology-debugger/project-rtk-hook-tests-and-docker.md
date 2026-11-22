---
name: 'rtk-hook tests live in anthology-runtime, run in own Docker'
description: 'Where the rtk-hook shell tests are and how to run them without scripts/rust-test.sh'
metadata:
  type: project
  modified: 2026-11-22
---

The rtk-hook wrapper tests are crates/anthology-runtime/tests/rtk_install.rs (module wrapper_test), not crates/repo-tests. A fake rtk is written by answer_fake; W::tools_dir builds a PATH holding only sed/head/grep/tr/cat symlinks plus named stubs, so a test controls which programs the hook can see. Never pass a tool name already symlinked as an extra stub: fs::write would write through the link into the real binary.
Why: scripts/rust-test.sh mounts the shared git dir and unnamed shared volumes; for a per-worktree container write a throwaway script copying its inner block with docker run --name and own volumes, and make it executable if scripts/dev.sh calls it through ANTHOLOGY_RUST_TEST.
How to apply: run cargo test --locked -p anthology-runtime --test rtk_install in that container; shellcheck on bin/rtk-hook is not in ci/jobs/shellcheck.sh, run koalaman/shellcheck from ci/images/shellcheck.Dockerfile by hand.
