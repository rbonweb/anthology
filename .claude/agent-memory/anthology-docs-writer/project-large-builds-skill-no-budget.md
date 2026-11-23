---
name: 'large-builds skill has no size budget or golden'
description: 'Editing plugins/anthology/skills/large-builds/SKILL.md needs only the needle tests; no budget, no golden bless'
metadata:
  type: project
  modified: 2026-11-22
---

The large-builds skill has no size or word budget. Only two needle tests read it: roster test_large_builds_skill_holds_the_detail_behind_the_guide_rules and session_start skills_folder_test (keep existing headings and phrases). The golden surfaces dev.startup-budget and hook.session-start did not drift after adding three rules, so no bless was needed.
**Why:** the routing guide has a size budget but the skill does not; checking this took several greps.
**How to apply:** for a skill-only rules edit run the golden check for those two surfaces (scripts/dev.sh golden check --surface dev.startup-budget,hook.session-start) and the two test filters, then stop. Version file is plugins/anthology/.claude-plugin/plugin.json; pass that path to release-edit set-version.
