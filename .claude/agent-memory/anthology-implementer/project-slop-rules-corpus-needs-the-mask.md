---
name: 'Slop rule tests: the corpus needs the Task 2 mask, so check rules with a scratch mask'
description: 'Before the Markdown mask lands, corpus entries with code, quotes or tables fail for the mask, not the rule; how to tell and prove it'
metadata:
  type: project
  modified: 2026-11-28
---

The slop corpus tests (crates/anthology-slop/tests/corpus.rs and crates/repo-tests/tests/slop_corpus.rs) run the rules on the Markdown view. While mask/markdown.rs is the Task 1 stub (View::plain), every entry that depends on masking fails: inline code, quoted spans, blockquotes, URLs, fences, ignore regions and table dashes. Why it matters: a rule PR that lands before the mask looks red for a reason it does not own. How to tell: every flagged line sits inside a fence or backticks (awk over the fence markers shows the ranges), and the docs outside docs/superpowers/plans are clean even unmasked. How to prove: write a throwaway mask/markdown.rs (common::regions, blank, mask_urls, mask_quotes, fences, inline code, blockquote lines, table rows as dash_skip), run both corpus tests, then git checkout the file; never commit it. The plan file itself is in the corpus (docs/superpowers/plans/2026-11-28-anti-slop-gate.md) and needs a real double-backtick inline-code mask for its Task 12 smoke line.
