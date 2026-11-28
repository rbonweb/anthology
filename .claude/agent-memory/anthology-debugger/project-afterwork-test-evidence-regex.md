---
name: 'After-work test evidence is a regex list, not a word'
description: 'Why TESTED in afterwork.ts lists verbs and states, and which nouns must stay out'
metadata:
  type: project
  modified: 2026-11-28
---

plugins/anthology/hooks/mods/afterwork.ts TESTED (test evidence) was a closed word list and raised a false "nothing about what was tested" for reports that verified by hash, curl, screenshot or comparison (PR 314). It now lists verbs and states (verified, checked, compared, ran, hash matched, curl status, N passed). The bare nouns check and verification stay out: the report format has a Verification heading and the test fixture FULL says "added the after-work check", so a noun match would make the gap test pass for nothing. Negations (not verified, did not check anything, couldn't run it) are cut first in NOT_TESTED. Run the tests with sh ci/jobs/mods.sh (Docker, about 1 minute); sh ci/jobs/mods.sh tsc for types.
