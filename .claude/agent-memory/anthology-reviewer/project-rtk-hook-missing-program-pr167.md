---
name: 'rtk-hook missing-program drop reviewed in PR 167'
description: 'Where the plain-rewrite drop in plugins/anthology/bin/rtk-hook is safe, its gaps, and how it was probed'
metadata:
  type: project
  modified: 2026-11-22
---

PR 167 adds a third block to plugins/anthology/bin/rtk-hook: after the ask and rtk read blocks, a plain rewrite (rc 0, updatedInput, no ask or deny in out) is dropped when the typed first word fails command -v or an rtk rg|grep|ag|find|fd in the rewrite names a missing program. Probed on debian dash, alpine ash and bash with a fake rtk and a PATH holding only sed/head/grep/tr/cat: deny and kept ask stayed, mutating the ask|deny guard line flipped both to DROP so the tests do catch it. Real rtk 1.2.1-anthology.4 (data dir under the plugin data folder) rewrites rg to rtk rg, grep to rtk grep, find to rtk find, and does not rewrite compound lines (cd x && rg, pipes); rtk rg without rg on PATH fails, rtk grep falls back to grep.
Gaps found: only the first word and search programs are checked (echo hi && git status keeps rtk git when git is missing); the first-word test applies to every program, so a hook PATH poorer than the Bash tool PATH (GUI-launched Claude Code) silently drops rewrites for cargo, npm, docker; rtk rg after a backtick or a JSON tab is not matched; the leading-space strip loop is quadratic (20000 spaces took 1s, hook timeout 60s with onFailure block).
Why: the guard hook G0-blocks running /bin/sh on a script file on the host, so probes ran in Docker only; BSD sed needs sed -i with an empty suffix.
How to apply: when reviewing later changes to this hook reuse the bare and full PATH probe and the guard-line mutation.
