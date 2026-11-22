---
name: 'rtk to ATL rename: facts that shaped the plan'
description: 'Fork is Apache-2.0 only; guard peel ships in the anthology binary; old sessions read rtk_path; setup stores only ops it applied'
metadata:
  type: project
  modified: 2026-11-22
---

The rtk fork in rtk/ is the Apache-2.0 edition only (BUSL engine, plugin and proxy trimmed); LICENSE says Copyright 2026 RTK AI Labs and there is no upstream NOTICE file, so Apache 4(a)-(c) and 6 (trademark) are what apply.
The guardrails guard peels rtk forms in crates/anthology-shell (prefixes.rs, rtk_forms.rs) and runs inside the anthology binary pinned by plugins/anthology-guardrails/anthology.lock, so a new wrapper name needs an anthology-bin release and a tighter guardrails dependency in plugins/anthology/.claude-plugin/plugin.json.
rtk/ path-depends on crates/anthology-shell (RTK_PASSTHROUGH, RTK_RUNNERS, analyze), so the two must rename together.
Old sessions run their cached plugin hook, which reads the rtk_path key of state.json; setup stores in settings_ops only ops that changed settings.json, so a stored Bash(rtk *) proves setup added it.
17 gzip goldens contain rtk; plain git grep misses them (use gzip -dc), and design-taste-frontend images match rtk by chance (use git grep -I).
Why: each of these changed the rename plan and none is visible from one file.
How to apply: recheck before planning any rename, wrapper or release-name change.
