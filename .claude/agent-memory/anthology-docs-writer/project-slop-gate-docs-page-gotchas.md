---
name: 'Writing docs that pass the slop corpus'
description: 'How to show specimens and waiver syntax in docs/slop-gate.md without tripping the slop rules'
metadata:
  type: project
  modified: 2026-11-28
---

Every specimen, rule ID and stock word goes in backticks or a fenced block; prose outside code is checked by the text and density rules (crates/repo-tests/tests/slop_corpus.rs runs them over README.md, the top-level docs and docs/**). Avoid em dashes, quoted-contrast sentences and stray words such as seamless, even inside a product name, unless in backticks.
Why: the region marker regex in crates/anthology-slop/src/mask/common.rs does not know about code fences, so a whole-line ignore-start marker inside a fenced example opens a real region and shows up as a waiver in slop check. Prefix the example lines with a label so the marker is not at line start.
The page test slop_gate_page_names_every_rule in crates/repo-tests/tests/docs.rs needs each RULES id in backticks; a second test rejects unknown ids.
Until the masking and rule tasks merge, the corpus test passes on stubs, so check prose by hand: strip code spans and grep for the vocabulary list.
