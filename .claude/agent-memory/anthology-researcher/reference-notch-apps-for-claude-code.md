---
name: 'Notch apps that answer Claude Code prompts (survey 2026-11-26)'
description: 'Which open-source macOS notch apps show sessions and answer approvals and AskUserQuestion, and the hook contract they use'
metadata:
  type: reference
  modified: 2026-11-26
---

Survey of 2026-11-26 (stars and dates from the GitHub API that day).
Leaders: Louis-CFM/coucou (MIT, about 7.5k stars, signed brew cask, macOS 15+), wxtsky/CodeIsland (MIT, about 2.6k, brew cask, macOS 14+), farouqaldori/vibe-notch, formerly claude-island (Apache-2.0, about 2.5k, Mixpanel telemetry, approvals only, quiet since 2026-04), sk-ruban/notchi (GPL-3.0, about 1k, shows state only), jazzenchen/agent-hud-open (Apache-2.0, about 840). Generic notch shells: TheBoredTeam/boring.notch and Ebullioscopic/Atoll, both GPL-3.0; they draw a borderless nonactivating NSPanel.
Contract they all use: hooks merged into the user settings file, a hook script that relays to a per-user Unix socket, and a block until the click. Coucou registers PermissionRequest (timeout 120 s, answers with decision behavior allow, deny or updatedPermissions) and a PreToolUse hook on matcher AskUserQuestion (timeout 130 s, needs Claude Code 2.1.85+), which returns permissionDecision allow plus updatedInput with questions and answers. Its socket is chmod 0600 with a getpeereid uid check, and the hook fails open: if the app does not answer within 300 ms, the normal prompt shows.
Mod API (2.1.332 types): tool.check can return a decision, and $.http.fetch takes a socketPath, so a mod could also relay. The hooks path is what existing apps expect.
Why: owner asked whether Anthology should build a notch UI; the recommendation was to integrate with an existing app, not build one. How to apply: re-check stars, licences and hook timeouts before any integration work.
