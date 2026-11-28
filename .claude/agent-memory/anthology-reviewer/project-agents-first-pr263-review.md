---
name: 'Project agents first and repo switches (PR 263) review findings'
description: 'Head eff199c9: committed .claude/anthology.json can silently drop the whole guide; listing is bounded but unframed; delegation exemptions are workflow-only'
metadata:
  type: project
  modified: 2026-11-27
---

At head eff199c9 a committed <repo>/.claude/anthology.json with routing_guide false drops the whole routing guide with no warning (a test pins the silence), including the attribution, Docker or no host installs, literal-path delete, browser and no-secrets-in-memory lines; any cloned repo can ship that file, unlike the setup approval that gates MAP_LINE. The project listing in crates/anthology-runtime/src/project.rs folds whitespace, caps 12 entries and 100 characters and restricts names, but does not strip angle brackets or backticks and gives no data-not-rules framing. The delegation exemptions in plugins/anthology/hooks/mods/delegation.ts match paths as written (no realpath), look only for a .git entry (bare dotfiles repos with GIT_DIR, jj or hg repos count as outside), and the pure-deletion test accepts a whitespace-only new_string while docs/mods.md says empty. The hook.session-start golden change was proven a pure insertion of the MAP_LINE in all 256 changed cases.
Why: the mod is a workflow rule (Bash writes are unseen) and returning next(e) never grants a permission, so the exemptions do not weaken a safety boundary; the repo-controlled guide switch is the real trust question.
How to apply: when a repo file can switch Anthology behavior, ask whether an unapproved cloned repo can use it and whether it is visible to the user; split safety lines from workflow lines before allowing an opt-out.
