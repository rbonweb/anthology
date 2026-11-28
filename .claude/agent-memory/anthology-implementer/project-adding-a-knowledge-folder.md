---
name: 'Adding a vendored knowledge folder: every list and count it touches'
description: 'New knowledge folder needs a Startup read, roster and smoke lists, five count tests, rename-skill if names differ; patch recipe'
metadata:
  type: project
  modified: 2026-11-28
---

Adding a vendored knowledge folder (website-clone, 2026-11-28) touches more than vendor/sources.json and a patch:
- every knowledge folder must be read in some agent's Startup section (roster.rs test_no_loose_knowledge) and be in repo::expected_knowledge (crates/repo-tests/src/repo.rs), so the reading agent's Startup gets the path even for a conditional read (reviewer's tech-debt line is the precedent: a list item ending in "only when ...");
- the reading agent's knowledge list in repo.rs expected_roster and in crates/anthology-dev/tests/smoke.rs (UI_KNOWLEDGE and EXPECTED_ROSTER);
- counts: repo.rs unit test, knowledge.rs (expected set), licenses.rs (folders, sources, checked fixed texts), crates/anthology-dev/tests/vendor/repo_api_test.rs, and the names list in sources_json_test.rs;
- the Startup file counts toward the agent's startup budget even when the read is conditional; then bless dev.startup-budget.
The vendor tool has no overlay transform (only rename-plugin, strip-connectors, rename-skill and patch), but git apply can add new files from a patch. Build the patch with git diff --no-index --no-prefix between a copy of upstream after the earlier transforms and the adapted copy, run from a script file, then vendor sync <name> --ref <tag>; the sync also writes vendor/licenses/<owner>-<repo>.txt. A skill whose name differs from its folder needs rename-skill, because knowledge.rs requires the frontmatter name to equal the folder.
Why: the first website-clone draft would have failed six tests in CI that only the count and roster lists reveal.
