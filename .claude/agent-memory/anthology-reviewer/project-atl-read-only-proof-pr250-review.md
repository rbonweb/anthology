---
name: 'ATL read-only proof (PR 250) review findings'
description: 'Head 6e76f198: cwd injection, PATH and symlink probes hold; HOME counts as a project root and the needless path emits an explicit allow'
metadata:
  type: project
  modified: 2026-11-27
---

Security review of PR 250 (atl/src/hooks/read_only_proof.rs), probed with real anthology and atl release builds in the pinned rust image from git archive.
Holds: payload cwd with ; $( && newline quote .. or a symlink gets no proof (literal_cd_target); a relative or empty ATL_HOOK_USER_PATH entry or a PATH dir in the project refuses; an unparseable settings file keeps the ask; deny comes first; symlink and .. reads outside the project ask; a recursive grep over a tree holding .ssh or .env asks.
Found: user_dirs::project_root walks up to the first .claude, so in a repo without .claude the root is HOME (from ~/.claude) and grep -n atl ~/secret.txt gets allow; the needless path answers permissionDecision allow, not a plain rewrite, so it allows even without Bash(atl *) where the same read with no atl mention only gets a rewrite with no decision; in auto mode a kept hook ask still carries updatedInput. Latency in the Linux container: hook 6.4 ms, plus about 4.5 ms (grep) or 8 ms (git) with the classifier, and the plugin shim alone costs 2.5 ms.
Why: the needless-ask allow sits on top of the plain shape; AllowRewrite there is parity with non-mention lines only when the host still decides.
How to apply: on a fix round re-run the HOME-root probe (project under HOME with no .claude, PATH without home entries) and the no-Bash(atl *) probe; the helper builds payloads with jq and runs atl hook claude with ATL_PERM_CLASSIFY pointing at the built anthology binary.
