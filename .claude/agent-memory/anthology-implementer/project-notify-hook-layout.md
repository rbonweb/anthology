---
name: 'Where the attention-alert hook and its pinned tests live'
description: 'notify.sh sits in plugins/anthology/hooks, tests are Rust in crates/repo-tests, three places pin the hooks.json shape'
metadata:
  type: project
  modified: 2026-11-22
---

The Notification alert is plugins/anthology/hooks/notify.sh, called as `sh notify.sh approval|idle` from plugins/anthology/hooks/hooks.json. Its tests are crates/repo-tests/tests/notify.rs.
Adding a hook event to that hooks.json also breaks: crates/repo-tests/tests/cutover_hooks.rs (event list, per-hook timeout 60), crates/repo-tests/tests/shims/statics.rs (exact JSON literal) and crates/anthology-runtime/tests/rtk_install.rs (sorted event names).
ci/jobs/shellcheck.sh lists every shim by name, so a new hooks/*.sh must be added there or CI never lints it.
Run Rust tests in rust:1.100-slim (digest in ci/images/rust.Dockerfile) with -e RUSTUP_TOOLCHAIN=1.100.0 -e CARGO_TARGET_DIR=/w/target, mounting only the worktree. The toolchain file asks for musl targets and would try to download them.
Why: each of these failed only after the hook change, in tests unrelated to it.
How to apply: grep crates for the existing event names before editing hooks.json. Delete leftover Python-era pycache folders (plugins/anthology/scripts, tests/) before running cutover_hooks: it fails if plugins/anthology/scripts exists.
