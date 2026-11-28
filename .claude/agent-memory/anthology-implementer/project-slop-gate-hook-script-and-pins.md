---
name: 'Fail-open PreToolUse shims: slop-gate.sh, commit-privacy.sh and where they are pinned'
description: 'The fail-open shim pattern (only binary exit 10 blocks, a switch set to 0 exits first), its tests, and the lists a new hook script must join'
metadata:
  type: project
  modified: 2026-11-28
---

plugins/anthology/hooks/slop-gate.sh is a fail-open PreToolUse shim: ANTHOLOGY_SLOP_GATE exactly 0 exits 0 at the top, before lib.sh (the Rust CLI and the after-work mod read the same switch; the shim needed its own check so the binary is never started). Then lib.sh is loaded in a subshell first (a broken lib.sh or missing file in a plain dot-source exits the shell with status 2, which would BLOCK), anth_resolve plus anth_installed (never installs), the binary run with stdout dropped and stderr kept after an x<code> sentinel, exit 2 only for binary exit 10. Tests: crates/repo-tests/tests/shims/slop_gate.rs (fake binary with a body per case; the Sandbox copies hooks/ from the repo at run time, so a new script needs no registration there; the switch test passes the env with sb.set_env and proves the binary did not start with a BS_RAN marker file); deleting the subshell guard turns a_library_or_lock_that_is_unusable_allows red.

plugins/anthology/hooks/commit-privacy.sh (the third Bash hook, after no-attribution.sh) has the same shape, with one difference: it reads stdin with the shell read builtin in a loop and looks at it with case for git then commit before loading lib.sh, so a Bash call that is no commit starts no process and never loads lib.sh. Its tests are crates/repo-tests/tests/shims/commit_privacy.rs, and shims/planted.rs runs it with a planted PATH (a non-commit input and a commit input).

A new hook entry joins the exact wants in shims/statics.rs and anthology-runtime/tests/atl_install.rs (test_hooks_json_shape), a planted-tool case in shims/planted.rs, a mod line in shims.rs, and the list in ci/jobs/shellcheck.sh. repo-tests/tests/cutover_hooks.rs checks every hook generically (script ships, timeout 60, onFailure block) and needed no edit. cargo test takes one name filter, so run a whole test binary (--test shims). The shellcheck job needs docker on the machine that runs it; the remote build host script only runs rust-test.sh, so shellcheck is left to CI. The shellcheck job pulls its pinned image on first use; remove it afterwards if you pulled it.
