---
name: 'right-first-time'
description: 'Owner wants work right the first time with few revision rounds; skill, prompt and doc changes get no local test rounds, CI is the check'
metadata:
  type: feedback
  modified: 2026-11-28
---

The owner, 2026-11-28: "do not do that much testing because it's a skill change" and "whenever you are implementing ... it should be perfect from the start, so fewer and fewer revisions are required", also for the Anthology agents.

**Why:** test-and-revise loops cost the owner hours of wall time; most rounds today were rework (conflicts, budget overruns, wrong hashes) that careful first drafts avoid.

**How to apply:** brief agents with everything they need so the first draft is final: exact files, budgets, version rules and pins. For skill, prompt, guide and doc changes, the agent runs only the generators the change needs (vendor sync, golden bless) and lets CI check; no local test rounds. Code changes still get tests, written once, targeted. Related: [[owner-build-rules-2026-11-21]], [[ci-affected-only]].
