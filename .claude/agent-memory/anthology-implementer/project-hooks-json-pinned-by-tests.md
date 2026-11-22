---
name: 'hooks.json event set is pinned in two tests'
description: 'Adding a hook event to plugins/anthology/hooks/hooks.json breaks two pinned-set assertions'
metadata:
  type: project
  modified: 2026-11-22
---

plugins/anthology/hooks/hooks.json event names are asserted as an exact set in tests/test_session_start.py (HooksJsonTest) and tests/test_rtk_install.py (FilesTest). Update both when adding an event.
Why: a new event otherwise fails CI on tests unrelated to the change.
How to apply: grep tests for the existing event names before editing hooks.json. tests/test_guard_cli.py covers the separate anthology-guardrails plugin, not this file. The repo has only a root README.md, no per-plugin README. Tests need pyyaml (tests/_repo.py imports it), so run them in python:3.12-slim after pip install pyyaml, from the tests directory.
