---
name: anthology-machine-facts
description: "The user's Mac setup relevant to anthology — tools, BrowserOS, installed plugins, past Chrome incident"
metadata:
  node_type: memory
  type: project
  originSessionId: 1c9fb279-a45d-4d27-8e78-1ddd82c1e344
  modified: 2026-11-20T09:14:08.882Z
---

Recorded on 2026-11-20. Re-verify before relying on any of it.

- **gh:** logged in as `rbonweb` (scopes: repo, workflow, gist, read:org).
- **Claude Code:** 2.1.311.
- **Toolchain:**
  - Docker = OrbStack (`/usr/local/bin/docker`).
  - Node v26 via nvm.
  - `jq` is at `/usr/bin/jq`, `brew` is at `/opt/homebrew/bin/brew`.
  - No Rust/cargo, and it shouldn't be installed: Rust work runs in Docker or CI.
- **Missing config:** no `~/.claude/CLAUDE.md`, no `~/.config/git/ignore`, and `core.excludesFile` is unset.
- **BrowserOS Neo:** user-scope MCP in `~/.claude.json` at `http://127.0.0.1:9010/mcp`, but the app listens on `*:9010`. The user should restrict it to 127.0.0.1 (a manual step).
- **Browsers:** agent-browser isn't installed and must stay that way. No Playwright/Puppeteer/Chromium on the host. Google Chrome is installed.
- **Past incident:** an automation agent took over the user's real Chrome profile, which locked Chrome. The guardrails G2 and G3 exist because of this.
- **Installed plugins:** all `@anthropic-plugin-directory`, which is the claude.ai directory binding and separate from the `claude-plugins-official` marketplace.
  - **To remove after anthology works:** design, engineering, marketing, product-management, website-design.
  - **To replace with anthology's copies:** superpowers, design-skills.
  - **Kept:** feature-dev, claude-security, social-media-skills, motion-reel, watch-video.
  - **Leftover:** the `brand-voice` cache folder still needs deleting.
- **Unrelated:** `~/Desktop/blabla` has nothing to do with anthology.

**Why:** These facts shaped the plan's guardrails, the Docker-only rule and the cleanup steps.

**How to apply:** Use them for anthology Task 14 (install/cleanup) and for setup troubleshooting. Related: [[anthology-status]].
