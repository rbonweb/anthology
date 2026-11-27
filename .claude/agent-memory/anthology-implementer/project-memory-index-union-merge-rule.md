---
name: 'Memory index union-merge rule: where it lives and what git needs'
description: 'approve() adds merge=union lines; setup preview shows them as memory.repo.union_attrs; git reads the rule from the merging checkout, GitHub ignores it'
metadata:
  type: project
  modified: 2026-11-27
---

The rule lives in crates/anthology-memory/src/approve.rs (UNION_LINES, plan_union_attrs, ensure_union_attrs) and runs on setup apply for an approved repo only; anthology-mem save is not gated on approval, so a repo where setup never ran or approval was skipped has agent-memory indexes with no rule. Setup preview shows the diff as memory.repo.union_attrs (setup.rs, union_attrs_preview). Real-git tests: crates/anthology-memory/tests/union_merge.rs. Measured: without the rule two branches adding an index line conflict (add/add and modify/modify); with the rule committed on the base, or only in the merging working tree, both merge clean. A worktree or clone made from a commit without the rule, and GitHub merge, still conflict; anthology-mem reindex rebuilds the index. Why: field report of repeated MEMORY.md conflicts. How to apply: before changing this, read union_merge.rs for the measured cases.
