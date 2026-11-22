---
name: 'Routing guide size budget'
description: 'routing-guide.md has a 3800-char test cap and is near it; how to make room'
metadata:
  type: project
  modified: 2026-11-22
---

plugins/anthology/templates/routing-guide.md is capped at 3800 characters by GuideTest.test_size_and_shape in tests/test_session_start.py, and sat at about 3790 before the lead-delegation change.
**Why:** the guide is injected into every session, so size is a context cost.
**How to apply:** measure with wc -m (characters, not bytes). To add a rule, tighten wording elsewhere: the section 2 table needs no spaces around pipes, and the tests pin exact phrases (commit before finishing, Before each merge or pull, the ui-designer builds then ui-reviewer checks line), so keep those. Run the tests from the tests folder: python -m unittest test_session_start, because the module imports _mem.
