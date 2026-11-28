---
name: 'Auto-mode rewrite report: check the ATL binary version first'
description: 'Why a plain grep was rewritten in auto mode on a current plugin, and the shim guard that now covers an old binary'
metadata:
  type: project
  modified: 2026-11-28
---

A report of "a hook changed this input after auto mode reviewed it" was ATL 1.2.1-anthology.6 (state.json atl_path, not the plugin atl.lock): the binary the shim runs is whatever state.json names, updated only by a session start. Auto-mode handling lives in atl (Mode::of reads top-level permission_mode, read_only_proof.rs) from .7 on; .9 does not rewrite there (checked by running the installed binary). Claude Code sends the live mode (permission_mode is the current permission context), so a mid-session switch is seen. bin/atl-hook now also drops a plain rewrite (updatedInput, no ask or deny) when permission_mode is auto, so a stale binary cannot break auto mode. An old binary ask in auto mode still carries its rewrite; the shim does not strip it. How to diagnose: session transcript jsonl in the project folder under ~/.claude/projects has permissionMode per user line and hook_success attachments with the hook stdout; compare the timestamp (UTC) with the mtime of .last-used in plugins/data atl version folders. Why: the lead asked whether detection missed auto mode; it did not, the binary was old.
