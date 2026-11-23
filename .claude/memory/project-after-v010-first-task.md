---
name: after-v010-first-task
description: "Post-release queue in order: rtk cd/head gate plus guard permission levels (one parser, one security review), then project map plus after-work check"
metadata:
  node_type: memory
  type: project
  originSessionId: 1c9fb279-a45d-4d27-8e78-1ddd82c1e344
  modified: 2026-11-22T03:44:50.765Z
---

Start these right after v0.1.0 ships, in this order. Items 1 and 2 can run in parallel because their files don't overlap.

1. **rtk gate and permission levels.** Both decide which commands are safe on the shared shell parser, and they get one security review together.
   - **rtk gate** (owner, 2026-11-21): the gate in `rtk/src/hooks/anthology_gate.rs` accepts `cd <literal dir> && …` and `| head/tail -n N`, which adds about 6 more rewrites in a 32-command mix. It needs a fork change, an rtk `1.2.1-anthology.5` release and an rtk.lock bump; the checklist is in PLAN.md §15 (PR #120). Also rebuild rtk on the fixed anthology-shell, because rtk/Cargo.lock still pins 0.1.0, which predates the #133 lexer fix.
   - **Permission levels** (owner, 2026-11-22, approved "right after release"): the guard auto-approves commands it can prove are read-only (ls, cat, grep, git status/log/diff, find without -delete/-exec, and so on), so Claude Code stops prompting for them in auto and bypass mode. Everything else gets Claude Code's normal handling, and destructive commands stay blocked or asked. The owner's words: "only the destructive ones should ask… even in bypass it should not ask for anything except highly destructive."
2. **Project map and after-work check** (owner, 2026-11-22, approved "after release").
   - The architect builds one committed project map: every piece and what connects to what. Every writing agent updates it in the same commit.
   - The reviewer checks each agent's work against the map before merge: connected pieces updated, no leftovers (temp files, dead code, stale docs, containers), cleanup done.
   - No new agent type. The short design is in `.superpowers/sdd/2026-11-20-rust-port/project-map-design.md`; commit it as a spec when the work starts.

3. **Build these as Claude Code mods** (owner, 2026-11-22, "Yes, after release"). Mods are TypeScript hooks modules shipped inside Anthology's plugins through `hooks/hooks.json` `"modules"`; the shell hooks stay as the fallback. The events that matter, from the plugin-authoring skill's types:
   - `tool.check` (allow/ask/deny per call) for the permission levels;
   - `prompt.compose`, plus refusing main-loop Edit/Write in `tool.call` (no `agentId`), for strict delegation;
   - `agent.report` (sees each agent's report before the lead) for the automatic after-work check;
   - `vcs.isolate` to put agent worktrees outside `.claude/`, if `.claude` writes are confirmed as the prompt source;
   - `$.audio.play` and `$.ui.toast` for alerts, and a pane for the live status.
   The permission part gets one security review. Never write under ~/.claude/dev-mods while only researching: writing there turns mods on.

4. **Fewer prompts, small items:** add "delete only literal paths, never `rm -rf $VAR`" to the agent rules and the routing guide; in bypass mode Claude Code still asks for possible critical-path removals, and a variable path counts. Ship an output style with `force-for-plugin: true` that makes the lead a coordinator, as part of strict delegation.

**Why:** the owner wanted risky gate code kept out of the launch, saw too many permission prompts, and saw agents miss connected updates and cleanups.
**How to apply:** once v0.1.0 ships, start item 1 before any other post-release work. Related: [[anthology-status]], [[rules-go-in-anthology]].
