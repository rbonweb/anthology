---
name: 'perm classify: where the path and git check live and how they are tested'
description: 'anthology perm classify sits in anthology-cli (cmd/perm.rs, cmd/perm/{rules,paths,git}.rs); git traps are proven with real git; rulings on allowlist and env'
metadata:
  type: project
  modified: 2026-11-22
---

anthology perm classify (P4, ADR-033) lives in crates/anthology-cli/src/cmd/perm.rs with rules.rs (Read() matcher, anchors read wide), paths.rs (readonly_paths: resolve, containment, globs, walk with a 20000-entry budget) and git.rs (asks git itself: rev-parse and config --list --includes -z run side by side; key allowlist safe_key; hooks, alternates, submodule, index checks). It is not in anthology-guard because another PR held that crate at the time.
Tests: crates/anthology-cli/tests/perm.rs runs the binary with env_clear, HOME in a temp dir and GIT_CONFIG_NOSYSTEM=1, and every git trap test first asserts a marker program ran with the real git, then asserts no decision. The container needs git (and procps for anthology-runtime bootstrap tests, which call ps). On macOS the same tests pass with Apple git; core.pager is shown with BSD script -q /dev/null git log.
Rulings worth knowing: allowlist, not denylist; every GIT_* env var refuses except GIT_CONFIG_NOSYSTEM, GIT_TERMINAL_PROMPT, GIT_OPTIONAL_LOCKS; PATH with a relative or empty entry refuses; any hook file but *.sample refuses (git status writes the index and runs post-index-change); a gitlink in the index refuses even without .gitmodules; the stderr-non-empty check in run_git is not pinned by a test (no case found where rev-parse passes and config warns).
Why: the security review and P5 will extend this, and a mutant that survives here is a silent allow.
