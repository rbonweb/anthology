---
name: 'Facts that shaped the 2026-11-23 audit fix plan'
description: 'ATL rg PATH check exists; mem save is agent-scope only; guide goldens use a fixture; MEMORY.md must stay tracked; anthology-shell change reaches ATL'
metadata:
  type: project
  modified: 2026-11-24
---

The ATL rewrite of rg, grep, ag, find and fd is already dropped when the program is not on PATH (PR 167, plugins/anthology/bin/atl-hook, tests in crates/anthology-runtime/tests/atl_install.rs); project audits on old plugin versions still report it.
anthology-mem save writes only agent notes (.claude/agent-memory/anthology-NAME/); there is no repo or lead scope yet, so a rule telling the lead to use it needs a new flag first.
The hook.session-start golden uses a fixture GUIDE constant (crates/difftest/src/surfaces/hook_session_start.rs), so editing templates/routing-guide.md needs no golden bless; only the 3800-char budget test applies.
Agents run in worktrees made from HEAD and Claude Code loads their MEMORY.md from there, so untracking the generated MEMORY.md indexes would leave every isolated agent without its index.
crates/anthology-shell is built by ATL too, so a shell change must keep the default analysis identical (new behaviour behind a parameter only the guard passes) or it changes the ATL binary.
version_bump.rs compares with the merge base; release.sh check-version already accepts a plugin.json that already holds the target version, so a gate against the last released tag fits release.sh unchanged.
Why: each of these changed a task in the fix plan and none is obvious from one file.
How to apply: check them again before planning guard, memory, release or guide work.
