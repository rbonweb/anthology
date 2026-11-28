---
name: 'How to audit a Claude Code session transcript'
description: 'Where the useful signals live in a session JSONL and its subagents folder'
metadata:
  type: reference
  modified: 2026-11-28
---

Copy the JSONL and its sibling session folder to /tmp first; live sessions keep growing (when the brief forbids writes, read in place).
Signals worth extracting with python3 -I or jq:
- type=user with origin.kind=human: typed messages. Messages typed while the lead is mid-turn arrive as attachment type queued_command, not as user lines; most corrections hide there. Their attachment.prompt is a string, or an array when the owner pasted an image: take only the text blocks, or the base64 image swamps the output and the text is lost.
- Drop human-origin lines whose text starts with task-notification, agent-message or command- tags: those are harness events, not owner words.
- attachment agent_ask_notice: a subagent waiting on a permission prompt (gives the tool_use id to look up in the subagent file).
- attachment hook_success / hook_system_message / hook_additional_context: which plugin version ran (guard.sh path in a hook error names the cache version), durationMs per hook, and what warnings showed. Counting anthology/anthology/<version> strings in the file gives the versions used.
- entrypoint and version on every line: sdk-cli or a host name means an app drove the session; cli or claude-vscode means a person.
- api-request lines reference api-request-blob hashes: that shows what was really sent, e.g. a hook context sent twice after resume.
- cost-state and assistant usage: cache_creation vs cache_read per call, for context-cost estimates.
- subagents/*.meta.json: agentType, model actually used, worktreePath, description. Assistant message.model inside each subagent file confirms the real model. Line 1 of a subagent file is its brief; a last user line of Request interrupted by user means it was stopped.
- To prove a checker or gate ran, list the tool_use Bash commands in the subagent file; grepping for GATE lines only finds the skill text the agent read.
- tool_result is_error text groups the friction (guard G0 blocks, worktree isolation refusals, Blocked sleep, Monitor InputValidationError, built-in rm safety check, anthology-mem refusals). Pair each block with the next two tool calls to see the workaround.
- Duration between tool_use and tool_result timestamps finds foreground waits and likely permission prompts.
- ugrep refuses long context regexes such as .{0,200} around a match; use jq select(tostring|test(...)) with input_line_number instead.
Why: the first audit (2026-11-22) lost time finding that queued_command and meta.json carry the key data; the 2026-11-23 audit needed entrypoint and blob references to judge host sessions; the 2026-11-28 design audit missed image-carrying owner messages until array prompts were handled.
Keep audit notes generic: a transcript can belong to another project, so record kinds of problems, counts and fixes, never names, paths, quotes or IDs.
