---
name: 'How to audit a Claude Code session transcript'
description: 'Where the useful signals live in a session JSONL and its subagents folder'
metadata:
  type: reference
  modified: 2026-11-23
---

Copy the JSONL and its sibling session folder to /tmp first; live sessions keep growing (when the brief forbids writes, read in place).
Signals worth extracting with python3 -I:
- type=user with origin.kind=human: typed messages. Messages typed while the lead is mid-turn arrive as attachment type queued_command, not as user lines; most corrections hide there.
- attachment agent_ask_notice: a subagent waiting on a permission prompt (gives the tool_use id to look up in the subagent file).
- attachment hook_success / hook_system_message / hook_additional_context: which plugin version ran (guard.sh path in a hook error names the cache version), durationMs per hook, and what warnings showed.
- entrypoint and version on every line: sdk-cli or a host name means an app drove the session; cli or claude-vscode means a person.
- api-request lines reference api-request-blob hashes: that shows what was really sent, e.g. a hook context sent twice after resume.
- cost-state and assistant usage: cache_creation vs cache_read per call, for context-cost estimates.
- subagents/*.meta.json: agentType, model actually used, worktreePath, description. Assistant message.model inside each subagent file confirms the real model.
- tool_result is_error text groups the friction (guard G0 blocks, worktree isolation refusals, Blocked sleep, Monitor InputValidationError, built-in rm safety check, anthology-mem refusals). Pair each block with the next two tool calls to see the workaround.
- Duration between tool_use and tool_result timestamps finds foreground waits and likely permission prompts.
Why: the first audit (2026-11-22) lost time finding that queued_command and meta.json carry the key data; the 2026-11-23 audit needed entrypoint and blob references to judge host sessions.
Keep audit notes generic: a transcript can belong to another project, so record kinds of problems, counts and fixes, never names, paths, quotes or IDs.
