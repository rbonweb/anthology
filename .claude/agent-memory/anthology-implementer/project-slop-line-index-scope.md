---
name: 'Slop engine: line lookups go through a scoped LineIndex'
description: 'check_with installs one line-start index per document so mask code finds lines by binary search; new text and waivers are looked up, not scanned'
metadata:
  type: project
  modified: 2026-11-28
---

In crates/anthology-slop, mask code (regions, comment_waivers) is handed only the doc, so it cannot take an index argument. check_with and waivers_in enter mask::common::IndexScope, a thread-local holding one LineIndex keyed by the doc address and length; index_for returns it or builds one (correct but O(doc) per call, so only tests take that path). The engine sorts and merges the new spans (NewText) and keys block waivers by (rule, line) in a BTreeMap, so a hit is waived by any line its span covers (a wrapped phrase) in O(log n). Why: line_of, comment_waivers and touching scanned from the file start per hit or waiver, so 1 MiB with 170k hits ran for over ten minutes in debug; now the three perf tests together take a few seconds. Any new per-hit or per-waiver code must not rescan the doc. Tests in lib.rs pin 170k hits, 18k waiver lines and 85k new spans under 10 s.
