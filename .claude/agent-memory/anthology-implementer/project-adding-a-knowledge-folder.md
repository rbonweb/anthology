---
name: 'Adding a vendored knowledge folder: every list and count it touches'
description: 'New knowledge folder needs a Startup read, roster and smoke lists, count tests, evals cases, three budget tables, rename-skill if names differ, drop-paths for unwanted upstream files; patch recipe'
metadata:
  type: project
  modified: 2026-11-28
---

Adding a vendored knowledge folder (website-clone, humanizer and avoid-ai-design, 2026-11-28) touches more than vendor/sources.json and a patch:
- every knowledge folder must be read in some agent's Startup section (roster.rs test_no_loose_knowledge) and be in repo::expected_knowledge (crates/repo-tests/src/repo.rs), so the reading agent's Startup gets the path even for a conditional read (reviewer's tech-debt line is the precedent: a list item ending in "only when ...");
- the reading agent's knowledge list in repo.rs expected_roster and in crates/anthology-dev/tests/smoke.rs (UI_KNOWLEDGE and EXPECTED_ROSTER);
- counts: repo.rs unit test, knowledge.rs (expected set), licenses.rs (folders, sources, checked fixed texts), crates/anthology-dev/tests/vendor/repo_api_test.rs, and the names list in sources_json_test.rs;
- evals/knowledge-<agent>/case.yaml for each non-worktree reader (marketer, product-planner, ui-reviewer, scout...): append read-, order- and inorder- graders in Startup order, or evals_layout test_knowledge_cases_read_the_startup_files_in_order fails;
- the Startup file counts toward the agent's startup budget even when the read is conditional (until the copy-and-marketing task A1 lands). The BUDGET table is in three files (src/startup_budget.rs, tests/startup_budget.rs the_budget_table_is_gc13_verbatim, repo-tests roster.rs), then bless dev.startup-budget; dev.upstream-watch did not drift when only these rows moved (check first);
- an agent body is capped at 60 lines (blank lines count): two added lines to ui-designer needed two existing bullets joined into their neighbours.
The vendor tool has no overlay transform (only rename-plugin, strip-connectors, rename-skill, drop-paths and patch), but git apply can add new files from a patch. To leave upstream files out, use drop-paths with a paths list (crates/anthology-dev/src/vendor/transforms.rs drop_paths, checked in manifest.rs check_transforms): a missing path fails the build, so an upstream rename is caught; a link on the way is refused. A patch file must exist before vendor build runs, so for a new entry set transform to [] first, run vendor fetch and build, edit the shipped tree, run vendor diff <name> > vendor/patches/<name>.patch (it handles added and deleted files), restore the patch transform, rebuild and run vendor check. Another route: git diff --no-index --no-prefix between a copy of upstream after the earlier transforms and the adapted copy, run from a script file, then vendor sync <name> --ref <tag>. vendor fetch and sync also write vendor/licenses/<owner>-<repo>.txt. A skill whose name differs from its folder needs rename-skill, because knowledge.rs requires the frontmatter name to equal the folder.
Why: the first website-clone draft would have failed six tests in CI that only the count and roster lists reveal; the humanizer task found the evals cases, the third budget table and the line cap the plan did not list.
