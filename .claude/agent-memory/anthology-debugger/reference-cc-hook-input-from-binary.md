---
name: 'Claude Code hook input fields can be read from the installed binary'
description: 'How to check which fields a hook input carries (agent_id on a subagent compact) from the local claude binary, and what 2.1.323 showed'
metadata:
  type: reference
  modified: 2026-11-22
---

The installed claude binary is a native bundle whose JS is plain text, so grep -a -b -o with a short pattern finds the code, then dd with bs=1 skip=<offset> count=<n> prints it. A leading .{500} in the pattern makes grep run for minutes; use the byte offset instead. In 2.1.323 the compaction code calls the SessionStart runner with agentId: H.agentId, and the base hook input sets agent_id from the agent context, so a subagent compact carries agent_id and the plugin check for a non-empty agent_id (is_subagent in crates/anthology-runtime/src/session_start.rs) is right for that version. Why: a field report of a subagent compact taking the full lead path (on 2.1.330) could not be tied to is_subagent; the likeliest reading is that the input lacked agent_id, which was not checked on that version. How to apply: before changing is_subagent, grep the newer binary for the compact call and the base input builder; agent_type alone is not a subagent signal because a main session started with --agent also has it.
