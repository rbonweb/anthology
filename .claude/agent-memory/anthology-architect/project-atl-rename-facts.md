---
name: 'The ATL rename: facts that shaped the plan'
description: 'Fork is Apache-2.0 only; guard peel ships in the anthology binary; old sessions read the legacy rtk_path key; setup stores only ops it applied'
metadata:
  type: project
  modified: 2026-11-22
---

ATL (atl/) is a fork of rtk by RTK AI Labs, the Apache-2.0 edition only (BUSL engine, plugin and proxy trimmed); atl/LICENSE says Copyright 2026 RTK AI Labs and upstream has no NOTICE file, so Apache 4(a)-(c) and 6 (trademark) are what apply. One atl/NOTICE ships in every archive.
The guardrails guard peels atl forms, and the legacy rtk forms, in crates/anthology-shell (prefixes.rs, atl_forms.rs) and runs inside the anthology binary pinned by plugins/anthology-guardrails/anthology.lock, so a new wrapper name needs an anthology-bin release and a tighter guardrails dependency in plugins/anthology/.claude-plugin/plugin.json.
atl/ path-depends on crates/anthology-shell (ATL_PASSTHROUGH, ATL_RUNNERS, analyze), so the two must rename together.
Old sessions run their cached plugin hook, which reads the legacy rtk_path key of state.json; setup stores in settings_ops only ops that changed settings.json, so a stored untagged legacy Bash(rtk *) add proves setup added it (a user own rule is never stored).
17 gzip goldens held the old name; plain git grep misses them (use gzip -dc), and design-taste-frontend images match it by chance (use git grep -I). Frozen goldens keep their case inputs, so bless alone never removes the old name from them.
Why: each of these changed the rename plan and none is visible from one file.
How to apply: recheck before planning any rename, wrapper or release-name change.
