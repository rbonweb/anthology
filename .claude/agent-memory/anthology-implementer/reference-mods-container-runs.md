---
name: 'Running the Claude Code mods checks in a container'
description: 'ci/jobs/mods.sh runs validate and plugin test; tsc mode is the dev type check; a long-lived dev container saves the npm ci on each run'
metadata:
  type: reference
  modified: 2026-11-22
---

Run sh ci/jobs/mods.sh from the worktree root for claude plugin validate --strict plus claude plugin test of plugins/anthology, in the pinned node image (ci/images/node.Dockerfile), with Claude Code from ci/claude-code installed under /tmp in the container. Each run spends 40 s to 2 min on npm ci.
sh ci/jobs/mods.sh tsc is the dev type check (typescript pinned to 5.9.3). scripts/mods-types.mjs takes the declarations out of the native binary and writes them to the gitignored plugins/anthology/.claude-plugin/types/, then runs tsc.
Fast loop: start one named container from the node image with the worktree mounted read-only and HOME=/tmp/home, sleep in it, run npm ci into /tmp/cc once with docker exec, then docker exec /tmp/cc/node_modules/.bin/claude plugin test plugins/anthology per change (under a second). After one tsc run has laid the types, npx -y -p typescript@5.9.3 tsc -p plugins/anthology works there too. Remove the container at the end.
Traps:
- validate does not type-check this plugin (it prints types: not checked because plugin.json names dependencies), so only tsc catches type errors.
- The extracted file lacks the per-machine BuiltinToolInputs table, so narrowing e.file_path through e.tool does not type-check locally.
- plugin test exits 1 when the plugin has no *.test.ts at all.
- A mutant edit must stay in place until the CI run has read the file: the job mounts the live worktree.
Why: without this, every mods change costs a round of rediscovery.
