---
name: 'Routing guide size budget'
description: 'routing-guide.md has a 3800-char test cap and is near it; how to make room'
metadata:
  type: project
  modified: 2026-11-22
---

plugins/anthology/templates/routing-guide.md is capped at 3800 characters by guide_test::test_size_and_shape in crates/anthology-runtime/tests/session_start.rs, and sat at about 3790 before the lead-delegation change.
**Why:** the guide is injected into every session, so size is a context cost.
**How to apply:** measure with wc -m (characters, not bytes). To add a rule, tighten wording elsewhere: the section 2 table needs no spaces around pipes, and the tests pin exact phrases (commit before finishing, Before each merge or pull, the ui-designer builds then ui-reviewer checks line), so keep those. Run them with cargo test -p anthology-runtime --test session_start guide_test.
