---
name: 'Mutating the anthology mods in a container'
description: 'Run claude plugin test mutants on plugins/anthology/hooks/mods from a git archive tar on stdin, no mount; which mutants survive by design'
metadata:
  type: project
  modified: 2026-11-22
---

Write the mutant script to a scratch dir, git archive HEAD plugins/anthology ci/claude-code to a tar there, then docker run -i --rm the pinned node image (ci/images/node.Dockerfile) with the script passed as an env var and eval, tar on stdin. Inside: npm ci the ci/claude-code pin under /tmp/cc, then sed one guard at a time and rerun claude plugin test plugins/anthology. About 2 minutes for 16 mutants.
The guardrails G0 check refuses a heredoc plus a pipe in one Bash call; split them.
On PR 164 head c22b28ae the marker, token, sandbox, newline, length, exit-code and kill-switch mutants died; v.ceiling, e.ceiling/isCeilingUnread/op, the Bash matcher and the .catch survived (the kit cannot raise ceiling; the non-Bash test passes through judgeable).
Why: the test kit cannot raise engine-pinned fields, so a pure-function test of judgeable is the only way to kill those survivors.
How to apply: rerun these mutants on later mods PRs; expect survivors to need direct unit tests of exported helpers.
