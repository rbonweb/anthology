---
name: 'Host signals and subagent waiting (spike 2026-11-24)'
description: 'Measured entrypoint, attended flag, Bash env, mod surfaces, version and background-notice behavior on Claude Code 2.1.323 and 2.1.330'
metadata:
  type: project
  modified: 2026-11-24
---

Measured 2026-11-24 in Docker with a mock Anthropic API (harness: reference-mock-api-hook-testing-harness.md), Claude Code 2.1.323 and 2.1.330 (CI pin, ci/claude-code/package.json), TS Agent SDK 0.3.323 and 0.3.330, hooks via --plugin-dir.
Entrypoint: CLAUDE_CODE_ENTRYPOINT is in every hook's env (SessionStart, PreToolUse, PostToolUse, Stop, SubagentStop, also in subagents) and in the Bash tool's env, same value: cli (interactive), sdk-cli (claude -p), sdk-ts (SDK query). A preset custom value (for example hostapp) is kept as is, but a preset cli is replaced by sdk-cli under -p. CLAUDE_CODE_SESSION_ATTENDED is 1 interactive and 0 for -p and SDK, recomputed even when preset; it is the more robust headless signal. The SDK also sets CLAUDE_AGENT_SDK_VERSION.
Hook input: the only mode difference is that SessionStart carries model in interactive runs and not under -p or SDK. Stop and SubagentStop carry background_tasks (id, type shell or subagent, status, command).
Bash tool env: same values as hooks for inherited vars. A SessionStart hook writing exports to CLAUDE_ENV_FILE, or exports in the shell rc files, change the value the Bash tool sees (CLAUDE_CODE_ENTRYPOINT included) but never what hooks see. Bash also gets CLAUDE_CODE_EXECPATH and AI_AGENT ending _agent where hooks get _harness.
Mods: $.session.surfaces() is [] under -p and SDK, ["terminal"] interactive; session.start e.isInteractive matches. globalThis.process is undefined in a mod. $.env.get('CLAUDE_CODE_ENTRYPOINT') returns the entrypoint. $.session.info() exists on 2.1.330 only and has no entrypoint field. $.session.version() works on both.
Version: hooks get no version field on stdin; AI_AGENT=claude-code_2-1-330_harness carries it in the env, but a preset AI_AGENT is kept, so it can be wrong in nested runs. Mods use $.session.version().
Waiting: Agent calls run async. A subagent that ends its turn with a run_in_background shell or an armed Monitor sends the lead a task-notification "Agent X is waiting on its own background work" marked interim, then "finished" once its children end. Every Monitor event wakes the subagent and sends another interim notice (3 events gave 4 interim and 1 final). Monitor returns "Monitor started" at once, until-loop or not, so it never keeps a turn open. A subagent does not resume while the lead runs a foreground tool. agent.report (2.1.330 only; on 2.1.323 the whole module is refused) can return consumed and the lead then sees nothing, but its input is identical for interim and final reports, so a blanket consume also drops the final one. A SubagentStop hook returning decision block gives the subagent another turn with the reason.
Why: the host-detection and waiting-rule fixes of the 2026-11-23 audit depend on these signals. How to apply: re-measure when the CI pin moves.
