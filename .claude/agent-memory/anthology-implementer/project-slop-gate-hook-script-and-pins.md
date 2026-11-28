---
name: 'Fail-open PreToolUse shim: slop-gate.sh and where it is pinned'
description: 'slop-gate.sh pattern (only binary exit 10 blocks, ANTHOLOGY_SLOP_GATE=0 exits first), its tests, and the lists a new hook script must join'
metadata:
  type: project
  modified: 2026-11-28
---

plugins/anthology/hooks/slop-gate.sh is a fail-open PreToolUse shim: ANTHOLOGY_SLOP_GATE exactly 0 exits 0 at the top, before lib.sh (the Rust CLI and the after-work mod read the same switch; the shim needed its own check so the binary is never started). Then lib.sh is loaded in a subshell first (a broken lib.sh or missing file in a plain dot-source exits the shell with status 2, which would BLOCK), anth_resolve plus anth_installed (never installs), the binary run with stdout dropped and stderr kept after an x<code> sentinel, exit 2 only for binary exit 10. Tests: crates/repo-tests/tests/shims/slop_gate.rs (fake binary with a body per case; the Sandbox copies hooks/ from the repo at run time, so a new script needs no registration there; the switch test passes the env with sb.set_env and proves the binary did not start with a BS_RAN marker file); deleting the subshell guard turns a_library_or_lock_that_is_unusable_allows red. A new hook entry joins the exact wants in shims/statics.rs and anthology-runtime/tests/atl_install.rs (test_hooks_json_shape), a planted-tool case in shims/planted.rs, and the list in ci/jobs/shellcheck.sh. cargo test takes one name filter, so run a whole test binary (--test shims). The shellcheck job pulls its pinned image on first use; remove it afterwards if you pulled it.
