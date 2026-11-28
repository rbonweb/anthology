---
name: 'Slop e2e fixture: the density cases are long on purpose'
description: 'should-fail.txt density lines need 150+ (dashes) and 300+ (lists of three) new words, or the e2e fails'
metadata:
  type: project
  modified: 2026-11-28
---

In crates/anthology-slop/tests/fixtures/should-fail.txt the text.em-dash-density and text.triad-density cases are single long lines: the plan sentences plus plain filler. Why: the density rules (src/rules/density.rs) stay quiet below MIN_WORDS_DASH (150) and MIN_WORDS_TRIAD (300) new words, and fire only when count x words-per-unit exceeds the word count (dashes 1 per 80 words, lists 1 per 150), so three dashes need 150 to 239 words and three lists need 300 to 449. If those constants move, lengthen or shorten the filler; keep the case in the middle of its range. Other cases stay the plan one-liners. Count words with: sed -n Np file | cut -f3 | wc -w (dash tokens included).
