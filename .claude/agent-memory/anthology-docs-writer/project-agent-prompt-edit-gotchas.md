---
name: 'Agent prompt and guide edit gotchas'
description: 'Roster bans the word credit; body cap 60 lines; the CHANGELOG is the root file; guide sat at exactly 3800 characters'
metadata:
  type: project
  modified: 2026-11-26
---

Adding a rule to plugins/anthology/agents/*.md: roster.rs test_no_forbidden_text rejects the word credit or credits in any agent file (use attribution). A body is capped at 60 lines, and ui-designer sits at the cap, so append to an existing bullet there instead of adding a line. Debugger and scout have almost no startup-budget headroom (check with scripts/dev.sh startup-budget before wording). The changelog is the root CHANGELOG.md under ## anthology, ### [Unreleased]; plugins/anthology has no CHANGELOG.md. The routing guide was at exactly 3800 characters after the licensing line; the next guide edit must trim first. Why: each of these failed or nearly failed a test late.
