---
name: 'Slop fixtures: counts pinned outside the fixture files'
description: 'Adding a rule or a should-fail line breaks pinned counts in e2e.rs and corpus.rs; where they are'
metadata:
  type: project
  modified: 2026-11-28
---

crates/anthology-slop/tests/e2e.rs pins the should-fail case count (the_fixture_holds_N_cases) and every rule needs one case, so a new rule or line means editing that count. crates/anthology-slop/tests/corpus.rs pins should-pass.txt at 20 entries and runs only the Markdown kind: add extra lines inside an existing entry, not a new entry. Why: a plan that lists only the fixture files still fails these tests; the pin edit is one line and unavoidable.
