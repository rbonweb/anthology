---
name: 'autoMemoryEnabled false also disables subagent memory'
description: 'Claude Code docs: turning auto memory off makes the agent memory field a no-op'
metadata:
  type: reference
  modified: 2026-11-22
---

Claude Code sub-agents docs (Enable persistent memory): subagent memory is part of auto memory; with autoMemoryEnabled false or CLAUDE_CODE_DISABLE_AUTO_MEMORY the memory field has no effect, so no MEMORY.md preload and no memory instructions.
Why: every roster agent in plugins/anthology/agents sets memory: project, so any change that writes autoMemoryEnabled false (PR 150) silently stops agent memory loading.
How to apply: when reviewing memory or setup changes, check agent-memory loading, README agent-memory paragraph and the doctor assumption, not only the main session.
