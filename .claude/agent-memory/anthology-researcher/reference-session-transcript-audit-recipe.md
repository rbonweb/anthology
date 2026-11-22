---
name: 'How to audit a Claude Code session transcript'
description: 'Where the useful signals live in a session JSONL and its subagents folder'
metadata:
  type: reference
  modified: 2026-11-22
---

Copy the JSONL and its sibling session folder to /tmp first; live sessions keep growing.
Signals worth extracting with python3 -I:
- type=user with origin.kind=human: typed messages. Messages typed while the lead is mid-turn arrive as attachment type queued_command, not as user lines; most corrections hide there.
- attachment agent_ask_notice: a subagent waiting on a permission prompt (gives the tool_use id to look up in the subagent file).
- attachment hook_success / hook_system_message / hook_additional_context: which plugin version ran (scripts/session-start.sh was 0.1.x, hooks/session-start.sh is 0.2.x) and what warnings showed.
- subagents/*.meta.json: agentType, model actually used, worktreePath, description. Assistant message.model inside each subagent file confirms the real model.
- tool_result is_error text groups the friction (worktree isolation refusals, Blocked sleep, Monitor InputValidationError, built-in rm safety check, anthology-mem refusals).
- Duration between tool_use and tool_result timestamps finds foreground waits and likely permission prompts.
Why: the first audit (2026-11-22) lost time finding that queued_command and meta.json carry the key data.
Keep audit notes generic: a transcript can belong to another project, so record kinds of problems, counts and fixes, never names, paths, quotes or IDs.
