---
name: 'anthology-superpowers cannot be disabled alone; manifest has no optional dependency'
description: 'Claude Code refuses to disable a plugin an enabled one requires; plugin.json dependencies take only name, version, marketplace'
metadata:
  type: project
  modified: 2026-11-27
---

Checked on claude 2.1.326 with a throwaway CLAUDE_CONFIG_DIR and the repo as a local marketplace: claude plugin disable anthology-superpowers@anthology fails with "still required by anthology". The docs (plugin dependencies page) list only name, version and marketplace for a dependency entry: no optional or recommended flag. So the duplicate-superpowers session-start line names the official superpowers to disable, via doctor::enabled_official_superpowers. Why it matters: any future warning or doc that tells a user to disable a bundled dependency alone gives a command that fails. Dropping the dependency would stop the bundle pulling anthology-superpowers in: a product decision, not made here.
