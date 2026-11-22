---
name: 'What the PreToolUse hook input can and cannot carry'
description: 'Real key order and model-controlled parts of the Bash hook input, for judging grep-based hook checks'
metadata:
  type: reference
  modified: 2026-11-22
---

In Claude Code 2.1.323 the PreToolUse input is built as {session_id, transcript_path, cwd, scratchpad_dir, prompt_id, permission_mode, agent_id, agent_type, effort, hook_event_name, tool_name, tool_input, tool_use_id} (JSON.stringify, compact), so permission_mode always sits before tool_input and only string values under tool_input are model text. JSON escaping means a bare quote never appears inside any string value (cwd included), so a pattern with a bare quote after a key name cannot be forged from command text. Reading the mode from the part before the first "tool_input" is therefore sound and fails safe (ask kept) if a host orders keys differently. Extra keys inside tool_input are also rejected by the Bash schema (never catchall). Hook scripts must pin LC_ALL=C on every tool in a pipeline, tr included: macOS BSD tr aborts with Illegal byte sequence on invalid UTF-8, which would make an extraction pipeline empty and a hook fail open (Claude Code cannot emit invalid UTF-8, so this is theoretical).
Why: confirmed from the 2.1.323 binary and by probing plugins/anthology/bin/atl-hook and plugins/anthology/hooks/no-attribution.sh on macOS sh with BSD grep, Debian dash and Alpine busybox (review 022 fix round).
