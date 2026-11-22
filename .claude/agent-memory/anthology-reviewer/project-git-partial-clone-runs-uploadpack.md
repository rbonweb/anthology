---
name: 'git log -p in a partial clone runs remote.*.uploadpack'
description: 'Lazy fetch of missing blobs runs a config-named program; any git config denylist must cover it'
metadata:
  type: project
  modified: 2026-11-22
---

In a partial clone (remote.<name>.promisor, extensions.partialClone), git log -p, show and diff fetch missing blobs on demand, and the fetch runs remote.<name>.uploadpack (and can reach credential.helper, core.sshCommand, core.gitProxy). Confirmed in debian:bookworm-slim with git 2.39.5: a marker script set as uploadpack ran on plain git log -p.
Why: the P3 fix round of PR 147 lists the git config keys P4 must refuse (Proven doc in crates/anthology-shell/src/readonly.rs, ADR-032 in DECISIONS.md), and this vector was missing. A denylist of keys keeps missing vectors.
How to apply: when reviewing the P4 path check or any "read-only git" claim, probe the lazy-fetch vector, and push for an allowlist of repository-local config keys rather than a denylist.
