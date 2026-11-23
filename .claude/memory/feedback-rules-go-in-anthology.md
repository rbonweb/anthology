---
name: rules-go-in-anthology
description: "Every working rule goes into the Anthology plugin, never into the owner's global ~/.claude config (CLAUDE.md, skills)"
metadata:
  node_type: memory
  type: feedback
  originSessionId: 1c9fb279-a45d-4d27-8e78-1ddd82c1e344
  modified: 2026-11-21T14:26:35.865Z
---

When the owner gives a working rule, put it in the Anthology plugin: the routing guide, agent prompts or skills. Then reinstall the owner's plugin. Never add it to ~/.claude/CLAUDE.md, ~/.claude/skills or any other per-machine config.

**Why:** owner, 2026-11-21: "that is the reason we are making the Anthology, so we do not have to add manual rules on the global Claude configuration or per system level." The rules also have to reach the owner's other projects, which run Anthology.

**How to apply:** for a new rule, start an Anthology PR into build, merge it, then reinstall. The earlier local copies (~/.claude/CLAUDE.md, ~/.claude/skills/large-builds, karpathy-guidelines) were moved into Anthology (#117: the anthology:large-builds skill, karpathy as agent knowledge) and removed locally on 2026-11-21; a backup is in ~/claude-local-rules-backup-2026-11-21/. This supersedes the parts of [[owner-build-rules-2026-11-21]] that say "saved to ~/.claude/CLAUDE.md".
