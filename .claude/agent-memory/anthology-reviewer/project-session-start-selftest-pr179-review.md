---
name: 'Session-start self-test softening (PR 179) review findings'
description: 'What PR 179 hides when time is low, and why the installed check matches the shim hot path'
metadata:
  type: project
  modified: 2026-11-22
---

At head ff91559a the installed check (bootstrap::is_installed) matches lib.sh anth_installed exactly (real dirs, executable non-empty file, non-empty .sha256, lock version dir; no hashing, same as the shim hot path). The not-attempted shim answer only arises from the shim missing state, so wrong version, empty .sha256, symlinked dir stay fail-closed; non-executable is a different (broken) message.
Gap: the low-time skip in session_start.rs self_test_in returns before any check, so with under 10 s of DEADLINE left a missing binary, an invalid lock or a guard that allows G1 is reported only as skipped. 28 of the 37 re-blessed goldens are this swap (old text was G1-allowed OFF, failing closed, or nothing); all 37 differ only in systemMessage lines, plus D-1 marks dropped on 2 cases.
Why: the cheap guard_binary_installed check could still run without a process.
How to apply: when reviewing later self-test changes, diff goldens per case by systemMessage line and check what real states the new skip path swallows.
