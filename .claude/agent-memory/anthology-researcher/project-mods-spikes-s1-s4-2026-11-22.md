---
name: 'Mods spikes S1-S4 results (2026-11-22)'
description: 'Measured Claude Code 2.1.311-2.1.330 behavior for hooks modules: loading, tool.check input, prompt.compose, spawn latency'
metadata:
  type: project
  modified: 2026-11-22
---

Results of the four spikes in .superpowers/sdd/2026-11-20-rust-port/post-release-mods-design.md section 9, run in a Docker container with a mock Anthropic API (no real key).
S1: hooks.json "modules" loads on 2.1.311, 2.1.323, 2.1.329 and 2.1.330, both via --plugin-dir and via marketplace install; shell hooks in the same hooks.json keep running. No warning, no error. "modules" takes ONE entry per plugin: a second entry fails the whole hooks.json load, shell hooks included. A module that hooks an event the build lacks is refused whole (only that module; shell hooks still run, only the debug log says so): agent.report is missing on 2.1.311 and 2.1.323, present on 2.1.329. $.session.machine is missing before 2.1.329. tool.check, tool.call, prompt.compose, agent.spawn, turn.start, session.start, session.end, ui.render, command.run load on 2.1.311. engines in plugin.json: ignored on 2.1.311; on 2.1.323 and 2.1.329 a git or url source is refused at install ("needs Claude Code >=2.1.330") while a local-folder source installs with a note.
S2: tool.check e.input is the REWRITTEN command after a PreToolUse updatedInput (rtk-style); tool.call in a mod sees the TYPED command. Same on 2.1.311-2.1.330. Verdict shape: allow with hook PreToolUse when the hook also allowed, allow with rule Bash(rtk *) when no hook decision, ask with hook PreToolUse when the hook asked. The default outside-working-dir read ask has NO rule/setting/hook marker (setting appears only when permissions.blockReadsOutsideWorkingDirectories is on).
S3: prompt.compose does NOT fire for subagent system prompts (general-purpose, a plugin agent with tools Read and Grep, Explore) on 2.1.311, 2.1.329 and 2.1.330 in headless -p. It fired only for the main loop (traits print+skills, 26 tools). A fork subagent reuses the parent composed prompt, marker section included. The general-purpose subagent request DID list the Agent tool, so a tools-based Agent check would be wrong if compose ever fired there. Not tested in the interactive TUI.
S4: $.process.run of a native Rust binary from tool.check, 200 runs each, Linux arm64 Docker VM: direct p50 1.18 ms p95 2.40 ms; via sh shim exec p50 1.35 p95 3.49; /bin/true p50 1.12 p95 2.26; max 20 ms. Whole tool.check hook (two spawns) p50 2.6 ms p95 6.0 ms. macOS NOT measured.
Why: the lead needs these before P5 to set the plugin floor and the mod flow. How to apply: re-verify only if the pinned Claude Code version moves; the lead owns the design file, so update it from these results.
