---
name: 'Where the hook scripts and their pinned tests live'
description: 'notify.sh and subagent-stop.sh sit in plugins/anthology/hooks, tests are Rust in crates/repo-tests, four places pin the hooks.json shape'
metadata:
  type: project
  modified: 2026-11-24
---

The Notification alert is plugins/anthology/hooks/notify.sh, called as `sh notify.sh approval|idle` from plugins/anthology/hooks/hooks.json. Its tests are crates/repo-tests/tests/notify.rs.
The SubagentStop nudge is plugins/anthology/hooks/subagent-stop.sh (fail-open wrapper: runs the installed binary, never downloads, always exits 0) calling `anthology hook subagent-stop`; the decision is crates/anthology-runtime/src/subagent_stop.rs, tests in crates/anthology-runtime/tests/subagent_stop.rs, crates/repo-tests/tests/subagent_stop.rs and the shims entry.rs cases.
Adding a hook event to that hooks.json also breaks: crates/repo-tests/tests/cutover_hooks.rs (event list, per-event timeout, SHIMS list), crates/repo-tests/tests/shims/statics.rs (exact JSON literal, shebang list) and crates/anthology-runtime/tests/atl_install.rs (sorted event names, test_hooks_json_shape).
ci/jobs/shellcheck.sh lists every shim by name, so a new hooks/*.sh must be added there or CI never lints it.
Run Rust tests with scripts/rust-test.sh run cargo test --locked -p <crate> (Docker, pinned image); run it with --no-fail-fast, because cargo stops at the first failing test binary and hides the rest.
Why: each of these failed only after the hook change, in tests unrelated to it.
How to apply: grep crates for the existing event names before editing hooks.json. A new subcommand of the anthology binary reaches users only with the next anthology-bin release, so the wrapper must tolerate an older binary (it exits non-zero on the unknown subcommand and the wrapper prints nothing). The cutover_docs test scans every tracked file, memory notes included, for a banned word, so keep that word out of notes, comments and test names.
