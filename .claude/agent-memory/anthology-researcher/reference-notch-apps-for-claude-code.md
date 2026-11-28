---
name: 'Notch and island apps that answer Claude Code prompts (survey 2026-11-26)'
description: 'Which open-source macOS, Windows and Linux apps show sessions and answer approvals and AskUserQuestion, and the hook contract they use'
metadata:
  type: reference
  modified: 2026-11-26
---

Survey of 2026-11-26 (stars and dates from the GitHub API that day).
macOS leaders: Louis-CFM/coucou (MIT, about 7.5k stars, signed brew cask, macOS 15+), wxtsky/CodeIsland (MIT, about 2.6k, brew cask, macOS 14+), farouqaldori/vibe-notch, formerly claude-island (Apache-2.0, about 2.5k, Mixpanel telemetry, approvals only, quiet since 2026-04), sk-ruban/notchi (GPL-3.0, about 1k, shows state only), jazzenchen/agent-hud-open (Apache-2.0, about 840). Generic notch shells: TheBoredTeam/boring.notch and Ebullioscopic/Atoll, both GPL-3.0; they draw a borderless nonactivating NSPanel.
Windows and Linux: Coucou has a Tauri 2 and Rust build in its windows folder. The rolling release windows-latest (2026-10-07) is an unsigned msi or setup.exe. It shows a top-center island, Allow or Deny and question options, and talks over the named pipe of the user SID with a same-SID check. Linux uses an AppImage, deb or rpm, a gtk-layer-shell island and a socket under XDG_RUNTIME_DIR. From WSL, the hook command in the distro settings calls the Windows coucou-hook.exe through /mnt/c, with WSLENV carrying the variables; the installer does not write WSL hooks. CodeIsland has a WPF and .NET 8 Windows port (an independent port kept in its repository, with a user-scoped named pipe and no release asset upstream) and no Linux build. Agent HUD ships unsigned Windows and Linux builds, but approvals in them are unconfirmed. vibe-notch is macOS only.
Contract they all use: hooks merged into the user settings file, a hook script that relays to a per-user socket or pipe, and a block until the click. Coucou registers PermissionRequest (timeout 120 s, answers with decision behavior allow, deny or updatedPermissions) and a PreToolUse hook on matcher AskUserQuestion (timeout 130 s, needs Claude Code 2.1.85+), which returns permissionDecision allow plus updatedInput with questions and answers. The hook fails open: if the app does not answer within 300 ms, the normal prompt shows.
Mod API (2.1.332 types): tool.check can return a decision, and $.http.fetch takes a socketPath, so a mod could also relay. The hooks path is what existing apps expect.
Why: owner asked whether Anthology should build a notch or island UI; the recommendation was to integrate with Coucou on every platform, not build one. How to apply: re-check releases, signing and hook timeouts before any integration work.
