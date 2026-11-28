---
name: 'Guard and the credentials folder under .config'
description: 'What the guard allows for the credentials path (Write, mkdir, chmod, read back) and the one collision: project names starting chromium or google-chrome'
metadata:
  type: project
  modified: 2026-11-28
---

Checked 2026-11-28 with anthology guard hook (stdin JSON): Write of a KEY=value file, mkdir -p, chmod 700 and 600, a redirect write of a key line, cat and Read of the file under a .config project folder are all allowed. G6 (secret patterns) blocks only writes into a memory path (.claude/memory, .claude/agent-memory), so a key line in an env file elsewhere passes. G1, G2, G4 and G5 do not look at these commands.
The one block is G3: PROFILE_ROOTS (crates/anthology-guard/src/data.rs) match a path component that starts with chromium or google-chrome under a .config folder, so a project whose lowercase name starts with chromium or google-chrome gets Chrome profiles are off-limits for every command and tool on its credentials folder. Nothing in the rule text can avoid it; route it to the guard owner rather than editing the guard.
Not a guard block: the read-only proof (anthology perm classify, crates/anthology-cli/src/cmd/perm/paths.rs) needs a path under a root, so a cat of a file in the home .config folder is not auto-allowed and Claude Code asks as usual. The secret-name list in crates/anthology-shell/src/readonly.rs matches a component named exactly .env, so a file called service.env is not on it. The slop gate only checks md, html, tsx, jsx and css files.
Why: the owner wants agents to write and read these files without a stop; this records where a stop can and cannot come from.
