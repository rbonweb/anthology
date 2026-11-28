---
name: 'Slop text rules: proper-noun and factual-clause guards'
description: 'How text.rs tells names and technical phrases from tells, and the 20-entry pin on should-pass.txt'
metadata:
  type: project
  modified: 2026-11-28
---

crates/anthology-slop/src/rules/text.rs (review item 3, 2026-11-28): ai_vocab skips a capitalized hit that is link text ([Delve] or [Delve before a masked ](url)), sits mid-sentence, or is followed by a capitalized word or a version (Tapestry 5), unless at least two thirds of the nearby words on its line are capitalized (a Title Case heading such as Elevate Your Workflow still fires). Why: the reviewer suggestion alone would have let every Title Case marketing heading through. All-caps words are not capitalized, so shouting still fires. not_x_but_y forms 1 to 3 skip a second clause made of optional adverbs plus a past participle (it is also read, it is set by); a lead-style allowlist of a/an/the/about would have broken the existing positive "they are costly". hype_cta "to the next level" needs a slogan verb (take, bring, elevate, push, kick, boost) in its own sentence and no descent verb right before; "say goodbye to" skips a version object only, so "slow builds" and "bugs" still fire. Every window is bounded (40 to 200 bytes) so the rules stay linear. crates/anthology-slop/tests/corpus.rs pins should-pass.txt at exactly 20 entries; new allow sentences go as extra paragraphs inside the topical entries unless that pin is changed.
