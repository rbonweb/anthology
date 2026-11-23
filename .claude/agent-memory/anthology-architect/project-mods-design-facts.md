---
name: 'Claude Code mods: facts that shaped the post-release design'
description: 'tool.check runs after PreToolUse and Bash(atl *) is an allow rule; guide is near 3800 chars; hooks.json is pinned in three tests'
metadata:
  type: project
  modified: 2026-11-22
---

Post-release mods design: .superpowers/sdd/2026-11-20-rust-port/post-release-mods-design.md.
tool.check fires after tool.call and PreToolUse, so the guard has already decided; a permission upgrade must only touch an ask with no rule, setting, hook, plugin or ceiling.
The plugin installs Bash(atl *) as an allow rule, so lines atl rewrites are already allowed; the read-only gap is lines atl leaves as typed.
The routing guide sits at about 3795 of its 3800-character budget (crates/anthology-runtime/tests/session_start.rs); any added line needs an equal trim.
hooks.json shape is pinned in crates/repo-tests/tests/shims/statics.rs, cutover_hooks.rs and crates/anthology-runtime/tests/atl_install.rs.
On the Rust line, read files with git show origin/notify-alert-rust:path (origin/build is still the old line).
Why: each of these changed a design decision and is not obvious from one file.
How to apply: check them again before planning hooks, guide or permission work.
