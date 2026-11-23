---
name: 'Mods integration (PR 175) review findings'
description: 'What combining the four mods added at head e1bdf942, the CHANGELOG mis-merge, and which dispatch mutants die'
metadata:
  type: project
  modified: 2026-11-22
---

At head e1bdf942 the only shared event is agent.report; index.ts calls next once and runs afterworkReport then alertsReport, each in util.ts isolated(). tool.check (Bash only, permissions) and tool.call (Edit, Write, NotebookEdit, delegation) have disjoint matchers and no shared state, so they cannot interact. Mutants run from a git archive tar on stdin: dropping either isolated(), a wrong afterwork fallback, or a second unmatched agent.report registration all fail claude plugin test (the kit reports: registered twice without a matcher; a second registration with a distinct matcher is allowed). Swapping the two parts survives and is harmless.
The merge of origin/build (release 0.2.9) into mods-integration put every mods CHANGELOG entry under the released 0.2.9 heading and left Unreleased empty, so release.sh changelog-check refuses the next anthology release and the notes claim 0.2.9 shipped the mods.
Why: a merge of a release commit into a long-lived branch silently moves the branch Unreleased entries into the released section, because the release only inserted a heading line above them.
How to apply: on any branch that merged a release from build, diff the CHANGELOG section of the released tag against the branch. The node:22-bookworm-slim image has no git; use diff -rq to confirm a mutant applied.
