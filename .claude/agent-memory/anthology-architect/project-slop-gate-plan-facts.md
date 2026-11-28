---
name: 'Facts that shaped the 2026-11-28 slop gate plan'
description: 'Hook pins, worktree paths under .claude, corpus is clean today, vendor target limits, startup budget headroom'
metadata:
  type: project
  modified: 2026-11-28
---

Why: these facts decided the anti-slop gate design (docs/superpowers/plans/2026-11-28-anti-slop-gate.md) and will matter again for any new hook or vendored knowledge.
- crates/repo-tests/tests/cutover_hooks.rs pins every hook to timeout 60, every PreToolUse hook to onFailure block, and the anthology events to exactly SessionStart and PreToolUse. A PostToolUse or Stop hook breaks those tests; a fail-open hook must fail open inside its own script.
- shims/statics.rs and anthology-runtime/tests/atl_install.rs pin the whole PreToolUse array of plugins/anthology/hooks/hooks.json.
- Claude Code worktrees live under .claude/worktrees/, so any path exemption on a .claude component must be taken relative to the repo root found by walking up to .git, or every worktree agent is exempt.
- On 2026-11-28 README.md, docs/**, CONTRIBUTING, SECURITY, DECISIONS and CHANGELOG had zero hits for the v1 slop word, phrase and contrast lists; em dashes there are mostly table cells.
- Vendor targets must be plugins/anthology-superpowers or plugins/anthology/knowledge/<name> (vendor/manifest.rs safe_target); every knowledge folder must be read in some agent Startup (roster.rs test_no_loose_knowledge), so knowledge plus prompts land in one task.
- funboy322/avoid-ai-design is 3.4 MB of mostly PNGs; vendor only its references/ path.
- docs-writer startup budget had about 70 bytes of headroom; blader/humanizer SKILL.md is 33.6 KB, so adding it needs a stated BUDGET raise in startup_budget.rs and roster.rs (difftest dev_upstream_watch.rs keeps its own pinned copy).
- impeccable detect is now a Rust workspace whose render checks use CDP (Chrome), which the browser rule forbids.
