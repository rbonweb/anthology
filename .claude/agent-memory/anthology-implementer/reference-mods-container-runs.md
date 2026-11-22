---
name: 'Running the Claude Code mods checks in a container'
description: 'ci/jobs/mods.sh runs validate and plugin test; ci/jobs/mods.sh tsc is the dev type check; what each does and does not cover'
metadata:
  type: reference
  modified: 2026-11-22
---

Run sh ci/jobs/mods.sh from the worktree root for claude plugin validate --strict plus claude plugin test of plugins/anthology, in the pinned node image (ci/images/node.Dockerfile), with Claude Code from ci/claude-code installed under /tmp in the container. Each run spends about 40 s on npm ci.
sh ci/jobs/mods.sh tsc is the dev type check. scripts/mods-types.mjs takes the declarations out of the native binary, where they sit as an embedded zstd frame that the npm package does not ship as a file. It writes them to the gitignored plugins/anthology/.claude-plugin/types/ and then runs tsc.
Traps:
- validate does not type-check this plugin. Because plugin.json names dependencies, it prints "types: not checked", so only the tsc run catches type errors.
- The extracted file lacks the BuiltinToolInputs table that the engine generates per machine, so narrowing e.file_path through e.tool does not type-check locally.
- plugin test exits 1 when the plugin has no *.test.ts at all.
Why: without this, every mods change costs a round of rediscovery.
