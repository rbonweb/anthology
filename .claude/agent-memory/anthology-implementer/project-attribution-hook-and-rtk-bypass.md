---
name: 'Attribution hook and rtk bypass-mode drop'
description: 'no-attribution.sh sits beside rtk-hook in one PreToolUse group; its pins, review rulings, and phrases the hook itself denies'
metadata:
  type: project
  modified: 2026-11-22
---

plugins/anthology/hooks/no-attribution.sh is the second hook inside the existing PreToolUse Bash group (not a new group), so cutover_hooks (timeout 60, onFailure block) keeps passing and the matcher set is unchanged. Pins to update when adding another hook there: crates/repo-tests/tests/shims/statics.rs and crates/anthology-runtime/tests/rtk_install.rs (test_hooks_json_shape), both exact JSON literals. Tests for the hook: crates/repo-tests/tests/no_attribution.rs. The rtk-hook bypass tests are in crates/anthology-runtime/tests/rtk_install.rs (wrapper_test).
Security review rulings: rtk-hook drops the ask only for permission_mode bypassPermissions (not auto: a dropped ask on an rtk line lets Bash(rtk *) skip auto mode classifier), and reads the mode only before the first "tool_input" so a nested key cannot forge it. no-attribution greps only the extracted "command" value, never the description. Its gaps (--trailer, -F, --body-file) are documented, not bugs.
The hook denies any git commit text that holds the assistant co-author trailer, so a commit message that merely describes the rule is denied too: word those messages as "attribution trailers".
Run just the changed tests with scripts/rust-test.sh run cargo test --locked --no-fail-fast -p anthology-runtime -p repo-tests --test <name> ...; cargo stops at the first failing test binary unless --no-fail-fast is set. The golden replay (cargo test -p difftest --test golden) takes several minutes and covers dev.startup-budget, which must be re-blessed after any agent prompt edit.
Why: each of these cost a round trip while building the 0.2.2 fix.
