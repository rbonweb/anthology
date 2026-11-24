---
name: 'The guard does not model shell aliases'
description: 'An alias defined, then used on a later line, ran a blocked program; closed in bin 0.1.4'
metadata:
  type: project
  modified: 2026-11-22
---

Status: closed in anthology-bin 0.1.4 (see crates/anthology-guard/tests/param_expansion.rs and g0_fp.rs).

shopt -s expand_aliases, then alias u set to npx playwright install, then u on the next line: Allow on build b42ae431 and on PR 153 head 1ef034a5; bash 5.2 runs it. zsh 5.9 -c did not expand an alias defined in the same string in the probe container. With an alias value holding a quoted name followed by a bracket and an escaped backtick pair, build blocked G0 and head allowed, so alias belongs in the TEXT_CONSUMERS gate of plain_text in crates/anthology-shell/src/collect.rs.
Why: found in the PR 153 re-review (2026-11-22) and reported as pre-existing.
How to apply: re-probe in later guard reviews; check how Claude Code really runs the command in zsh (eval of the text may expand aliases) before rating severity.
