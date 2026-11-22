---
name: 'Routing guide size budget'
description: 'routing-guide.md has a 3800-char test cap and is near it; what was trimmed, where new rule tests go'
metadata:
  type: project
  modified: 2026-11-22
---

plugins/anthology/templates/routing-guide.md is capped at 3800 characters by guide_test::test_size_and_shape in crates/anthology-runtime/tests/session_start.rs. After the field-fix rules it sits at about 3788; measure with wc -m (characters, not bytes).
To add a rule, tighten wording elsewhere. The model-to-agent list and a few glosses were dropped to fit; the section 2 table needs no spaces around pipes. Tests pin exact phrases (commit before finishing, Before each merge or pull, the Lead, Inline only and project-map lines, the browser lines, the delete and attribution lines), so keep those verbatim.
Tests for newer guide rules and the large-builds skill detail live in crates/repo-tests/tests/roster.rs, not session_start.rs, because another change often edits the runtime crate and would conflict.
Why: the guide is injected into every session, so size is a context cost. Put detail in plugins/anthology/skills/large-builds and keep guide lines short.
