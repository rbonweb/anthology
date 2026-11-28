---
name: 'Guard G0 changes need a golden bless'
description: 'How a deliberate stricter guard or parser change gets past the frozen goldens, and where G0 must be signaled'
metadata:
  type: project
  modified: 2026-11-22
---

A deliberate behavior change in crates/anthology-shell or crates/anthology-guard shows up as unexplained diffs in the frozen goldens (guard.check, guard.hook, shell.analyze, shell.segments). Run `difftest golden check --surface guard.check,guard.hook,shell.analyze,shell.segments --report <file>` (about 10 minutes), confirm every diff is stricter (Python allowed, Rust G0; analyze now incomplete), then `difftest golden bless` on the same surfaces. Frozen Python test rows (crates/anthology-guard/tests/data/test_guard_rules.jsonl) are not edited; rule_table.rs has a STRICTER override list.
**Why:** ADR-027 says a deliberate change needs a blessed golden; CI runs the golden check, and a D-8 divergence entry only waives cases where Rust is stricter.
**How to apply:** signal a new G0 class through `Analysis.incomplete` (set a Ctx flag such as `unproven` and keep analyzing, so a G1 reading still names itself). Do not add a field to `Analysis`: rtk constructs it and its gate asks only on `incomplete`.
